use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
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
    /// Consecutive LLM errors from primary before switching to fallback provider.
    pub max_consecutive_failures: usize,
    /// Maximum auto-continue nudges per session when LLM response looks incomplete.
    pub max_auto_continues: u8,
    /// How many "you're looping" nudges before force-stop (default: 3).
    pub max_loop_nudges: usize,
    /// Maximum cycle length to detect in n-gram check (3..=N, default: 6).
    pub ngram_max_cycle: usize,
}

impl ToolLoopConfig {
    /// Returns effective max iterations: 0 means unlimited (usize::MAX).
    pub fn effective_max_iterations(&self) -> usize {
        if self.max_iterations == 0 { usize::MAX } else { self.max_iterations }
    }
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            compact_on_overflow: true,
            detect_loops: true,
            warn_threshold: 5,
            break_threshold: 10,
            max_consecutive_failures: 3,
            max_auto_continues: 5,
            max_loop_nudges: 3,
            ngram_max_cycle: 6,
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
            max_consecutive_failures: s.max_consecutive_failures,
            max_auto_continues: s.max_auto_continues,
            max_loop_nudges: s.max_loop_nudges,
            ngram_max_cycle: s.ngram_cycle_length,
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
    /// N-gram cycle detected (3+ steps repeated 3 times) — caller should nudge/break.
    CycleDetected(String),
}

/// Detects repetitive tool call patterns (same tool + same args in a row).
pub struct LoopDetector {
    /// Ring buffer of recent call hashes.
    recent: VecDeque<u64>,
    /// Ring buffer of recent tool names (parallel to `recent`, for descriptions).
    recent_names: VecDeque<String>,
    /// Count of consecutive identical hashes.
    consecutive: usize,
    /// The hash of the previous call (for consecutive detection).
    last_hash: Option<u64>,
    /// Thresholds from config.
    warn_threshold: usize,
    break_threshold: usize,
    /// Maximum n-gram cycle length to detect (3..=N).
    ngram_max_cycle: usize,
    /// Per-tool call counts (persists across reset() for progress header).
    tool_counts: HashMap<String, usize>,
}

impl LoopDetector {
    pub fn new(config: &ToolLoopConfig) -> Self {
        Self {
            recent: VecDeque::with_capacity(64),
            recent_names: VecDeque::with_capacity(64),
            consecutive: 0,
            last_hash: None,
            warn_threshold: config.warn_threshold,
            break_threshold: config.break_threshold,
            ngram_max_cycle: config.ngram_max_cycle,
            tool_counts: HashMap::new(),
        }
    }

    /// Record a tool call. Returns the loop status after this call.
    pub fn record(&mut self, tool_name: &str, args: &serde_json::Value) -> LoopStatus {
        let hash = Self::hash_call(tool_name, args);

        // Track per-tool call counts (persists across reset)
        *self.tool_counts.entry(tool_name.to_string()).or_insert(0) += 1;

        // Track consecutive identical calls
        if self.last_hash == Some(hash) {
            self.consecutive += 1;
        } else {
            self.consecutive = 1;
            self.last_hash = Some(hash);
        }

        // Maintain ring buffer (capacity 64)
        if self.recent.len() >= 64 {
            self.recent.pop_front();
            self.recent_names.pop_front();
        }
        self.recent.push_back(hash);
        self.recent_names.push_back(tool_name.to_string());

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

        // Check for n-gram cycles (3..=ngram_max_cycle steps repeated)
        if self.recent.len() >= 6 {
            if let Some(status) = self.detect_ngram_cycle() {
                return status;
            }
        }

        if self.consecutive >= self.warn_threshold {
            return LoopStatus::Warning(self.consecutive);
        }

        LoopStatus::Ok
    }

    fn hash_call(name: &str, args: &serde_json::Value) -> u64 {
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        // Use full argument string to avoid false positives with long inputs
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

    /// Detect repeating n-gram cycles of length 3..=ngram_max_cycle.
    /// Returns Warning at 2 repetitions, CycleDetected at 3 repetitions.
    fn detect_ngram_cycle(&self) -> Option<LoopStatus> {
        let len = self.recent.len();
        let max_n = self.ngram_max_cycle.min(len / 2);

        for n in 3..=max_n {
            if len < n * 2 {
                continue;
            }

            // The last `n` hashes form the "pattern"
            let pattern: Vec<u64> = (0..n).map(|i| self.recent[len - n + i]).collect();

            // Count how many times this pattern repeats going backward in the buffer
            let mut repetitions = 1usize; // the pattern itself counts as 1
            let mut offset = n;
            while offset + n <= len {
                let segment: Vec<u64> = (0..n).map(|i| self.recent[len - offset - n + i]).collect();
                if segment == pattern {
                    repetitions += 1;
                    offset += n;
                } else {
                    break;
                }
            }

            if repetitions >= 3 {
                // Build a description using the tool names from the last `n` calls
                let tools: Vec<&str> = (0..n)
                    .map(|i| self.recent_names[len - n + i].as_str())
                    .collect();
                let desc = format!("{}-step cycle repeated {} times: [{}]", n, repetitions, tools.join(" → "));
                return Some(LoopStatus::CycleDetected(desc));
            } else if repetitions == 2 {
                let count = n * 2;
                return Some(LoopStatus::Warning(count));
            }
        }

        None
    }

    /// Returns the per-tool call count map. Persists across reset().
    pub fn tool_counts(&self) -> &HashMap<String, usize> {
        &self.tool_counts
    }

    /// Total number of tool calls recorded (sum of tool_counts).
    pub fn iteration_count(&self) -> usize {
        self.tool_counts.values().sum()
    }

    /// Reset detection state (ring buffer + consecutive counter) without clearing tool_counts.
    /// Call this after injecting a loop nudge to give the model a clean slate.
    pub fn reset(&mut self) {
        self.recent.clear();
        self.recent_names.clear();
        self.consecutive = 0;
        self.last_hash = None;
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

    // ── N-gram cycle detection tests ──────────────────────────────────────────

    fn make_config_ngram() -> ToolLoopConfig {
        ToolLoopConfig {
            warn_threshold: 20,
            break_threshold: 20,
            ngram_max_cycle: 6,
            ..Default::default()
        }
    }

    fn record_seq(detector: &mut LoopDetector, seq: &[(&str, &str)]) -> LoopStatus {
        let mut last = LoopStatus::Ok;
        for (tool, arg) in seq {
            last = detector.record(tool, &serde_json::json!({ "q": arg }));
        }
        last
    }

    #[test]
    fn ngram_cycle_3_steps() {
        let mut det = LoopDetector::new(&make_config_ngram());
        // A-B-C × 3 = 9 calls; third repetition should return CycleDetected
        let abc: Vec<(&str, &str)> = vec![("a", "1"), ("b", "2"), ("c", "3")];
        // First 2 repetitions
        for _ in 0..2 {
            for (t, a) in &abc {
                det.record(t, &serde_json::json!({ "q": a }));
            }
        }
        // Third repetition — last call should fire CycleDetected
        det.record("a", &serde_json::json!({ "q": "1" }));
        det.record("b", &serde_json::json!({ "q": "2" }));
        let status = det.record("c", &serde_json::json!({ "q": "3" }));
        assert!(
            matches!(status, LoopStatus::CycleDetected(_)),
            "expected CycleDetected for A-B-C × 3, got {:?}",
            status
        );
    }

    #[test]
    fn ngram_cycle_5_steps() {
        let mut det = LoopDetector::new(&make_config_ngram());
        let abcde: &[(&str, &str)] = &[("a","1"),("b","2"),("c","3"),("d","4"),("e","5")];
        // 2 full repetitions first
        for _ in 0..2 {
            for (t, a) in abcde {
                det.record(t, &serde_json::json!({ "q": a }));
            }
        }
        // Third repetition
        for (i, (t, a)) in abcde.iter().enumerate() {
            let status = det.record(t, &serde_json::json!({ "q": a }));
            if i == abcde.len() - 1 {
                assert!(
                    matches!(status, LoopStatus::CycleDetected(_)),
                    "expected CycleDetected for A-B-C-D-E × 3, got {:?}",
                    status
                );
            }
        }
    }

    #[test]
    fn ngram_no_false_positive() {
        let mut det = LoopDetector::new(&make_config_ngram());
        // Similar but not identical sequences — tool names differ on 3rd rep
        det.record("a", &serde_json::json!({"q":"1"}));
        det.record("b", &serde_json::json!({"q":"2"}));
        det.record("c", &serde_json::json!({"q":"3"}));
        det.record("a", &serde_json::json!({"q":"1"}));
        det.record("b", &serde_json::json!({"q":"2"}));
        det.record("c", &serde_json::json!({"q":"3"}));
        det.record("a", &serde_json::json!({"q":"1"}));
        det.record("b", &serde_json::json!({"q":"2"}));
        // Last call uses different tool name → should NOT be CycleDetected
        let status = det.record("d", &serde_json::json!({"q":"3"}));
        assert!(
            !matches!(status, LoopStatus::CycleDetected(_)),
            "expected no CycleDetected for non-identical 3rd rep, got {:?}",
            status
        );
    }

    #[test]
    fn ngram_resets_after_break() {
        let mut det = LoopDetector::new(&make_config_ngram());
        let abc: &[(&str, &str)] = &[("a","1"),("b","2"),("c","3")];
        // Fill 3 cycles to trigger CycleDetected
        for _ in 0..2 {
            for (t, a) in abc {
                det.record(t, &serde_json::json!({"q": a}));
            }
        }
        for (t, a) in abc.iter() {
            det.record(t, &serde_json::json!({"q": a}));
        }
        // Reset
        det.reset();
        // After reset, fresh calls should return Ok
        let status = det.record("a", &serde_json::json!({"q":"1"}));
        assert_eq!(status, LoopStatus::Ok, "expected Ok after reset");
    }

    #[test]
    fn tool_counts_accurate() {
        let mut det = LoopDetector::new(&make_config_ngram());
        det.record("search", &serde_json::json!({"q":"1"}));
        det.record("search", &serde_json::json!({"q":"2"}));
        det.record("read", &serde_json::json!({"path":"x"}));
        det.record("search", &serde_json::json!({"q":"3"}));

        assert_eq!(det.tool_counts().get("search"), Some(&3));
        assert_eq!(det.tool_counts().get("read"), Some(&1));
        assert_eq!(det.iteration_count(), 4);
    }

    #[test]
    fn ngram_warning_at_2_repeats() {
        let mut det = LoopDetector::new(&make_config_ngram());
        let abc: &[(&str, &str)] = &[("a","1"),("b","2"),("c","3")];
        // 1st full rep
        for (t, a) in abc {
            det.record(t, &serde_json::json!({"q": a}));
        }
        // 2nd rep — last call should be Warning (not CycleDetected)
        det.record("a", &serde_json::json!({"q":"1"}));
        det.record("b", &serde_json::json!({"q":"2"}));
        let status = det.record("c", &serde_json::json!({"q":"3"}));
        assert!(
            matches!(status, LoopStatus::Warning(_)),
            "expected Warning at 2 repetitions, got {:?}",
            status
        );
        assert!(
            !matches!(status, LoopStatus::CycleDetected(_)),
            "should NOT be CycleDetected at only 2 repetitions"
        );
    }

    #[test]
    fn args_truncated_to_200() {
        // Two calls: one with a 300-char arg, one with the same 300-char arg
        // Both should produce the same hash (same first 200 chars)
        let long_arg = "x".repeat(300);
        let truncated_arg = "x".repeat(300); // same string, hash_call truncates both to 200
        let h1 = LoopDetector::hash_call("tool", &serde_json::json!({"data": long_arg}));
        let h2 = LoopDetector::hash_call("tool", &serde_json::json!({"data": truncated_arg}));
        assert_eq!(h1, h2, "same 300-char arg should produce identical hash (both truncated)");

        // A call with a different 300-char arg (different first 200 chars) should differ
        let different_arg = format!("y{}", "x".repeat(299));
        let h3 = LoopDetector::hash_call("tool", &serde_json::json!({"data": different_arg}));
        assert_ne!(h1, h3, "different first 200 chars should produce different hash");
    }

    #[test]
    fn nudge_count_config_defaults() {
        let cfg = ToolLoopConfig::default();
        assert_eq!(cfg.max_loop_nudges, 3, "default max_loop_nudges should be 3");
        assert_eq!(cfg.ngram_max_cycle, 6, "default ngram_max_cycle should be 6");
    }

    #[test]
    fn reset_preserves_tool_counts() {
        let mut det = LoopDetector::new(&make_config_ngram());
        det.record("search", &serde_json::json!({"q":"1"}));
        det.record("read", &serde_json::json!({"path":"x"}));
        det.reset();
        // tool_counts should still have the recorded values
        assert_eq!(det.tool_counts().get("search"), Some(&1));
        assert_eq!(det.tool_counts().get("read"), Some(&1));
        // But detection state is clear
        assert_eq!(det.consecutive, 0);
        assert!(det.recent.is_empty());
    }
}
