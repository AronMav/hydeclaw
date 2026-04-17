//! Phase 62 minimal metrics registry.
//!
//! AtomicU64 counters keyed by (agent, event_type). Phase 65 OBS-02 layers
//! OpenTelemetry meter wrappers on top — Phase 62 only needs raw counters
//! to back GET /api/health/dashboard and the Phase 62 RES-01 coalescer
//! drop counter. NO external dependencies (std + tracing only).

use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Sampled-warn sampling rate: log 1 out of every 64 drops.
/// Keeps logs non-overwhelming under saturation (RES-02).
const DROP_WARN_SAMPLE_RATE: u64 = 64;

/// Central metrics registry for Phase 62 observability.
///
/// Lookup path: RwLock for keyed entry (insert on first use), AtomicU64 for
/// the hot-path increment. Reads take the RwLock in shared mode + AtomicU64
/// load. Keeps contention minimal even under 10k+ synthetic sessions.
pub struct MetricsRegistry {
    /// (agent, event_type) -> dropped counter.
    sse_events_dropped: RwLock<HashMap<(String, String), AtomicU64>>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            sse_events_dropped: RwLock::new(HashMap::new()),
        }
    }

    /// Record a dropped SSE event. Safe to call from any task.
    /// Emits sampled warn log every 64th drop per (agent, event_type).
    pub fn record_sse_drop(&self, agent: &str, event_type: &str) {
        // Fast path: key already exists, grab shared read lock + atomic inc.
        {
            let read = self.sse_events_dropped.read().expect("metrics RwLock poisoned");
            if let Some(counter) = read.get(&(agent.to_string(), event_type.to_string())) {
                let prev = counter.fetch_add(1, Ordering::Relaxed);
                let new_count = prev.wrapping_add(1);
                if new_count % DROP_WARN_SAMPLE_RATE == 0 {
                    tracing::warn!(
                        agent = %agent,
                        event_type = %event_type,
                        total = new_count,
                        "sse event drop (sampled 1/{})",
                        DROP_WARN_SAMPLE_RATE
                    );
                }
                return;
            }
        }
        // Slow path: insert new key under write lock.
        let mut write = self.sse_events_dropped.write().expect("metrics RwLock poisoned");
        let counter = write
            .entry((agent.to_string(), event_type.to_string()))
            .or_insert_with(|| AtomicU64::new(0));
        let prev = counter.fetch_add(1, Ordering::Relaxed);
        let new_count = prev.wrapping_add(1);
        if new_count % DROP_WARN_SAMPLE_RATE == 0 {
            tracing::warn!(
                agent = %agent,
                event_type = %event_type,
                total = new_count,
                "sse event drop (sampled 1/{})",
                DROP_WARN_SAMPLE_RATE
            );
        }
    }

    /// Snapshot all dropped-event counters. Used by /api/health/dashboard.
    pub fn snapshot_sse_drops(&self) -> HashMap<(String, String), u64> {
        let read = self.sse_events_dropped.read().expect("metrics RwLock poisoned");
        read.iter()
            .map(|(k, v)| (k.clone(), v.load(Ordering::Relaxed)))
            .collect()
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the `GET /api/health/dashboard` response body from a `MetricsRegistry`.
///
/// Pure flat→nested transformation: `snapshot_sse_drops()` returns
/// `HashMap<(agent, event_type), u64>` (flat), and this function groups it
/// into `{agent: {event_type: count}}` (nested) using `BTreeMap` for stable
/// key ordering in serialized JSON.
///
/// Used by `gateway::handlers::monitoring::api_health_dashboard` as the
/// single source of truth for the dashboard JSON shape.  Exposed on the
/// library surface so integration tests (`integration_dashboard_metrics.rs`)
/// can pin the nested-grouping contract without reaching into the gateway
/// handler subtree.
///
/// Returns a JSON object of the form:
/// ```json
/// {
///   "version": "0.19.0",
///   "sse_events_dropped_total": { "<agent>": { "<event_type>": <count> } }
/// }
/// ```
/// Phase 65 OBS-05 extends with additional fields (active_agents, DB pool
/// stats, …); clients MUST treat unknown top-level fields as opaque.
pub fn build_dashboard_body(registry: &MetricsRegistry) -> serde_json::Value {
    use std::collections::BTreeMap;

    let drops = registry.snapshot_sse_drops();
    // Flat (agent, event_type) → nested {agent: {event_type: count}}.
    let mut by_agent: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for ((agent, event_type), count) in drops {
        by_agent.entry(agent).or_default().insert(event_type, count);
    }
    serde_json::json!({
        "version": "0.19.0",
        "sse_events_dropped_total": by_agent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn new_registry_has_empty_snapshot() {
        let reg = MetricsRegistry::new();
        assert!(reg.snapshot_sse_drops().is_empty());
    }

    #[test]
    fn record_sse_drop_accumulates() {
        let reg = MetricsRegistry::new();
        for _ in 0..3 {
            reg.record_sse_drop("agent-a", "text-delta");
        }
        reg.record_sse_drop("agent-b", "finish");
        let snap = reg.snapshot_sse_drops();
        assert_eq!(snap.get(&("agent-a".to_string(), "text-delta".to_string())), Some(&3));
        assert_eq!(snap.get(&("agent-b".to_string(), "finish".to_string())), Some(&1));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn record_sse_drop_is_thread_safe() {
        let reg = Arc::new(MetricsRegistry::new());
        let mut handles = Vec::new();
        for _ in 0..100 {
            let r = reg.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..100 {
                    r.record_sse_drop("agent-x", "text-delta");
                }
            }));
        }
        for h in handles {
            h.await.expect("task failed");
        }
        let snap = reg.snapshot_sse_drops();
        assert_eq!(
            snap.get(&("agent-x".to_string(), "text-delta".to_string())),
            Some(&10_000)
        );
    }
}
