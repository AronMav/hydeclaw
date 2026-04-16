use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::IntoResponse,
};
use std::collections::HashMap;
use std::time::Instant;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

/// Eviction threshold for rate limiter `HashMaps`.
/// When the map exceeds this size, expired entries are cleaned up.
const RATE_LIMITER_EVICT_THRESHOLD: usize = 50;

/// Tracks failed auth attempts per IP. After `max_attempts` failures within the window,
/// the IP is locked out for `lockout_secs` seconds.
pub(crate) struct AuthRateLimiter {
    max_attempts: u32,
    lockout_secs: u64,
    /// IP → (`fail_count`, `first_fail_time`, `locked_until`)
    #[allow(clippy::type_complexity)]
    state: Mutex<HashMap<String, (u32, Instant, Option<Instant>)>>,
}

impl AuthRateLimiter {
    pub(crate) fn new(max_attempts: u32, lockout_secs: u64) -> Self {
        Self {
            max_attempts,
            lockout_secs,
            state: Mutex::new(HashMap::new()),
        }
    }

    async fn is_locked(&self, ip: &str) -> bool {
        let state = self.state.lock().await;
        if let Some((_, _, Some(locked_until))) = state.get(ip)
            && Instant::now() < *locked_until {
                return true;
            }
        false
    }

    async fn record_failure(&self, ip: &str) {
        let mut state = self.state.lock().await;
        let now = Instant::now();

        // Evict expired entries to prevent unbounded growth from bot scans
        if state.len() > RATE_LIMITER_EVICT_THRESHOLD {
            let lockout = std::time::Duration::from_secs(self.lockout_secs);
            state.retain(|_, (_, first_fail, locked_until)| {
                if let Some(until) = locked_until {
                    now < *until // keep if still locked out
                } else {
                    now.duration_since(*first_fail) < lockout // keep if window active
                }
            });
        }

        let entry = state.entry(ip.to_string()).or_insert((0, now, None));

        // Reset if previous window expired (no lockout active)
        if entry.2.is_none() && now.duration_since(entry.1).as_secs() > self.lockout_secs {
            *entry = (0, now, None);
        }

        entry.0 += 1;
        if entry.0 >= self.max_attempts {
            let lockout_until = now + std::time::Duration::from_secs(self.lockout_secs);
            entry.2 = Some(lockout_until);
            tracing::warn!(ip = %ip, "auth rate limit: IP locked for {}s after {} failed attempts", self.lockout_secs, self.max_attempts);
        }
    }

    async fn record_success(&self, ip: &str) {
        let mut state = self.state.lock().await;
        state.remove(ip);
    }
}

/// Per-IP request rate limiter using a fixed-window counter.
/// Protects the Pi from overload by limiting requests per minute.
pub(crate) struct RequestRateLimiter {
    max_per_minute: u32,
    /// IP → (`request_count`, `window_start`)
    state: Mutex<HashMap<String, (u32, Instant)>>,
}

impl RequestRateLimiter {
    pub(crate) fn new(max_per_minute: u32) -> Self {
        Self {
            max_per_minute,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Returns Ok(()) if allowed, `Err(seconds_until_reset)` if rate-limited.
    async fn check(&self, ip: &str) -> std::result::Result<(), u64> {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);

        // Evict stale entries to prevent unbounded growth from bot scans
        if state.len() > RATE_LIMITER_EVICT_THRESHOLD {
            state.retain(|_, (_, start)| now.duration_since(*start) < window);
        }

        let entry = state.entry(ip.to_string()).or_insert((0, now));

        // Reset window if expired
        if now.duration_since(entry.1) >= window {
            *entry = (0, now);
        }

        entry.0 += 1;

        if entry.0 > self.max_per_minute {
            let elapsed = now.duration_since(entry.1).as_secs();
            let retry_after = 60u64.saturating_sub(elapsed);
            Err(retry_after)
        } else {
            Ok(())
        }
    }
}

/// Per-IP budget for concurrent WebSocket upgrades (pre-auth).
/// Prevents `DoS` via mass WS upgrade requests before auth is checked.
pub(crate) struct WsConnectionBudget {
    max_per_ip: u32,
    /// IP → active connection count
    counts: Mutex<HashMap<String, u32>>,
}

impl WsConnectionBudget {
    pub(crate) fn new(max_per_ip: u32) -> Self {
        Self { max_per_ip, counts: Mutex::new(HashMap::new()) }
    }

    pub(crate) async fn acquire(&self, ip: &str) -> bool {
        let mut counts = self.counts.lock().await;
        let count = counts.entry(ip.to_string()).or_insert(0);
        if *count >= self.max_per_ip {
            return false;
        }
        *count += 1;
        true
    }

    pub(crate) async fn release(&self, ip: &str) {
        let mut counts = self.counts.lock().await;
        if let Some(count) = counts.get_mut(ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(ip);
            }
        }
    }
}

pub(crate) async fn request_rate_limit_middleware(
    req: Request<Body>,
    next: Next,
    limiter: &'static RequestRateLimiter,
    ws_budget: &'static WsConnectionBudget,
) -> impl IntoResponse {
    let path = req.uri().path();
    // Exempt health from rate limiting
    if path == "/health" {
        return next.run(req).await;
    }

    // WebSocket: enforce connection budget instead of request rate
    if path.starts_with("/ws") {
        let client_ip = extract_client_ip(&req);
        if !ws_budget.acquire(&client_ip).await {
            tracing::warn!(ip = %client_ip, "WS connection budget exceeded");
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        let resp = next.run(req).await;
        // Release on response (upgrade failures and normal responses)
        ws_budget.release(&client_ip).await;
        return resp;
    }

    let client_ip = extract_client_ip(&req);

    // Exempt loopback from request rate limiting (internal services: toolgate, channels, engine)
    if is_loopback(&client_ip) {
        return next.run(req).await;
    }

    match limiter.check(&client_ip).await {
        Ok(()) => next.run(req).await,
        Err(retry_after) => {
            tracing::warn!(ip = %client_ip, "rate limited: {} req/min exceeded", limiter.max_per_minute);
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                format!("Rate limit exceeded. Retry after {retry_after}s."),
            ).into_response();
            response.headers_mut().insert(
                "Retry-After",
                retry_after.to_string().parse().expect("integer is valid header value"),
            );
            response
        }
    }
}

pub(crate) fn extract_client_ip(req: &Request<Body>) -> String {
    // Use actual TCP peer address (ConnectInfo) — not spoofable.
    // X-Forwarded-For/X-Real-IP are ignored because there is no trusted reverse proxy.
    req.extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map_or_else(|| "unknown".to_string(), |ci| ci.0.ip().to_string())
}

/// Check if an IP string represents a loopback address.
/// Handles: "127.0.0.1", "`::1`", "`::ffff:127.0.0.1`".
/// "unknown" (missing `ConnectInfo`) is NOT treated as loopback — unknown origin must authenticate.
pub(crate) fn is_loopback(ip: &str) -> bool {
    ip == "127.0.0.1" || ip == "::1" || ip.starts_with("::ffff:127.")
}

pub(crate) async fn auth_middleware(
    req: Request<Body>,
    next: Next,
    expected_token: &'static str,
    rate_limiter: &'static AuthRateLimiter,
    ws_tickets: std::sync::Arc<Mutex<HashMap<String, std::time::Instant>>>,
) -> impl IntoResponse {
    let path = req.uri().path();

    // ── Public paths (no auth required) ──────────────────────────────
    // /health              — liveness probe
    // /webhook/*           — per-endpoint auth (HMAC signatures)
    // /uploads/*           — UUID filenames, no secrets
    // /api/oauth/callback  — browser redirect from OAuth provider
    // /api/triggers/email/push — validates ?token= query param internally
    const PUBLIC_EXACT: &[&str] = &["/health", "/api/oauth/callback", "/api/triggers/email/push"];
    const PUBLIC_PREFIX: &[&str] = &["/webhook/", "/uploads/"];

    if PUBLIC_EXACT.contains(&path) || PUBLIC_PREFIX.iter().any(|p| path.starts_with(p)) {
        return next.run(req).await;
    }

    let client_ip = extract_client_ip(&req);
    tracing::debug!(ip = %client_ip, path = %path, loopback = is_loopback(&client_ip), "auth middleware");

    // ── Loopback-only paths (internal service calls) ─────────────────
    // /api/mcp/callback    — MCP server callbacks
    // /api/channels/notify — watchdog/internal alerts
    // /api/media/upload    — toolgate media uploads
    // /uploads/*           — static file serving
    // /ws*                 — WebSocket (validated separately via ticket)
    if is_loopback(&client_ip) {
        const LOOPBACK_EXACT: &[&str] = &["/health", "/api/mcp/callback", "/api/channels/notify", "/api/media/upload"];
        const LOOPBACK_PREFIX: &[&str] = &["/uploads/", "/ws"];
        let loopback_allowed = LOOPBACK_EXACT.contains(&path)
            || LOOPBACK_PREFIX.iter().any(|p| path.starts_with(p));
        if loopback_allowed {
            return next.run(req).await;
        }
        // All other loopback requests must still provide a valid auth token
    }

    let exempt_from_lockout = is_loopback(&client_ip);

    // Check Authorization header BEFORE lockout — a valid token always passes and clears lockout.
    // This prevents locking out legitimate users who accumulated failures (e.g. during login page reload).
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    if let Some(header) = auth_header
        && let Some(token) = header.strip_prefix("Bearer ")
        && token.as_bytes().ct_eq(expected_token.as_bytes()).into() {
            rate_limiter.record_success(&client_ip).await;
            return next.run(req).await;
        }

    // Only block fully unauthenticated requests (no header at all) from locked IPs.
    // If the request HAS an Authorization header (even invalid), return 401 not 429 —
    // the frontend will redirect to login, and the next valid-token request clears lockout.
    if !exempt_from_lockout && rate_limiter.is_locked(&client_ip).await && auth_header.is_none() {
        tracing::warn!(ip = %client_ip, path = %path, "auth rate limit: locked (no auth header)");
        return (StatusCode::TOO_MANY_REQUESTS, "Too many failed attempts. Try again later.").into_response();
    }

    // For WebSocket paths, also accept ?ticket= (one-time) or ?token= (legacy) query parameter
    // (browser WebSocket API cannot set custom headers)
    if path.starts_with("/ws")
        && let Some(query) = req.uri().query() {
            for pair in query.split('&') {
                // One-time ticket (preferred — avoids exposing static token in URL/logs)
                if let Some(val) = pair.strip_prefix("ticket=")
                    && crate::gateway::handlers::auth::validate_ws_ticket(&ws_tickets, val).await {
                        rate_limiter.record_success(&client_ip).await;
                        return next.run(req).await;
                    }
                // Legacy token= removed — use ticket= instead
            }
        }

    // Don't lock loopback — internal services must not be locked out.
    // Don't count static asset failures — browsers preflight these without tokens.
    let is_static_asset = path.starts_with("/_next/") || path.ends_with(".js") || path.ends_with(".css")
        || path.ends_with(".png") || path.ends_with(".jpg") || path.ends_with(".ico") || path.ends_with(".svg")
        || path.ends_with(".woff2") || path.starts_with("/api/setup/");
    if !exempt_from_lockout && !is_static_asset {
        rate_limiter.record_failure(&client_ip).await;
    }
    StatusCode::UNAUTHORIZED.into_response()
}
