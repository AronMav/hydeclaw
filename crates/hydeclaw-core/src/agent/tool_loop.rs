use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

/// Configuration for the tool execution loop.
#[derive(Debug, Clone)]
pub struct ToolLoopConfig {
    pub max_iterations: usize,
    pub compact_on_overflow: bool,
    pub detect_loops: bool,
    pub warn_threshold: usize,
    pub break_threshold: usize,
    pub max_consecutive_failures: usize,
    pub max_auto_continues: u8,
    pub max_loop_nudges: usize,
    pub ngram_max_cycle: usize,
    pub error_break_threshold: usize,
}

impl ToolLoopConfig {
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
            error_break_threshold: 3,
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
            error_break_threshold: s.error_break_threshold.unwrap_or(3),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum LoopStatus {
    Ok,
    Warning(usize),
    Break(String),
    CycleDetected(String),
}

/// Detects repetitive tool call patterns with two-phase checking.
pub struct LoopDetector {
    recent: VecDeque<u64>,
    recent_names: VecDeque<String>,
    consecutive: usize,
    last_hash: Option<u64>,
    warn_threshold: usize,
    break_threshold: usize,
    ngram_max_cycle: usize,
    tool_counts: HashMap<String, usize>,
    consecutive_errors: usize,
    last_error_tool: Option<String>,
    error_break_threshold: usize,
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
            consecutive_errors: 0,
            last_error_tool: None,
            error_break_threshold: config.error_break_threshold,
        }
    }

    /// PHASE 1: Check if this call WOULD trigger a loop break. Call BEFORE execution.
    pub fn check_limits(&self, tool_name: &str, args: &serde_json::Value) -> LoopStatus {
        let hash = Self::hash_call(tool_name, args);

        if self.last_hash == Some(hash) && self.consecutive + 1 >= self.break_threshold {
            return LoopStatus::Break(format!(
                "tool '{}' called {} times consecutively with identical arguments",
                tool_name, self.consecutive + 1
            ));
        }

        if self.recent.len() >= 7 {
            let mut temp_recent = self.recent.clone();
            temp_recent.push_back(hash);
            if let Some(reason) = self.simulate_ping_pong(&temp_recent) {
                return LoopStatus::Break(reason);
            }
        }

        LoopStatus::Ok
    }

    /// PHASE 2: Record actual execution and result. Call AFTER getting tool result.
    pub fn record_execution(&mut self, tool_name: &str, args: &serde_json::Value, success: bool) -> LoopStatus {
        let hash = Self::hash_call(tool_name, args);
        *self.tool_counts.entry(tool_name.to_string()).or_insert(0) += 1;

        if self.last_hash == Some(hash) {
            self.consecutive += 1;
        } else {
            self.consecutive = 1;
            self.last_hash = Some(hash);
        }

        if self.recent.len() >= 64 {
            self.recent.pop_front();
            self.recent_names.pop_front();
        }
        self.recent.push_back(hash);
        self.recent_names.push_back(tool_name.to_string());

        // Handle error tracking
        if !success {
            if self.last_error_tool.as_deref() == Some(tool_name) {
                self.consecutive_errors += 1;
            } else {
                self.consecutive_errors = 1;
                self.last_error_tool = Some(tool_name.to_string());
            }
            if self.consecutive_errors >= self.error_break_threshold {
                return LoopStatus::Break(format!(
                    "tool '{}' failed {} times consecutively — stopping to avoid infinite error loop",
                    tool_name, self.consecutive_errors
                ));
            }
        } else {
            self.consecutive_errors = 0;
            self.last_error_tool = None;
        }

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

    /// Warm up the detector with a known hash from history (e.g. WAL).
    pub fn warm_up(&mut self, hash: u64, name: &str) {
        *self.tool_counts.entry(name.to_string()).or_insert(0) += 1;
        if self.last_hash == Some(hash) {
            self.consecutive += 1;
        } else {
            self.consecutive = 1;
            self.last_hash = Some(hash);
        }
        if self.recent.len() >= 64 {
            self.recent.pop_front();
            self.recent_names.pop_front();
        }
        self.recent.push_back(hash);
        self.recent_names.push_back(name.to_string());
    }

    pub fn hash_call_raw(name: &str, args: &serde_json::Value) -> u64 {
        Self::hash_call(name, args)
    }

    fn hash_call(name: &str, args: &serde_json::Value) -> u64 {
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        args.to_string().hash(&mut hasher);
        hasher.finish()
    }

    fn simulate_ping_pong(&self, recent: &VecDeque<u64>) -> Option<String> {
        let len = recent.len();
        if len < 8 { return None; }
        let a = *recent.get(len - 1)?;
        let b = *recent.get(len - 2)?;
        if a == b { return None; }
        let is_ping_pong = (0..4).all(|i| {
            recent.get(len - 1 - 2 * i) == Some(&a) && 
            recent.get(len - 2 - 2 * i) == Some(&b)
        });
        if is_ping_pong {
            Some("ping-pong pattern detected (simulated)".into())
        } else {
            None
        }
    }

    fn detect_ngram_cycle(&self) -> Option<LoopStatus> {
        let len = self.recent.len();
        let max_n = self.ngram_max_cycle.min(len / 2);
        for n in 3..=max_n {
            if len < n * 2 { continue; }
            let pattern: Vec<u64> = (0..n).filter_map(|i| self.recent.get(len - n + i)).cloned().collect();
            if pattern.len() < n { continue; }
            let mut repetitions = 1usize;
            let mut offset = n;
            while offset + n <= len {
                let segment: Vec<u64> = (0..n).filter_map(|i| self.recent.get(len - offset - n + i)).cloned().collect();
                if segment == pattern {
                    repetitions += 1;
                    offset += n;
                } else { break; }
            }
            if repetitions >= 3 {
                let tools: Vec<&str> = (0..n).filter_map(|i| self.recent_names.get(len - n + i).map(|s| s.as_str())).collect();
                return Some(LoopStatus::CycleDetected(format!("{}-step cycle repeated {} times: [{}]", n, repetitions, tools.join(" -> "))));
            } else if repetitions == 2 {
                return Some(LoopStatus::Warning(n * 2));
            }
        }
        None
    }

    pub fn tool_counts(&self) -> &HashMap<String, usize> { &self.tool_counts }
    pub fn iteration_count(&self) -> usize { self.tool_counts.values().sum() }
    pub fn reset(&mut self) {
        self.recent.clear();
        self.recent_names.clear();
        self.consecutive = 0;
        self.last_hash = None;
        self.consecutive_errors = 0;
        self.last_error_tool = None;
    }
}

pub fn is_context_overflow(error: &anyhow::Error) -> bool {
    let msg = error.to_string().to_lowercase();
    msg.contains("context length") || msg.contains("token limit") || msg.contains("too many token") || msg.contains("input too long") || msg.contains("400") && (msg.contains("too long") || msg.contains("too large"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_two_phase() {
        let cfg = ToolLoopConfig::default();
        let mut det = LoopDetector::new(&cfg);
        let args = serde_json::json!({"q": "1"});
        assert_eq!(det.check_limits("t1", &args), LoopStatus::Ok);
        assert_eq!(det.record_execution("t1", &args, true), LoopStatus::Ok);
    }
}
