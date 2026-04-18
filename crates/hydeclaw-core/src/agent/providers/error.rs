use std::sync::Arc;
use thiserror::Error;

/// The reason a `cancellable_stream` was terminated. Written to a
/// `CancelSlot` before `CancellationToken::cancel()` fires, so readers
/// that wake on the token always see a populated reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    ConnectTimeout { elapsed_secs: u64 },
    InactivityTimeout { silent_secs: u64 },
    MaxDurationExceeded { elapsed_secs: u64 },
    UserCancelled,
    ShutdownDrain,
}

/// Single typed error enum every LLM provider returns.
///
/// Every variant that corresponds to a cancellation carries
/// `partial_text` so the engine can persist work already produced;
/// see spec §5.
#[derive(Debug, Clone, Error)]
pub enum LlmCallError {
    #[error("{provider}: connect timed out after {elapsed_secs}s")]
    ConnectTimeout { provider: String, elapsed_secs: u64 },

    #[error("{provider}: provider stopped sending data for {silent_secs}s")]
    InactivityTimeout {
        provider: String,
        silent_secs: u64,
        partial_text: String,
    },

    #[error("{provider}: request timed out after {elapsed_secs}s")]
    RequestTimeout { provider: String, elapsed_secs: u64 },

    #[error("{provider}: stream exceeded max duration {elapsed_secs}s")]
    MaxDurationExceeded {
        provider: String,
        elapsed_secs: u64,
        partial_text: String,
    },

    #[error("stopped by user")]
    UserCancelled { partial_text: String },

    #[error("interrupted by shutdown drain")]
    ShutdownDrain { partial_text: String },

    #[error("{provider}: schema error at byte {at_bytes}: {detail}")]
    SchemaError {
        provider: String,
        detail: String,
        /// Offset into the response body where the error was detected.
        /// `0` means "request rejected before any bytes streamed" → failover.
        /// Non-zero means "error mid-stream" → no failover (partial content
        /// already delivered to the user).
        at_bytes: u64,
    },

    #[error("{provider}: auth failed with status {status}")]
    AuthError { provider: String, status: u16 },

    #[error("{provider}: server returned {status}")]
    Server5xx { provider: String, status: u16 },

    // `reqwest::Error` is not `Clone`, so wrap in `Arc` to keep the
    // `LlmCallError: Clone` contract required by downstream consumers
    // (e.g. error broadcast to multiple tasks). Manual `From` impl below
    // since `#[from]` on an `Arc<T>`-wrapped field is not supported.
    #[error("network error: {0}")]
    Network(Arc<reqwest::Error>),
}

impl From<reqwest::Error> for LlmCallError {
    fn from(err: reqwest::Error) -> Self {
        LlmCallError::Network(Arc::new(err))
    }
}

impl LlmCallError {
    /// True when `RoutingProvider` should attempt the next route.
    pub fn is_failover_worthy(&self) -> bool {
        use LlmCallError::*;
        match self {
            ConnectTimeout { .. }
            | InactivityTimeout { .. }
            | RequestTimeout { .. }
            | Network(_)
            | Server5xx { .. } => true,

            MaxDurationExceeded { .. }
            | UserCancelled { .. }
            | ShutdownDrain { .. }
            | AuthError { .. } => false,

            SchemaError { at_bytes, .. } => *at_bytes == 0,
        }
    }

    /// Returns the preserved partial text if this variant carries any.
    pub fn partial_text(&self) -> Option<&str> {
        use LlmCallError::*;
        match self {
            InactivityTimeout { partial_text, .. }
            | MaxDurationExceeded { partial_text, .. }
            | UserCancelled { partial_text }
            | ShutdownDrain { partial_text } => Some(partial_text.as_str()),
            _ => None,
        }
    }

    /// Stable short identifier persisted to `messages.abort_reason`.
    /// Changing these strings breaks historical rows.
    pub fn abort_reason(&self) -> Option<&'static str> {
        use LlmCallError::*;
        Some(match self {
            ConnectTimeout { .. } => "connect_timeout",
            InactivityTimeout { .. } => "inactivity",
            RequestTimeout { .. } => "request_timeout",
            MaxDurationExceeded { .. } => "max_duration",
            UserCancelled { .. } => "user_cancelled",
            ShutdownDrain { .. } => "shutdown_drain",
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_failover_worthy_connect_timeout() {
        let e = LlmCallError::ConnectTimeout { provider: "p".into(), elapsed_secs: 10 };
        assert!(e.is_failover_worthy());
    }

    #[test]
    fn is_failover_worthy_inactivity_timeout() {
        let e = LlmCallError::InactivityTimeout {
            provider: "p".into(),
            silent_secs: 60,
            partial_text: String::new(),
        };
        assert!(e.is_failover_worthy());
    }

    #[test]
    fn is_failover_worthy_request_timeout() {
        let e = LlmCallError::RequestTimeout { provider: "p".into(), elapsed_secs: 120 };
        assert!(e.is_failover_worthy());
    }

    #[test]
    fn is_failover_worthy_server_5xx() {
        let e = LlmCallError::Server5xx { provider: "p".into(), status: 503 };
        assert!(e.is_failover_worthy());
    }

    #[test]
    fn not_failover_worthy_max_duration() {
        let e = LlmCallError::MaxDurationExceeded {
            provider: "p".into(),
            elapsed_secs: 600,
            partial_text: String::new(),
        };
        assert!(!e.is_failover_worthy());
    }

    #[test]
    fn not_failover_worthy_user_cancelled() {
        let e = LlmCallError::UserCancelled { partial_text: String::new() };
        assert!(!e.is_failover_worthy());
    }

    #[test]
    fn not_failover_worthy_shutdown_drain() {
        let e = LlmCallError::ShutdownDrain { partial_text: String::new() };
        assert!(!e.is_failover_worthy());
    }

    #[test]
    fn not_failover_worthy_auth_error() {
        let e = LlmCallError::AuthError { provider: "p".into(), status: 401 };
        assert!(!e.is_failover_worthy());
    }

    #[test]
    fn schema_error_failover_depends_on_at_bytes() {
        let pre = LlmCallError::SchemaError {
            provider: "p".into(),
            detail: "bad".into(),
            at_bytes: 0,
        };
        assert!(pre.is_failover_worthy(), "pre-stream schema error MUST fail over");

        let mid = LlmCallError::SchemaError {
            provider: "p".into(),
            detail: "bad".into(),
            at_bytes: 1024,
        };
        assert!(!mid.is_failover_worthy(), "mid-stream schema error MUST NOT fail over");
    }

    #[test]
    fn variants_carrying_partial_text_can_return_it() {
        let e = LlmCallError::UserCancelled { partial_text: "hello".into() };
        assert_eq!(e.partial_text(), Some("hello"));

        let e2 = LlmCallError::ConnectTimeout { provider: "p".into(), elapsed_secs: 5 };
        assert_eq!(e2.partial_text(), None);
    }

    #[test]
    fn abort_reason_strings_are_stable() {
        // Used as the persisted `messages.abort_reason` column — changing
        // these strings breaks historical rows. Pin them here.
        use LlmCallError::*;
        assert_eq!(ConnectTimeout { provider: "p".into(), elapsed_secs: 1 }.abort_reason(), Some("connect_timeout"));
        assert_eq!(InactivityTimeout { provider: "p".into(), silent_secs: 1, partial_text: "".into() }.abort_reason(), Some("inactivity"));
        assert_eq!(RequestTimeout { provider: "p".into(), elapsed_secs: 1 }.abort_reason(), Some("request_timeout"));
        assert_eq!(MaxDurationExceeded { provider: "p".into(), elapsed_secs: 1, partial_text: "".into() }.abort_reason(), Some("max_duration"));
        assert_eq!(UserCancelled { partial_text: "".into() }.abort_reason(), Some("user_cancelled"));
        assert_eq!(ShutdownDrain { partial_text: "".into() }.abort_reason(), Some("shutdown_drain"));
    }
}
