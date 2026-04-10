use axum::{
    Router,
    middleware as axum_mw,
};
use tower_http::services::{ServeDir, ServeFile};
use serde::{Deserialize, Serialize};

pub(crate) mod middleware;
pub mod stream_registry;
pub mod stream_jobs;
pub mod state;
mod handlers;
pub use state::*;
use middleware::*;
// Re-export for use by main.rs
pub use handlers::agents::start_agent_from_config;
pub use handlers::email_triggers::renew_expiring_gmail_watches;
pub use handlers::channels::migrate_credentials_to_vault;
pub use handlers::providers::migrate_provider_keys_to_vault;
pub(crate) use handlers::backup::create_backup_internal;
pub(crate) use handlers::notifications::notify;

/// SSE event type constants for Vercel AI SDK v3 compatibility.
mod sse_types {
    pub const DATA_SESSION_ID: &str = "data-session-id";
    pub const START: &str = "start";
    pub const TEXT_START: &str = "text-start";
    pub const TEXT_DELTA: &str = "text-delta";
    pub const TEXT_END: &str = "text-end";
    pub const TOOL_INPUT_START: &str = "tool-input-start";
    pub const TOOL_INPUT_DELTA: &str = "tool-input-delta";
    pub const TOOL_INPUT_AVAILABLE: &str = "tool-input-available";
    pub const TOOL_OUTPUT_AVAILABLE: &str = "tool-output-available";
    pub const RICH_CARD: &str = "rich-card";
    pub const FILE: &str = "file";
    pub const SYNC: &str = "sync";
    pub const FINISH: &str = "finish";
    pub const ERROR: &str = "error";
    pub const APPROVAL_NEEDED: &str = "approval-needed";
    pub const APPROVAL_RESOLVED: &str = "approval-resolved";
}

/// Public OpenAI-format message — used by gateway AND referenced from engine::handle_openai.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    /// Vercel AI SDK 3.x format: array of message parts (text, reasoning, tool calls)
    #[serde(default)]
    pub parts: Option<Vec<MessagePart>>,
}

/// Part of a message in Vercel AI SDK 3.x format
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessagePart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(default)]
    pub text: Option<String>,
}

pub fn router(state: AppState) -> anyhow::Result<Router> {
    let auth_token = state
        .config
        .gateway
        .auth_token_env
        .as_ref()
        .and_then(|env_name| std::env::var(env_name).ok());

    let app = Router::new()
        .merge(handlers::chat::routes())           // /health, /api/chat, /v1/chat/completions, /v1/models, /v1/embeddings, /api/mcp/callback
        .merge(handlers::auth::routes())            // /api/auth/ws-ticket
        .merge(handlers::channel_ws::routes())      // /ws, /ws/channel/{agent_name}
        .merge(handlers::agents::routes())          // /api/agents/*, /api/approvals/*
        .merge(handlers::sessions::routes())        // /api/sessions/*, /api/messages/*
        .merge(handlers::monitoring::routes(&state))// /api/setup/*, /api/status, /api/stats, /api/usage/*, /api/doctor, /api/audit/*, /api/watchdog/*
        .merge(handlers::providers::routes())       // /api/providers/*, /api/provider-types, /api/media-drivers, /api/media-config, /api/provider-active
        .merge(handlers::network::routes())         // /api/network/addresses
        .merge(handlers::secrets::routes())         // /api/secrets/*
        .merge(handlers::memory::routes())          // /api/memory/*
        .merge(handlers::cron::routes())            // /api/cron/*
        .merge(handlers::tools::routes())           // /api/tool-definitions, /api/tools/*, /api/mcp/*
        .merge(handlers::yaml_tools::routes())      // /api/yaml-tools/*, /api/agents/*/yaml-tools/*
        .merge(handlers::skills::routes())          // /api/skills/*, /api/agents/*/skills/*
        .merge(handlers::channels::routes())        // /api/channels/*, /api/agents/*/channels/*, /api/agents/*/hooks
        .merge(handlers::config::routes())          // /api/config/*, /api/restart, /api/tts/*, /api/canvas/*
        .merge(handlers::backup::routes())          // /api/backup/*, /api/restore
        .merge(handlers::services::routes())        // /api/services/*, /api/containers/*
        .merge(handlers::webhooks::routes())        // /api/webhooks/*, /webhook/*
        .merge(handlers::oauth::routes())           // /api/oauth/*, /api/agents/*/oauth/*
        .merge(handlers::email_triggers::routes())  // /api/triggers/email/*
        .merge(handlers::github_repos::routes())    // /api/agents/*/github/repos/*
        .merge(handlers::access::routes())          // /api/access/*
        .merge(handlers::tasks::routes())           // /api/tasks/*
        .merge(handlers::notifications::routes())   // /api/notifications/*
        .merge(handlers::media::routes())           // /uploads/*, /api/media/*
        .merge(handlers::workspace::routes());      // /api/workspace/*

    // Auth middleware — REQUIRED. Refuse to start without a token.
    let app = if let Some(token) = auth_token {
        let shared_token: &'static str = Box::leak(token.into_boxed_str());
        let rate_limiter: &'static AuthRateLimiter = Box::leak(Box::new(AuthRateLimiter::new(500, 30)));
        let ws_tickets = state.ws_tickets.clone();
        app.layer(axum_mw::from_fn(move |req, next| {
            let ws_tickets = ws_tickets.clone();
            async move { auth_middleware(req, next, shared_token, rate_limiter, ws_tickets).await }
        }))
    } else {
        tracing::error!("FATAL: no auth token configured — refusing to start unauthenticated gateway");
        tracing::error!("set gateway.auth_token_env in config and provide the env var");
        anyhow::bail!("gateway requires auth token — set gateway.auth_token_env in hydeclaw.toml");
    };

    // Request rate limiting (per-IP, from config limits.max_requests_per_minute)
    let max_rpm = state.config.limits.max_requests_per_minute;
    let req_limiter: &'static RequestRateLimiter =
        Box::leak(Box::new(RequestRateLimiter::new(max_rpm)));
    let ws_budget: &'static WsConnectionBudget =
        Box::leak(Box::new(WsConnectionBudget::new(32)));
    let app = app.layer(axum_mw::from_fn(move |req, next| {
        request_rate_limit_middleware(req, next, req_limiter, ws_budget)
    }));

    // CORS: restrict to configured origins or derive from listen address.
    let cors_origins: Vec<axum::http::HeaderValue> = if state.config.gateway.cors_origins.is_empty() {
        // Derive from listen address: allow UI on same host (:5173) + API port
        let host = state.config.gateway.listen.split(':').next().unwrap_or("0.0.0.0");
        let port = state.config.gateway.listen.rsplit(':').next().unwrap_or("18789");
        let mut origins = vec![
            format!("http://{}:{}", host, port).parse().expect("valid CORS origin"),
            format!("http://{}:5173", host).parse().expect("valid CORS origin"),
        ];
        // For 0.0.0.0: also allow localhost + all local network interfaces
        if host == "0.0.0.0" {
            origins.push("http://localhost:5173".parse().expect("valid CORS origin"));
            origins.push(format!("http://localhost:{}", port).parse().expect("valid CORS origin"));
            // Add all non-loopback IPv4 addresses (for LAN access)
            for iface in get_local_ipv4_addrs() {
                if let Ok(v) = format!("http://{}:{}", iface, port).parse() { origins.push(v); }
                if let Ok(v) = format!("http://{}:5173", iface).parse() { origins.push(v); }
            }
            // Add Docker subnet gateway IPs for CORS
            for gw in get_docker_subnet_gateways(&state.config.gateway.cors_docker_subnets) {
                if let Ok(v) = format!("http://{}:{}", gw, port).parse() { origins.push(v); }
                if let Ok(v) = format!("http://{}:5173", gw).parse() { origins.push(v); }
            }
        }
        // Also add public_url origin if configured
        if let Some(ref pu) = state.config.gateway.public_url
            && let Ok(v) = pu.trim_end_matches('/').parse() { origins.push(v); }
        origins
    } else {
        state.config.gateway.cors_origins.iter()
            .filter_map(|o| o.parse().ok())
            .collect()
    };
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(cors_origins)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
        ]);
    // Serve static UI files from ui/out/ with SPA fallback to index.html.
    // API routes have priority (registered above); unmatched paths serve static files.
    // _next/ assets served WITHOUT fallback (404 if missing — prevents stale cache getting HTML).
    // All other paths fall back to index.html for SPA routing.
    let ui_dir = std::path::Path::new("ui/out");
    let app = if ui_dir.is_dir() {
        // _next/ assets are content-hashed → cache forever (immutable)
        let next_service = ServeDir::new(ui_dir.join("_next"));
        let app = app.nest_service(
            "/_next",
            tower_http::set_header::SetResponseHeader::overriding(
                next_service,
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
        );
        // HTML/other files → always revalidate
        let serve = tower_http::set_header::SetResponseHeader::overriding(
            ServeDir::new(ui_dir).fallback(ServeFile::new(ui_dir.join("index.html"))),
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        );
        app.fallback_service(serve)
    } else {
        app
    };

    let app = app.layer(cors);

    // Security headers: prevent MIME sniffing, clickjacking, XSS reflection
    let app = app.layer(axum_mw::from_fn(|req: axum::http::Request<axum::body::Body>, next: axum_mw::Next| async move {
        let mut response = next.run(req).await;
        let headers = response.headers_mut();
        headers.insert("X-Content-Type-Options", "nosniff".parse().expect("valid header value"));
        headers.insert("X-Frame-Options", "DENY".parse().expect("valid header value"));
        headers.insert("X-XSS-Protection", "1; mode=block".parse().expect("valid header value"));
        headers.insert("Referrer-Policy", "strict-origin-when-cross-origin".parse().expect("valid header value"));
        response
    }));

    Ok(app.with_state(state))
}

/// Get the primary non-loopback IPv4 address of the host (for CORS auto-derivation).
fn get_local_ipv4_addrs() -> Vec<String> {
    // UDP connect trick: connect to external IP (no actual traffic sent),
    // then read local_addr to get the outbound interface IP.
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0")
        && sock.connect("8.8.8.8:80").is_ok()
            && let Ok(local) = sock.local_addr()
                && !local.ip().is_loopback() {
                    return vec![local.ip().to_string()];
                }
    Vec::new()
}

/// Parse CIDR subnets and return their gateway IPs (.1 address).
/// e.g. "172.17.0.0/16" -> "172.17.0.1"
fn get_docker_subnet_gateways(subnets: &[String]) -> Vec<String> {
    subnets.iter().filter_map(|cidr| {
        let ip_part = cidr.split('/').next()?;
        let octets: Vec<&str> = ip_part.split('.').collect();
        if octets.len() == 4 {
            Some(format!("{}.{}.{}.1", octets[0], octets[1], octets[2]))
        } else {
            None
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::handlers::agents::{validate_agent_name, agent_config_path};
    use super::handlers::secrets::mask_secret_value;
    use super::handlers::workspace::format_workspace_size;
    use super::handlers::skills::skill_safe_name;

    // ── mask_secret_value ────────────────────────────────────────────────────

    #[test]
    fn mask_empty_string() {
        assert_eq!(mask_secret_value(""), "");
    }

    #[test]
    fn mask_short_3_chars() {
        assert_eq!(mask_secret_value("abc"), "***");
    }

    #[test]
    fn mask_exactly_8_chars() {
        assert_eq!(mask_secret_value("12345678"), "********");
    }

    #[test]
    fn mask_9_chars() {
        assert_eq!(mask_secret_value("123456789"), "1234...6789");
    }

    #[test]
    fn mask_12_chars() {
        assert_eq!(mask_secret_value("abcdefghijkl"), "abcd...ijkl");
    }

    // ── validate_agent_name ──────────────────────────────────────────────────

    #[test]
    fn validate_agent_name_valid_compound() {
        assert!(validate_agent_name("my-agent_1").is_ok());
    }

    #[test]
    fn validate_agent_name_single_char() {
        assert!(validate_agent_name("a").is_ok());
    }

    #[test]
    fn validate_agent_name_empty() {
        assert!(validate_agent_name("").is_err());
    }

    #[test]
    fn validate_agent_name_too_long() {
        let name = "a".repeat(33);
        assert!(validate_agent_name(&name).is_err());
    }

    #[test]
    fn validate_agent_name_special_chars() {
        assert!(validate_agent_name("my agent!").is_err());
    }

    #[test]
    fn validate_agent_name_dash_underscore() {
        assert!(validate_agent_name("my_agent-1").is_ok());
    }

    #[test]
    fn validate_agent_name_exactly_32_chars() {
        let name = "a".repeat(32);
        assert!(validate_agent_name(&name).is_ok());
    }

    // ── agent_config_path ────────────────────────────────────────────────────

    #[test]
    fn agent_config_path_main() {
        let path = agent_config_path("main");
        assert_eq!(path, std::path::Path::new("config/agents/main.toml"));
    }

    // ── format_workspace_size ────────────────────────────────────────────────

    #[test]
    fn format_workspace_size_zero() {
        assert_eq!(format_workspace_size(0), "0 B");
    }

    #[test]
    fn format_workspace_size_bytes() {
        assert_eq!(format_workspace_size(500), "500 B");
    }

    #[test]
    fn format_workspace_size_exactly_1_kb() {
        assert_eq!(format_workspace_size(1024), "1.0 KB");
    }

    #[test]
    fn format_workspace_size_1_5_kb() {
        assert_eq!(format_workspace_size(1536), "1.5 KB");
    }

    #[test]
    fn format_workspace_size_exactly_1_mb() {
        assert_eq!(format_workspace_size(1_048_576), "1.0 MB");
    }

    // ── skill_safe_name ──────────────────────────────────────────────────────

    #[test]
    fn skill_safe_name_unchanged() {
        assert_eq!(skill_safe_name("simple-name"), "simple-name");
    }

    #[test]
    fn skill_safe_name_slashes() {
        assert_eq!(skill_safe_name("path/to\\file"), "path-to-file");
    }

    #[test]
    fn skill_safe_name_spaces() {
        assert_eq!(skill_safe_name("name with spaces"), "name-with-spaces");
    }

    #[test]
    fn skill_safe_name_all_special_chars() {
        // : * ? " < > | all replaced with -
        assert_eq!(
            skill_safe_name("file:name*bad?\"<>|"),
            "file-name-bad-----"
        );
    }

    // ── docker_subnet_gateways ──────────────────────────────────────────────

    #[test]
    fn docker_subnet_gateway_basic() {
        let subnets = vec!["172.17.0.0/16".to_string()];
        let gws = super::get_docker_subnet_gateways(&subnets);
        assert_eq!(gws, vec!["172.17.0.1"]);
    }

    #[test]
    fn docker_subnet_gateway_multiple() {
        let subnets = vec![
            "172.17.0.0/16".to_string(),
            "172.18.0.0/16".to_string(),
        ];
        let gws = super::get_docker_subnet_gateways(&subnets);
        assert_eq!(gws, vec!["172.17.0.1", "172.18.0.1"]);
    }

    #[test]
    fn docker_subnet_gateway_empty() {
        let gws = super::get_docker_subnet_gateways(&[]);
        assert!(gws.is_empty());
    }

    #[test]
    fn docker_subnet_gateway_invalid() {
        let subnets = vec!["not-a-cidr".to_string()];
        let gws = super::get_docker_subnet_gateways(&subnets);
        assert!(gws.is_empty());
    }
}
