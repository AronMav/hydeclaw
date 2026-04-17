//! MetricsProbe — test helper that reads AtomicU64 counters by (agent, event_type).
//!
//! Standalone today (stores its own DashMap<(String, String), AtomicU64> for
//! self-tests). Once Plan 02 ships `src/metrics.rs`, the `connect(&MetricsRegistry)`
//! method wires this probe to the real registry. Until then, `connect` is a stub.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub struct MetricsProbe {
    // (agent, event_type) -> counter
    inner: Mutex<HashMap<(String, String), AtomicU64>>,
}

impl MetricsProbe {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Read counter value for (agent, event_type). Returns 0 if unknown.
    pub fn read_counter(&self, agent: &str, event_type: &str) -> u64 {
        let guard = self.inner.lock().expect("metrics probe poisoned");
        guard
            .get(&(agent.to_string(), event_type.to_string()))
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Take a snapshot of all counters.
    pub fn snapshot(&self) -> HashMap<(String, String), u64> {
        let guard = self.inner.lock().expect("metrics probe poisoned");
        guard
            .iter()
            .map(|(k, v)| (k.clone(), v.load(Ordering::Relaxed)))
            .collect()
    }

    /// Test-only: increment a counter. Production counters are updated by
    /// src/metrics.rs (Plan 02) — this is for self-tests of the probe.
    #[allow(dead_code)]
    pub fn bump(&self, agent: &str, event_type: &str) {
        let mut guard = self.inner.lock().expect("metrics probe poisoned");
        guard
            .entry((agent.to_string(), event_type.to_string()))
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Stub: will wire to `crate::metrics::MetricsRegistry` once Plan 02 ships.
    /// Returns self unchanged today. Do not remove — keeps Plan 02 API stable.
    #[allow(dead_code)]
    pub fn connect(self) -> Self {
        self
    }
}

impl Default for MetricsProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_counter_returns_zero_before_bump() {
        let probe = MetricsProbe::new();
        assert_eq!(probe.read_counter("agent-a", "text-delta"), 0);
    }

    #[test]
    fn bump_increments_counter() {
        let probe = MetricsProbe::new();
        probe.bump("agent-a", "text-delta");
        probe.bump("agent-a", "text-delta");
        probe.bump("agent-a", "tool-call");
        assert_eq!(probe.read_counter("agent-a", "text-delta"), 2);
        assert_eq!(probe.read_counter("agent-a", "tool-call"), 1);
        assert_eq!(probe.read_counter("agent-b", "text-delta"), 0);
    }

    #[test]
    fn snapshot_returns_all_labels() {
        let probe = MetricsProbe::new();
        probe.bump("a", "x");
        probe.bump("b", "y");
        let snap = probe.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap.get(&("a".to_string(), "x".to_string())), Some(&1));
        assert_eq!(snap.get(&("b".to_string(), "y".to_string())), Some(&1));
    }
}
