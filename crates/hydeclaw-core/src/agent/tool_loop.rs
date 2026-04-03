use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

/// Configuration for the tool execution loop.
/// Built from per-agent TOML settings with sensible defaults.
#[derive(Debug, Clone)]
pub struct ToolLoopConfig {
    /// Maximum number of LLM ↔ tool iterations before forcing a final response.
    pub max_iterations: usize,
    /// Whether to attempt mid-loop compaction on context overflow (400 errors).
    pub compact_on_overflow: bool,
    /// Whether loop detection is enabled.
    pub detect_loops: bool,
    /// Number of consecutive identical tool calls before logging a warning.
    pub warn_threshold: usize,
    /// Number of consecutive identical tool calls before forcibly breaking the loop.
    pub break_threshold: usize,
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            compact_on_overflow: true,
            detect_loops: true,
            warn_threshold: 5,
            break_threshold: 10,
        }
    }
}

impl From<&crate::config::ToolLoopSettings> for ToolLoopConfig {
    fn from(s: &crate::config::ToolLoopSettings) -> Self {
        Self {
            max_iterations: s.max_iterations,
            compact_on_overflow: s.compact_on_overflow,
            detect_loops: s.detect_loops,
            warn_threshold: s.warn_threshold,
            break_threshold: s.break_threshold,
        }
    }
}

/// Result of recording a tool call in the loop detector.
#[derive(Debug, PartialEq)]
pub enum LoopStatus {
    /// No loop detected.
    Ok,
    /// Consecutive identical calls detected — warning only.
    Warning(usize),
    /// Threshold exceeded — caller should break the loop.
    Break(String),
}

/// Detects repetitive tool call patterns (same tool + same args in a row).
pub struct LoopDetector {
    /// Ring buffer of recent call hashes.
    recent: VecDeque<u64>,
    /// Count of consecutive identical hashes.
    consecutive: usize,
    /// The hash of the previous call (for consecutive detection).
    last_hash: Option<u64>,
    /// Thresholds from config.
    warn_threshold: usize,
    break_threshold: usize,
}

impl LoopDetector {
    pub fn new(config: &ToolLoopConfig) -> Self {
        Self {
            recent: VecDeque::with_capacity(32),
            consecutive: 0,
            last_hash: None,
            warn_threshold: config.warn_threshold,
            break_threshold: config.break_threshold,
        }
    }

    /// Record a tool call. Returns the loop status after this call.
    pub fn record(&mut self, tool_name: &str, args: &serde_json::Value) -> LoopStatus {
        let hash = Self::hash_call(tool_name, args);

        // Track consecutive identical calls
        if self.last_hash == Some(hash) {
            self.consecutive += 1;
        } else {
            self.consecutive = 1;
            self.last_hash = Some(hash);
        }

        // Maintain ring buffer for future ping-pong detection
        if self.recent.len() >= 32 {
            self.recent.pop_front();
        }
        self.recent.push_back(hash);

        if self.consecutive >= self.break_threshold {
            return LoopStatus::Break(format!(
                "tool '{}' called {} times consecutively with identical arguments",
                tool_name, self.consecutive
            ));
        }

        // Check for ping-pong pattern (A-B-A-B repeating)
        if self.recent.len() >= 8
            && let Some(reason) = self.detect_ping_pong() {
                return LoopStatus::Break(reason);
            }

        if self.consecutive >= self.warn_threshold {
            return LoopStatus::Warning(self.consecutive);
        }

        LoopStatus::Ok
    }

    fn hash_call(name: &str, args: &serde_json::Value) -> u64 {
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        // Stable JSON string for hashing (serde_json serialization is deterministic for same Value)
        let args_str = args.to_string();
        args_str.hash(&mut hasher);
        hasher.finish()
    }

    /// Detect A-B-A-B ping-pong pattern in the last 8 calls.
    fn detect_ping_pong(&self) -> Option<String> {
        let len = self.recent.len();
        if len < 8 {
            return None;
        }

        // Check if last 8 calls form a repeating 2-element pattern
        let a = self.recent[len - 1];
        let b = self.recent[len - 2];
        if a == b {
            return None; // Same call, not ping-pong
        }

        let is_ping_pong = (0..4).all(|i| {
            self.recent[len - 1 - 2 * i] == a && self.recent[len - 2 - 2 * i] == b
        });

        if is_ping_pong {
            Some("ping-pong pattern detected: two tools alternating with identical arguments".into())
        } else {
            None
        }
    }
}

/// Check if an error message indicates a context window overflow.
/// LLM providers return 400 errors with various phrasings when the context is too long.
pub fn is_context_overflow(error: &anyhow::Error) -> bool {
    let msg = error.to_string().to_lowercase();
    // Common patterns from various LLM providers:
    // - "context length exceeded" (OpenAI)
    // - "maximum context length" (OpenAI)
    // - "token limit" / "token budget" (generic)
    // - "too many tokens" (generic)
    // - "input too long" (MiniMax, Anthropic)
    // - "prompt is too long" (various)
    // - "exceeds the model's maximum" (OpenAI)
    msg.contains("context length")
        || msg.contains("context_length")
        || msg.contains("token limit")
        || msg.contains("too many token")
        || msg.contains("input too long")
        || msg.contains("prompt is too long")
        || msg.contains("maximum context")
        || msg.contains("exceeds the model")
        || msg.contains("input_tokens_exceeded")
        || msg.contains("sequence_length_exceeded")
        || msg.contains("prompt_too_long")
        || msg.contains("payload exceeded")
        || msg.contains("request too large")
        || (msg.contains("400") && (msg.contains("too long") || msg.contains("too large")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_detector_no_loop() {
        let config = ToolLoopConfig { warn_threshold: 3, break_threshold: 5, ..Default::default() };
        let mut detector = LoopDetector::new(&config);

        assert_eq!(detector.record("search", &serde_json::json!({"q": "a"})), LoopStatus::Ok);
        assert_eq!(detector.record("search", &serde_json::json!({"q": "b"})), LoopStatus::Ok);
        assert_eq!(detector.record("read", &serde_json::json!({"path": "x"})), LoopStatus::Ok);
    }

    #[test]
    fn loop_detector_warns_then_breaks() {
        let config = ToolLoopConfig { warn_threshold: 3, break_threshold: 5, ..Default::default() };
        let mut detector = LoopDetector::new(&config);

        let args = serde_json::json!({"q": "same"});
        assert_eq!(detector.record("search", &args), LoopStatus::Ok); // 1
        assert_eq!(detector.record("search", &args), LoopStatus::Ok); // 2
        assert!(matches!(detector.record("search", &args), LoopStatus::Warning(3))); // 3
        assert!(matches!(detector.record("search", &args), LoopStatus::Warning(4))); // 4
        assert!(matches!(detector.record("search", &args), LoopStatus::Break(_))); // 5
    }

    #[test]
    fn loop_detector_resets_on_different_call() {
        let config = ToolLoopConfig { warn_threshold: 3, break_threshold: 5, ..Default::default() };
        let mut detector = LoopDetector::new(&config);

        let args = serde_json::json!({"q": "same"});
        assert_eq!(detector.record("search", &args), LoopStatus::Ok);
        assert_eq!(detector.record("search", &args), LoopStatus::Ok);
        // Different call resets
        assert_eq!(detector.record("read", &serde_json::json!({"path": "x"})), LoopStatus::Ok);
        assert_eq!(detector.record("search", &args), LoopStatus::Ok); // 1 again
        assert_eq!(detector.record("search", &args), LoopStatus::Ok); // 2
    }

    #[test]
    fn context_overflow_detection() {
        assert!(is_context_overflow(&anyhow::anyhow!("MiniMax API error 400: input too long")));
        assert!(is_context_overflow(&anyhow::anyhow!("context length exceeded")));
        assert!(is_context_overflow(&anyhow::anyhow!("This model's maximum context length is 128000 tokens")));
        assert!(!is_context_overflow(&anyhow::anyhow!("MiniMax API error 400: invalid format")));
        assert!(!is_context_overflow(&anyhow::anyhow!("network error")));
    }

    #[test]
    fn test_ping_pong_detection() {
        // Set consecutive thresholds high so they won't trigger — only ping-pong should fire.
        let config = ToolLoopConfig {
            warn_threshold: 20,
            break_threshold: 20,
            ..Default::default()
        };
        let mut detector = LoopDetector::new(&config);

        let args_a = serde_json::json!({"q": "alpha"});
        let args_b = serde_json::json!({"q": "beta"});

        // Alternate A-B for 8 calls (4 pairs). Ping-pong triggers at recent.len() >= 8.
        // Calls 1..7 should be Ok.
        for _ in 0..3 {
            assert_eq!(detector.record("tool_a", &args_a), LoopStatus::Ok);
            assert_eq!(detector.record("tool_b", &args_b), LoopStatus::Ok);
        }
        // 7th call (A) — still only 7 in buffer
        assert_eq!(detector.record("tool_a", &args_a), LoopStatus::Ok);
        // 8th call (B) — now 8 entries, ping-pong check fires
        let status = detector.record("tool_b", &args_b);
        assert!(
            matches!(status, LoopStatus::Break(ref reason) if reason.contains("ping-pong")),
            "expected Break with ping-pong reason, got {:?}",
            status,
        );
    }

    #[test]
    fn test_default_config() {
        let cfg = ToolLoopConfig::default();
        assert_eq!(cfg.max_iterations, 50);
        assert!(cfg.compact_on_overflow);
        assert!(cfg.detect_loops);
        assert_eq!(cfg.warn_threshold, 5);
        assert_eq!(cfg.break_threshold, 10);
    }

    #[test]
    fn test_context_overflow_400_too_long() {
        // Boundary: message containing both "400" and "too long" should be detected.
        assert!(is_context_overflow(&anyhow::anyhow!(
            "HTTP 400 Bad Request: the request payload is too long"
        )));
        // Only "400" without "too long" should NOT match.
        assert!(!is_context_overflow(&anyhow::anyhow!(
            "HTTP 400 Bad Request: malformed JSON"
        )));
        // Only "too long" without "400" should NOT match (unless another rule matches).
        assert!(!is_context_overflow(&anyhow::anyhow!(
            "the request payload is too long"
        )));
    }

    #[test]
    fn test_different_args_no_warning() {
        let config = ToolLoopConfig {
            warn_threshold: 3,
            break_threshold: 5,
            ..Default::default()
        };
        let mut detector = LoopDetector::new(&config);

        // Same tool name, but each call has different args — should never warn.
        for i in 0..6 {
            let args = serde_json::json!({"q": format!("query_{}", i)});
            assert_eq!(
                detector.record("search", &args),
                LoopStatus::Ok,
                "call {} with unique args should be Ok",
                i,
            );
        }
    }

    #[test]
    fn loop_detector_ping_pong_detection() {
        // Use default thresholds (warn=5, break=10) so consecutive detection doesn't trigger.
        let config = ToolLoopConfig::default();
        let mut detector = LoopDetector::new(&config);

        let args_search = serde_json::json!({"q": "a"});
        let args_read = serde_json::json!({"path": "x"});

        // 8 alternating calls: search, read, search, read, ... (4 pairs)
        for i in 0..3 {
            assert_eq!(
                detector.record("search", &args_search),
                LoopStatus::Ok,
                "search call {} should be Ok",
                i * 2 + 1,
            );
            assert_eq!(
                detector.record("read", &args_read),
                LoopStatus::Ok,
                "read call {} should be Ok",
                i * 2 + 2,
            );
        }
        // 7th call (search)
        assert_eq!(detector.record("search", &args_search), LoopStatus::Ok);
        // 8th call (read) — triggers ping-pong detection
        let status = detector.record("read", &args_read);
        assert!(
            matches!(status, LoopStatus::Break(ref reason) if reason.contains("ping-pong")),
            "expected Break with ping-pong message, got {:?}",
            status,
        );
    }

    #[test]
    fn loop_detector_ping_pong_not_triggered_with_less_than_8() {
        let config = ToolLoopConfig::default();
        let mut detector = LoopDetector::new(&config);

        let args_search = serde_json::json!({"q": "a"});
        let args_read = serde_json::json!({"path": "x"});

        // Only 6 alternating calls (3 pairs) — not enough for ping-pong detection
        for _ in 0..3 {
            assert_eq!(detector.record("search", &args_search), LoopStatus::Ok);
            assert_eq!(detector.record("read", &args_read), LoopStatus::Ok);
        }
        // All 6 calls should have returned Ok (no ping-pong triggered)
    }

    #[test]
    fn default_config_values() {
        let cfg = ToolLoopConfig::default();
        assert_eq!(cfg.max_iterations, 50);
        assert!(cfg.compact_on_overflow);
        assert!(cfg.detect_loops);
        assert_eq!(cfg.warn_threshold, 5);
        assert_eq!(cfg.break_threshold, 10);
    }

    #[test]
    fn loop_detector_same_tool_different_args_no_loop() {
        let config = ToolLoopConfig {
            warn_threshold: 3,
            break_threshold: 5,
            ..Default::default()
        };
        let mut detector = LoopDetector::new(&config);

        // Call the same tool 20 times, but with different args each time — always Ok
        for i in 0..20 {
            let args = serde_json::json!({"q": format!("unique_query_{}", i)});
            assert_eq!(
                detector.record("search", &args),
                LoopStatus::Ok,
                "call {} with unique args should be Ok",
                i,
            );
        }
    }

    #[test]
    fn test_context_overflow_new_patterns() {
        let cases = vec![
            "input_tokens_exceeded: limit is 128000",
            "sequence_length_exceeded for this model",
            "Error: prompt_too_long",
            "payload exceeded size limit",
            "request too large for model context",
            "400 Bad Request: input too large",
        ];
        for msg in cases {
            assert!(
                is_context_overflow(&anyhow::anyhow!("{}", msg)),
                "should detect overflow in: {}",
                msg,
            );
        }
    }

    #[test]
    fn test_context_overflow_negative_cases() {
        let cases = vec![
            "connection refused",
            "unauthorized: bad API key",
            "rate limit exceeded",
        ];
        for msg in cases {
            assert!(
                !is_context_overflow(&anyhow::anyhow!("{}", msg)),
                "should NOT detect overflow in: {}",
                msg,
            );
        }
    }
}
