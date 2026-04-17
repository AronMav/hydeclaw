//! Phase 62 minimal metrics registry.
//!
//! AtomicU64 counters keyed by (agent, event_type). Phase 65 OBS-02 layers
//! OpenTelemetry meter wrappers on top — Phase 62 only needs raw counters
//! to back GET /api/health/dashboard and the Phase 62 RES-01 coalescer
//! drop counter. NO external dependencies (std + tracing only).
//!
//! Phase 64 SEC-05 (additive): CSP violation counter keyed by directive,
//! with length + cardinality caps to prevent hostile browsers from inflating
//! the map. Overflow attempts past the cap bump a single `csp_violations_overflow`
//! atomic instead of growing the map.

use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Sampled-warn sampling rate: log 1 out of every 64 drops.
/// Keeps logs non-overwhelming under saturation (RES-02).
const DROP_WARN_SAMPLE_RATE: u64 = 64;

/// Phase 64 SEC-05: cap distinct directives to prevent unbounded growth from
/// hostile browsers cycling directive names. Covers every standard CSP directive
/// (default-src, script-src, style-src, img-src, connect-src, font-src, object-src,
/// media-src, frame-src, worker-src, manifest-src, form-action, frame-ancestors,
/// base-uri, report-uri, report-to, upgrade-insecure-requests, block-all-mixed-content,
/// require-sri-for, require-trusted-types-for, trusted-types, sandbox, plugin-types,
/// prefetch-src, navigate-to, referrer, child-src, script-src-elem, script-src-attr,
/// style-src-elem, style-src-attr, webrtc ≈ 31) with 1 slot of headroom.
pub const MAX_CSP_DIRECTIVES: usize = 32;

/// Phase 64 SEC-05: cap each directive key length — hostile browsers could otherwise
/// send multi-KB "directive" strings that bloat the counter map memory footprint.
pub const MAX_CSP_DIRECTIVE_LEN: usize = 64;

/// Central metrics registry for Phase 62 observability.
///
/// Lookup path: RwLock for keyed entry (insert on first use), AtomicU64 for
/// the hot-path increment. Reads take the RwLock in shared mode + AtomicU64
/// load. Keeps contention minimal even under 10k+ synthetic sessions.
pub struct MetricsRegistry {
    /// (agent, event_type) -> dropped counter.
    sse_events_dropped: RwLock<HashMap<(String, String), AtomicU64>>,
    /// Phase 64 SEC-05: directive -> violation count. Cardinality capped at
    /// `MAX_CSP_DIRECTIVES` (see `record_csp_violation`). Keys are truncated
    /// to `MAX_CSP_DIRECTIVE_LEN` before storage.
    csp_violations_total: RwLock<HashMap<String, AtomicU64>>,
    /// Phase 64 SEC-05: number of attempts to add a directive past the
    /// cardinality cap. A non-zero value signals abuse and should trigger
    /// operator attention.
    csp_violations_overflow: AtomicU64,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            sse_events_dropped: RwLock::new(HashMap::new()),
            csp_violations_total: RwLock::new(HashMap::new()),
            csp_violations_overflow: AtomicU64::new(0),
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
                if new_count.is_multiple_of(DROP_WARN_SAMPLE_RATE) {
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
        if new_count.is_multiple_of(DROP_WARN_SAMPLE_RATE) {
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

    // ── Phase 64 SEC-05 — CSP violations counter ────────────────────────

    /// Record a single CSP violation for the given directive.
    ///
    /// Defensive policy:
    ///   * Directive keys longer than [`MAX_CSP_DIRECTIVE_LEN`] are truncated.
    ///   * Existing keys always increment — even if the map is at capacity.
    ///   * New keys are rejected once the map reaches [`MAX_CSP_DIRECTIVES`]
    ///     entries; the rejection increments `csp_violations_overflow` so
    ///     operators see the abuse signal in the dashboard.
    ///
    /// Truncation happens on a byte boundary via `char` iteration so we never
    /// split UTF-8 mid-sequence, even though browsers normally only send ASCII
    /// directive names.
    pub fn record_csp_violation(&self, directive: &str) {
        let key: String = if directive.len() > MAX_CSP_DIRECTIVE_LEN {
            let mut truncated = String::with_capacity(MAX_CSP_DIRECTIVE_LEN);
            for ch in directive.chars() {
                if truncated.len() + ch.len_utf8() > MAX_CSP_DIRECTIVE_LEN {
                    break;
                }
                truncated.push(ch);
            }
            truncated
        } else {
            directive.to_string()
        };

        // Fast path: key already present → bump under a read lock.
        {
            let read = self
                .csp_violations_total
                .read()
                .expect("csp RwLock poisoned");
            if let Some(counter) = read.get(&key) {
                counter.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }

        // Slow path: upgrade to write lock, enforce cardinality cap.
        let mut write = self
            .csp_violations_total
            .write()
            .expect("csp RwLock poisoned");
        // Re-check after re-acquiring (another writer may have inserted).
        if let Some(counter) = write.get(&key) {
            counter.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if write.len() >= MAX_CSP_DIRECTIVES {
            self.csp_violations_overflow.fetch_add(1, Ordering::Relaxed);
            return;
        }
        write
            .entry(key)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Read current count for a specific directive (0 if absent).
    /// Test-facing accessor (used by `integration_csp_report.rs`).
    #[allow(dead_code)]
    pub fn csp_violations_total_count(&self, directive: &str) -> u64 {
        let read = self
            .csp_violations_total
            .read()
            .expect("csp RwLock poisoned");
        read.get(directive)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Number of distinct directives currently stored (useful for cap tests).
    /// Test-facing accessor.
    #[allow(dead_code)]
    pub fn csp_violations_map_len(&self) -> usize {
        let read = self
            .csp_violations_total
            .read()
            .expect("csp RwLock poisoned");
        read.len()
    }

    /// Snapshot all stored directive keys (test-facing; allocates a Vec).
    #[allow(dead_code)]
    pub fn csp_violations_keys_snapshot(&self) -> Vec<String> {
        let read = self
            .csp_violations_total
            .read()
            .expect("csp RwLock poisoned");
        read.keys().cloned().collect()
    }

    /// Snapshot all CSP violation counts as a `{directive: count}` map.
    pub fn snapshot_csp_violations(&self) -> HashMap<String, u64> {
        let read = self
            .csp_violations_total
            .read()
            .expect("csp RwLock poisoned");
        read.iter()
            .map(|(k, v)| (k.clone(), v.load(Ordering::Relaxed)))
            .collect()
    }

    /// Overflow counter — bumped every time a new directive is rejected
    /// because the map is already at [`MAX_CSP_DIRECTIVES`] entries.
    pub fn csp_violations_overflow_count(&self) -> u64 {
        self.csp_violations_overflow.load(Ordering::Relaxed)
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

    // Phase 64 SEC-05: CSP violation counter (additive field; pre-existing
    // dashboard consumers treat unknown keys as opaque — see RES-02 doc).
    let csp_violations: BTreeMap<String, u64> = registry
        .snapshot_csp_violations()
        .into_iter()
        .collect();

    serde_json::json!({
        "version": "0.19.0",
        "sse_events_dropped_total": by_agent,
        "csp_violations": csp_violations,
        "csp_violations_overflow": registry.csp_violations_overflow_count(),
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
