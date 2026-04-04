//! LLM error classification for context-specific recovery.
//!
//! Classifies anyhow errors from LLM providers into actionable categories,
//! enabling different recovery strategies (retry, compact, user message, etc.).

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmErrorClass {
    /// Context/prompt too large for the model.
    ContextOverflow,
    /// Orphan tool messages, invalid role ordering — session state is broken.
    SessionCorruption,
    /// Transient server errors (500, 502, 503, 504, 521-524, 529).
    TransientHttp,
    /// Rate limited (429, TPM/RPM exceeded).
    RateLimit,
    /// Authentication permanently failed (invalid/revoked API key).
    AuthPermanent,
    /// Billing/quota issue (402, insufficient credits).
    Billing,
    /// Provider overloaded (capacity, high demand).
    Overloaded,
    /// Unrecognized error.
    Unknown,
}

// ── Regex patterns (compiled once) ──────────────────────────────────────────

static RE_CONTEXT_OVERFLOW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)context.?length|token.?limit|too.many.token|input.too.long|prompt.is.too.long|maximum.context|exceeds.the.model|request_too_large|context.overflow|上下文").unwrap()
});

static RE_SESSION_CORRUPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)tool_use_block|tool_result.*not.*found|role.*ordering|roles.must.alternate|orphan.*tool|function.call.turn.comes.immediately|incorrect.role.information").unwrap()
});

static RE_TRANSIENT_HTTP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(500|502|503|504|521|522|523|524|529)\b|bad.gateway|gateway.timeout|without.sending.*(chunks?|response)").unwrap()
});

static RE_RATE_LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b429\b|rate.?limit|too.many.requests|tokens.per.minute|\btpm\b|resource.?exhausted|usage.?limit").unwrap()
});

static RE_AUTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\b(401|403)\b.*(api.?key|unauthorized|authentication|forbidden))|(api.?key.*(invalid|revoked|expired|deactivated))|(unauthorized|authentication.*(failed|error))|PERMISSION_DENIED").unwrap()
});

static RE_BILLING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b402\b|payment.required|insufficient.credit|quota.exceeded|insufficient.balance|insufficient.quota|billing").unwrap()
});

static RE_OVERLOADED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)overloaded|service.unavailable.*capacity|high.demand|overloaded_error").unwrap()
});

/// Classify an LLM error into an actionable category.
pub fn classify(error: &anyhow::Error) -> LlmErrorClass {
    let msg = error.to_string();
    classify_str(&msg)
}

/// Classify from a raw error string (useful for testing and provider errors).
pub fn classify_str(msg: &str) -> LlmErrorClass {
    // Order matters: more specific patterns first.
    // Billing before rate limit (402 vs 429).
    // Auth before transient (401/403 vs 502).

    if RE_BILLING.is_match(msg) {
        return LlmErrorClass::Billing;
    }
    if RE_AUTH.is_match(msg) {
        return LlmErrorClass::AuthPermanent;
    }
    if RE_CONTEXT_OVERFLOW.is_match(msg) {
        return LlmErrorClass::ContextOverflow;
    }
    if RE_SESSION_CORRUPTION.is_match(msg) {
        return LlmErrorClass::SessionCorruption;
    }
    if RE_RATE_LIMIT.is_match(msg) {
        return LlmErrorClass::RateLimit;
    }
    if RE_OVERLOADED.is_match(msg) {
        return LlmErrorClass::Overloaded;
    }
    if RE_TRANSIENT_HTTP.is_match(msg) {
        return LlmErrorClass::TransientHttp;
    }
    LlmErrorClass::Unknown
}

/// Recommended cooldown duration based on error class.
pub fn cooldown_duration(class: &LlmErrorClass) -> std::time::Duration {
    use std::time::Duration;
    match class {
        LlmErrorClass::AuthPermanent | LlmErrorClass::Billing => Duration::from_secs(3600),
        LlmErrorClass::RateLimit => Duration::from_secs(60),
        LlmErrorClass::Overloaded => Duration::from_secs(30),
        LlmErrorClass::TransientHttp | LlmErrorClass::Unknown => Duration::from_secs(15),
        LlmErrorClass::ContextOverflow | LlmErrorClass::SessionCorruption => Duration::ZERO,
    }
}

/// User-friendly message for each error class.
/// Language defaults to Russian when not specified.
pub fn user_message(class: &LlmErrorClass) -> &'static str {
    user_message_lang(class, "ru")
}

/// User-friendly message for each error class with explicit language.
pub fn user_message_lang(class: &LlmErrorClass, language: &str) -> &'static str {
    let e = super::localization::get_error_strings(language);
    match class {
        LlmErrorClass::ContextOverflow => e.context_overflow,
        LlmErrorClass::SessionCorruption => e.session_corruption,
        LlmErrorClass::TransientHttp => e.transient_http,
        LlmErrorClass::RateLimit => e.rate_limit,
        LlmErrorClass::AuthPermanent => e.auth_permanent,
        LlmErrorClass::Billing => e.billing,
        LlmErrorClass::Overloaded => e.overloaded,
        LlmErrorClass::Unknown => e.unknown,
    }
}

/// Format error for user display: classify + user message with warning emoji.
pub fn format_user_error(error: &anyhow::Error) -> String {
    format!("⚠️ {}", user_message(&classify(error)))
}

/// Whether the error class is worth retrying at the engine level.
pub fn is_retryable(class: &LlmErrorClass) -> bool {
    matches!(
        class,
        LlmErrorClass::TransientHttp | LlmErrorClass::Overloaded
    )
}

// ── ProviderErrorKind ─────────────────────────────────────────────────────────
// Failover-oriented classification: tells the routing layer what to do.

/// Error kind for provider failover decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// 500, 502, 503, timeout, overloaded — retry locally then failover.
    Transient,
    /// 429, rate limited — cooldown with Retry-After if available.
    RateLimit,
    /// 400, 404, context overflow, session corruption — don't retry or failover.
    Permanent,
    /// 401, 403, billing — alert user, don't retry.
    Auth,
}

/// Classify a provider error into a failover-oriented kind.
pub fn classify_provider_error(error: &anyhow::Error) -> ProviderErrorKind {
    let class = classify(error);
    provider_kind_from_class(&class)
}

/// Map LlmErrorClass to ProviderErrorKind.
pub fn provider_kind_from_class(class: &LlmErrorClass) -> ProviderErrorKind {
    match class {
        LlmErrorClass::TransientHttp | LlmErrorClass::Overloaded | LlmErrorClass::Unknown => {
            ProviderErrorKind::Transient
        }
        LlmErrorClass::RateLimit => ProviderErrorKind::RateLimit,
        LlmErrorClass::AuthPermanent | LlmErrorClass::Billing => ProviderErrorKind::Auth,
        LlmErrorClass::ContextOverflow | LlmErrorClass::SessionCorruption => {
            ProviderErrorKind::Permanent
        }
    }
}

/// Whether a ProviderErrorKind should trigger failover to the next provider.
pub fn should_failover(kind: &ProviderErrorKind) -> bool {
    matches!(kind, ProviderErrorKind::Transient | ProviderErrorKind::RateLimit)
}

/// Whether a ProviderErrorKind should be retried locally before failover.
pub fn should_retry_locally(kind: &ProviderErrorKind) -> bool {
    matches!(kind, ProviderErrorKind::Transient)
}

/// Parse the Retry-After header value (seconds or HTTP date).
/// Returns seconds to wait, or None if the header is absent or unparseable.
pub fn parse_retry_after(header_value: &str) -> Option<u64> {
    // Try as integer seconds first
    if let Ok(secs) = header_value.trim().parse::<u64>() {
        return Some(secs);
    }
    // Try as HTTP date (RFC 7231): e.g. "Mon, 04 Apr 2026 12:00:00 GMT"
    if let Ok(date) = chrono::DateTime::parse_from_rfc2822(header_value.trim()) {
        let now = chrono::Utc::now();
        let delta = date.signed_duration_since(now);
        if delta.num_seconds() > 0 {
            return Some(delta.num_seconds() as u64);
        }
        return Some(0);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_overflow_patterns() {
        assert_eq!(classify_str("context length exceeded for model"), LlmErrorClass::ContextOverflow);
        assert_eq!(classify_str("error: token limit reached"), LlmErrorClass::ContextOverflow);
        assert_eq!(classify_str("request_too_large: prompt is too long"), LlmErrorClass::ContextOverflow);
        assert_eq!(classify_str("input too long for this model"), LlmErrorClass::ContextOverflow);
        assert_eq!(classify_str("exceeds the model maximum context"), LlmErrorClass::ContextOverflow);
    }

    #[test]
    fn session_corruption_patterns() {
        assert_eq!(classify_str("tool_use_block must follow assistant"), LlmErrorClass::SessionCorruption);
        assert_eq!(classify_str("roles must alternate between user and assistant"), LlmErrorClass::SessionCorruption);
        assert_eq!(classify_str("function call turn comes immediately after another"), LlmErrorClass::SessionCorruption);
        assert_eq!(classify_str("incorrect role information in messages"), LlmErrorClass::SessionCorruption);
    }

    #[test]
    fn transient_http_patterns() {
        assert_eq!(classify_str("minimax API error 502: bad gateway"), LlmErrorClass::TransientHttp);
        assert_eq!(classify_str("error 503 service unavailable"), LlmErrorClass::TransientHttp);
        assert_eq!(classify_str("HTTP 504 gateway timeout"), LlmErrorClass::TransientHttp);
        assert_eq!(classify_str("error sending request: 521"), LlmErrorClass::TransientHttp);
        assert_eq!(classify_str("minimax API error 500: internal server error"), LlmErrorClass::TransientHttp);
        assert_eq!(classify_str("HTTP 500 Internal Server Error"), LlmErrorClass::TransientHttp);
    }

    #[test]
    fn rate_limit_patterns() {
        assert_eq!(classify_str("API error 429: rate limit exceeded"), LlmErrorClass::RateLimit);
        assert_eq!(classify_str("too many requests, please slow down"), LlmErrorClass::RateLimit);
        assert_eq!(classify_str("tokens per minute limit reached"), LlmErrorClass::RateLimit);
        assert_eq!(classify_str("resource exhausted: TPM quota"), LlmErrorClass::RateLimit);
    }

    #[test]
    fn auth_patterns() {
        assert_eq!(classify_str("401 unauthorized: invalid api key"), LlmErrorClass::AuthPermanent);
        assert_eq!(classify_str("api key revoked or expired"), LlmErrorClass::AuthPermanent);
        assert_eq!(classify_str("403 forbidden: authentication failed"), LlmErrorClass::AuthPermanent);
    }

    #[test]
    fn billing_patterns() {
        assert_eq!(classify_str("HTTP 402 payment required"), LlmErrorClass::Billing);
        assert_eq!(classify_str("insufficient credits on account"), LlmErrorClass::Billing);
        assert_eq!(classify_str("quota exceeded for this month"), LlmErrorClass::Billing);
    }

    #[test]
    fn overloaded_patterns() {
        assert_eq!(classify_str("overloaded_error: server at capacity"), LlmErrorClass::Overloaded);
        assert_eq!(classify_str("service unavailable due to high demand"), LlmErrorClass::Overloaded);
    }

    #[test]
    fn google_permission_denied_is_auth() {
        assert_eq!(
            classify_str("google API error: PERMISSION_DENIED: API key not valid"),
            LlmErrorClass::AuthPermanent
        );
    }

    #[test]
    fn google_resource_exhausted_is_rate_limit() {
        assert_eq!(
            classify_str("google API error: RESOURCE_EXHAUSTED: GenerateContent request rate limit"),
            LlmErrorClass::RateLimit
        );
    }

    #[test]
    fn unknown_fallback() {
        assert_eq!(classify_str("something random happened"), LlmErrorClass::Unknown);
        assert_eq!(classify_str(""), LlmErrorClass::Unknown);
    }

    #[test]
    fn retryable_check() {
        assert!(is_retryable(&LlmErrorClass::TransientHttp));
        assert!(is_retryable(&LlmErrorClass::Overloaded));
        assert!(!is_retryable(&LlmErrorClass::RateLimit));
        assert!(!is_retryable(&LlmErrorClass::AuthPermanent));
        assert!(!is_retryable(&LlmErrorClass::Unknown));
    }

    #[test]
    fn billing_before_rate_limit() {
        // 402 should be billing, not confused with other patterns
        assert_eq!(classify_str("402 payment required"), LlmErrorClass::Billing);
    }

    #[test]
    fn user_messages_not_empty() {
        let classes = [
            LlmErrorClass::ContextOverflow, LlmErrorClass::SessionCorruption,
            LlmErrorClass::TransientHttp, LlmErrorClass::RateLimit,
            LlmErrorClass::AuthPermanent, LlmErrorClass::Billing,
            LlmErrorClass::Overloaded, LlmErrorClass::Unknown,
        ];
        for class in &classes {
            assert!(!user_message(class).is_empty(), "empty message for {:?}", class);
        }
    }

    // ── ProviderErrorKind tests ─────────────────────────────────────────────

    #[test]
    fn provider_kind_transient_from_http_errors() {
        assert_eq!(provider_kind_from_class(&LlmErrorClass::TransientHttp), ProviderErrorKind::Transient);
        assert_eq!(provider_kind_from_class(&LlmErrorClass::Overloaded), ProviderErrorKind::Transient);
        assert_eq!(provider_kind_from_class(&LlmErrorClass::Unknown), ProviderErrorKind::Transient);
    }

    #[test]
    fn provider_kind_rate_limit() {
        assert_eq!(provider_kind_from_class(&LlmErrorClass::RateLimit), ProviderErrorKind::RateLimit);
    }

    #[test]
    fn provider_kind_permanent_from_context_and_session() {
        assert_eq!(provider_kind_from_class(&LlmErrorClass::ContextOverflow), ProviderErrorKind::Permanent);
        assert_eq!(provider_kind_from_class(&LlmErrorClass::SessionCorruption), ProviderErrorKind::Permanent);
    }

    #[test]
    fn provider_kind_auth_from_auth_and_billing() {
        assert_eq!(provider_kind_from_class(&LlmErrorClass::AuthPermanent), ProviderErrorKind::Auth);
        assert_eq!(provider_kind_from_class(&LlmErrorClass::Billing), ProviderErrorKind::Auth);
    }

    #[test]
    fn should_failover_transient_and_rate_limit() {
        assert!(should_failover(&ProviderErrorKind::Transient));
        assert!(should_failover(&ProviderErrorKind::RateLimit));
        assert!(!should_failover(&ProviderErrorKind::Permanent));
        assert!(!should_failover(&ProviderErrorKind::Auth));
    }

    #[test]
    fn should_retry_locally_only_transient() {
        assert!(should_retry_locally(&ProviderErrorKind::Transient));
        assert!(!should_retry_locally(&ProviderErrorKind::RateLimit));
        assert!(!should_retry_locally(&ProviderErrorKind::Permanent));
        assert!(!should_retry_locally(&ProviderErrorKind::Auth));
    }

    // ── parse_retry_after tests ─────────────────────────────────────────────

    #[test]
    fn parse_retry_after_integer_seconds() {
        assert_eq!(parse_retry_after("120"), Some(120));
        assert_eq!(parse_retry_after(" 60 "), Some(60));
        assert_eq!(parse_retry_after("0"), Some(0));
    }

    #[test]
    fn parse_retry_after_garbage_returns_none() {
        assert_eq!(parse_retry_after("not-a-number"), None);
        assert_eq!(parse_retry_after(""), None);
    }

    #[test]
    fn parse_retry_after_past_date_returns_zero() {
        // A date far in the past
        assert_eq!(parse_retry_after("Mon, 01 Jan 2024 00:00:00 GMT"), Some(0));
    }
}
