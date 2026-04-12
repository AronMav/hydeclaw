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
    pub break_threshold: usize,
    pub max_consecutive_failures: usize,
    pub max_auto_continues: u8,
    pub max_loop_nudges: usize,
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
            break_threshold: 10,
            max_consecutive_failures: 3,
            max_auto_continues: 5,
            max_loop_nudges: 3,
            error_break_threshold: 3,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum LoopStatus {
    Ok,
    Break(String),
}

/// Detects repetitive tool call patterns with two-phase checking.
pub struct LoopDetector {
    recent: VecDeque<u64>,
    recent_names: VecDeque<String>,
    consecutive: usize,
    last_hash: Option<u64>,
    break_threshold: usize,
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
            break_threshold: config.break_threshold,
            tool_counts: HashMap::new(),
            consecutive_errors: 0,
            last_error_tool: None,
            error_break_threshold: config.error_break_threshold,
        }
    }

    /// PHASE 1: Check if this call WOULD trigger a loop break. Call BEFORE execution.
    pub fn check_limits(&self, tool_name: &str, args: &serde_json::Value) -> LoopStatus {
        if !self.recent.is_empty() {
            let hash = Self::hash_call(tool_name, args);
            if self.last_hash == Some(hash) && self.consecutive + 1 >= self.break_threshold {
                return LoopStatus::Break(format!("tool '{}' called {} times consecutively", tool_name, self.consecutive + 1));
            }
        }
        LoopStatus::Ok
    }

    /// PHASE 2: Record actual execution.
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

        self.record_result(tool_name, success)
    }

    /// Record only the result (used for WAL warm-up and after execution).
    pub fn record_result(&mut self, tool_name: &str, success: bool) -> LoopStatus {
        if success {
            self.consecutive_errors = 0;
            self.last_error_tool = None;
        } else {
            if self.last_error_tool.as_deref() == Some(tool_name) {
                self.consecutive_errors += 1;
            } else {
                self.consecutive_errors = 1;
                self.last_error_tool = Some(tool_name.to_string());
            }
            if self.consecutive_errors >= self.error_break_threshold {
                return LoopStatus::Break(format!("tool '{}' failed {} times consecutively", tool_name, self.consecutive_errors));
            }
        }
        LoopStatus::Ok
    }

    pub fn hash_call_raw(name: &str, args: &serde_json::Value) -> u64 { Self::hash_call(name, args) }

    fn hash_call(name: &str, args: &serde_json::Value) -> u64 {
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        let args_str = serde_json::to_string(args).unwrap_or_default();
        args_str.hash(&mut hasher);
        hasher.finish()
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

impl From<&crate::config::ToolLoopSettings> for ToolLoopConfig {
    fn from(s: &crate::config::ToolLoopSettings) -> Self {
        Self {
            max_iterations: s.max_iterations,
            compact_on_overflow: s.compact_on_overflow,
            detect_loops: s.detect_loops,
            break_threshold: s.break_threshold,
            max_consecutive_failures: s.max_consecutive_failures,
            max_auto_continues: s.max_auto_continues,
            max_loop_nudges: s.max_loop_nudges,
            error_break_threshold: s.error_break_threshold.unwrap_or(3),
        }
    }
}

pub fn is_context_overflow(error: &anyhow::Error) -> bool {
    let msg = error.to_string().to_lowercase();
    msg.contains("context length") || msg.contains("token limit") || msg.contains("too many token") || msg.contains("input too long") || (msg.contains("400") && (msg.contains("too long") || msg.contains("too large")))
}
