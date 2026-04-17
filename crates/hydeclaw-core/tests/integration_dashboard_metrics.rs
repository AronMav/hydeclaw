//! RES-02: Validate `/api/health/dashboard` contract.
//!
//! Three tests cover:
//!   1. Registry empty → empty snapshot (baseline surface)
//!   2. Registry + BoundMetricsProbe → recorded drops readable via probe
//!   3. Handler-level grouping: `build_dashboard_body` (the pure function
//!      `api_health_dashboard` delegates to) produces `{agent: {event_type:
//!      count}}` nested JSON — NOT flat `"agent:event_type"` pair-keys.
//!
//! The third test pins the exact shape that the `/api/health/dashboard`
//! handler emits, guarding against regression to a flat representation
//! during Phase 65 OBS-05 extensions.

mod support;

use std::sync::Arc;
use std::time::Duration;

use hydeclaw_core::metrics::{build_dashboard_body, MetricsRegistry};
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_empty_returns_empty_snapshot() {
    timeout(Duration::from_secs(10), async {
        let registry = Arc::new(MetricsRegistry::new());
        let snapshot = registry.snapshot_sse_drops();
        assert!(snapshot.is_empty(), "fresh registry must be empty");
    })
    .await
    .expect("test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registry_reflects_recorded_drops_via_probe() {
    timeout(Duration::from_secs(10), async {
        let registry = Arc::new(MetricsRegistry::new());
        registry.record_sse_drop("agent-a", "text-delta");
        registry.record_sse_drop("agent-a", "text-delta");
        registry.record_sse_drop("agent-b", "finish");

        let snapshot = registry.snapshot_sse_drops();
        assert_eq!(
            snapshot.get(&("agent-a".to_string(), "text-delta".to_string())),
            Some(&2)
        );
        assert_eq!(
            snapshot.get(&("agent-b".to_string(), "finish".to_string())),
            Some(&1)
        );

        // BoundMetricsProbe reads via the same path the handler uses.
        let probe = support::MetricsProbe::new().connect(registry.clone());
        assert_eq!(probe.read_counter("agent-a", "text-delta"), 2);
        assert_eq!(probe.read_counter("agent-b", "finish"), 1);
        assert_eq!(probe.read_counter("agent-missing", "text-delta"), 0);
    })
    .await
    .expect("test timed out");
}

/// Handler-shape test. Exercises the flat→nested transformation that the
/// `/api/health/dashboard` handler `api_health_dashboard(` delegates to via
/// `build_dashboard_body`. Asserts nested grouping, stable keys, and
/// lossless serde round-trip. Explicitly rejects a flat `"agent:event_type"`
/// pair-key representation as a regression guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dashboard_handler_groups_drops_by_agent() {
    timeout(Duration::from_secs(10), async {
        // Seed a real registry with mixed counters across 2 agents and
        // 2 event types (matches the shape Phase 62 RES-01 coalescer will
        // emit — text-delta drops + the occasional finish drop).
        let registry = Arc::new(MetricsRegistry::new());
        registry.record_sse_drop("agent-a", "text-delta");
        registry.record_sse_drop("agent-a", "text-delta");
        registry.record_sse_drop("agent-a", "finish");
        for _ in 0..5 {
            registry.record_sse_drop("agent-b", "text-delta");
        }

        // Build the dashboard body via the same pure function the handler
        // `api_health_dashboard(` uses. No gateway state extraction needed:
        // the handler itself is a one-liner `Json(build_dashboard_body(...))`
        // so testing this function IS testing the handler's payload.
        let body = build_dashboard_body(&registry);

        // 1. Top-level version.
        assert_eq!(body["version"], "0.19.0", "version field must be 0.19.0");

        // 2. `sse_events_dropped_total` must be an object (nested shape).
        let map = body["sse_events_dropped_total"]
            .as_object()
            .expect("sse_events_dropped_total must be a JSON object (nested)");

        // 3. Nested grouping: {agent: {event_type: count}}.
        assert_eq!(
            map["agent-a"]["text-delta"].as_u64(),
            Some(2),
            "agent-a text-delta count mismatch; body: {body}"
        );
        assert_eq!(
            map["agent-a"]["finish"].as_u64(),
            Some(1),
            "agent-a finish count mismatch; body: {body}"
        );
        assert_eq!(
            map["agent-b"]["text-delta"].as_u64(),
            Some(5),
            "agent-b text-delta count mismatch; body: {body}"
        );

        // 4. Flat pair-keys MUST NOT appear (regression guard).
        assert!(
            !map.contains_key("agent-a:text-delta"),
            "must not emit flat pair-keys like 'agent-a:text-delta'; body: {body}"
        );
        assert!(
            !map.contains_key("agent-b:text-delta"),
            "must not emit flat pair-keys like 'agent-b:text-delta'; body: {body}"
        );

        // 5. Round-trip through serde_json::to_string without loss.
        let rendered = serde_json::to_string(&body).expect("serialize");
        let reparsed: serde_json::Value =
            serde_json::from_str(&rendered).expect("reparse");
        assert_eq!(reparsed, body, "serde round-trip must be lossless");
    })
    .await
    .expect("handler test timed out");
}
