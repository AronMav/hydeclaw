//! Library facade for `hydeclaw-core` integration tests.
//!
//! This crate also has a binary target (`src/main.rs`) — the `[lib]` exists
//! solely so test code can re-export shared types.
//!
//! Phase 61 keeps this surface MINIMAL. The ONLY re-export is `hydeclaw_types`
//! so the integration test mock can construct LlmResponse-shaped values
//! without re-importing the workspace dep at the dev-dep layer.
//!
//! Wave-2 plans (notably Plan 03 db re-export) extend by adding
//! `pub mod` declarations for modules they need — capped at 10 modules total
//! to avoid the lib facade becoming a parallel module tree.
//!
//! DEFERRED (out of scope for Phase 61): exposing `crate::agent::providers::LlmProvider`
//! requires re-including the entire agent subtree (cli_backend, secrets, thinking,
//! providers_*_impl) which exceeds the 10-module cascade cap. Phase 66 REF-01
//! splits engine.rs and naturally exposes a smaller provider surface; the bridge
//! is a one-line blanket impl at that point.

#![allow(clippy::missing_docs_in_private_items)]

// Re-export hydeclaw-types so test code can build LlmResponse values
// without re-importing the workspace dep at the dev-dep layer.
pub use hydeclaw_types;

// ── Phase 62 Plan 02: metrics surface ──────────────────────────────────
// `metrics` is a leaf module with zero crate-internal deps (std + tracing only),
// so re-exporting it here does not cascade the lib facade. Integration tests
// (`integration_dashboard_metrics.rs`) and `tests/support/metrics_probe.rs`
// both reach `MetricsRegistry` via `hydeclaw_core::metrics::MetricsRegistry`.
#[path = "metrics.rs"]
pub mod metrics;

// ── Phase 62 Plan 03: SSE coalescer + StreamEvent leaf exposure ────────
// `agent::stream_event` and `gateway::sse::coalescer` are both leaf modules
// (zero `crate::*` imports) so the lib can expose them for the
// `tests/integration_sse_coalescing.rs` 10k-burst + drop-counter tests.
// We preserve the original paths the binary target uses:
//   * `hydeclaw_core::agent::engine::StreamEvent` — facade that re-exports
//     the leaf enum (same path the binary's `crate::agent::engine::StreamEvent`
//     resolves to). Callers don't need to learn a new path.
//   * `hydeclaw_core::gateway::sse::spawn_coalescing_converter` — leaf
//     coalescer task entry point.
// Neither `agent/engine.rs` nor any other non-leaf module is pulled in.
pub mod agent {
    //! Test-facing re-export subset of the binary's `src/agent/` tree.
    //! ONLY the two leaf modules are exposed — including `engine.rs` would
    //! cascade dozens of `super::*` imports (secrets, providers, tool_loop,
    //! workspace, …) and blow the 10-module lib-facade cap.
    //!
    //! `engine` here is a TINY facade that re-exports `StreamEvent` so
    //! external callers can keep using `agent::engine::StreamEvent`.

    #[path = "stream_event.rs"]
    pub mod stream_event;

    pub mod engine {
        //! Facade preserving `agent::engine::StreamEvent` path.
        pub use super::stream_event::StreamEvent;
    }
}

// ── Phase 62 Plan 04: shutdown drain surface ───────────────────────────
// `shutdown` is trait-parametric over `DrainableAgent`, so it has zero
// crate-internal deps (only std + tokio + futures-util + tracing). Safe
// to re-export here without cascading the agent subtree into the lib.
// Integration tests (`integration_shutdown_reproducer.rs`) can exercise
// the drain sequence directly against fake handles; the binary target
// wires `AgentHandle: DrainableAgent` in `src/agent/handle.rs`.
#[path = "shutdown.rs"]
pub mod shutdown;

// ── Phase 62 Plan 06: rate limiter sweep() surface ─────────────────────
// `gateway::rate_limiter` is a leaf module (deps: std + tokio::sync::Mutex
// + tracing — no `crate::*` references). We re-export just the leaf via a
// minimal `gateway::middleware` facade so integration tests can reach
// `AuthRateLimiter` / `RequestRateLimiter` at the path they expect:
// `hydeclaw_core::gateway::middleware::{AuthRateLimiter, RequestRateLimiter}`.
// This keeps the test-facing lib surface intact without pulling the gateway
// handler subtree (which would cascade dozens of modules — see Phase 61
// 10-module cap note above).
#[path = "gateway"]
pub mod gateway {
    //! Test-facing re-export subset of the binary's `src/gateway/` tree.
    //! ONLY the leaf `rate_limiter` and `sse` modules are exposed;
    //! `middleware` is a pure re-export facade for the
    //! `middleware::{AuthRateLimiter, ...}` path consumed by Phase 62 RES-04
    //! integration tests. `sse` is exposed for Phase 62 RES-01
    //! integration tests.

    #[path = "rate_limiter.rs"]
    pub mod rate_limiter;

    pub mod middleware {
        //! Facade preserving `gateway::middleware::{AuthRateLimiter, RequestRateLimiter}`
        //! path used by `integration_rate_limiter_sweeper.rs`.
        pub use super::rate_limiter::{AuthRateLimiter, RequestRateLimiter};
    }

    // Phase 62 RES-01: `sse::coalescer` is a leaf module
    // (deps: std + tokio + tracing + `crate::agent::engine::StreamEvent`
    // + `crate::metrics::MetricsRegistry` — both already exposed above).
    // Safe to re-export without cascading the gateway handler subtree.
    #[path = "sse"]
    pub mod sse {
        //! SSE coalescer leaf — safe to re-export for
        //! `integration_sse_coalescing.rs`.

        #[path = "coalescer.rs"]
        pub mod coalescer;

        pub use coalescer::spawn_coalescing_converter;
    }
}

// ── Test-facing re-exports added by Phase 61 Plan 03 ────────────────────
// Wave-2 characterization tests need direct access to `db::approvals`.
// These re-exports are TEST-FACING ONLY — production consumers continue
// to use the binary's internal module tree via `src/main.rs`.
//
// CASCADE AVOIDANCE: including `db/mod.rs` would pull in every db submodule,
// some of which reference `crate::memory` (see `db/memory_queries.rs`) and
// would in turn cascade to `config`, `secrets`, etc. — exceeding the
// 10-module budget documented at the top of this file. Instead, we include
// ONLY `db/approvals.rs` via a `#[path]` attribute, because `approvals.rs`
// has zero crate-internal dependencies (only `anyhow`, `chrono`, `sqlx`,
// `uuid` — all regular `[dependencies]` so the lib already has them).
pub mod db {
    //! Test-facing re-export subset of the binary's `src/db/` tree.
    //! Keep this minimal — every added submodule risks pulling in new
    //! crate::* cross-references and cascading the lib surface.
    //!
    //! The `#[path]` attribute is resolved relative to the default
    //! directory of this inline module, which is `src/db/`. Hence the
    //! bare filename points at `src/db/approvals.rs`.

    #[path = "approvals.rs"]
    pub mod approvals;

    // Phase 62 RES-03: `session_wal` is a leaf module (deps: anyhow, sqlx,
    // uuid, serde_json — no crate::* references). Safe to re-export without
    // cascading the lib surface. Consumed by
    // `tests/integration_session_events_cleanup.rs`.
    #[path = "session_wal.rs"]
    pub mod session_wal;

    // Phase 63 DATA-02: `sessions` is a leaf module (deps: anyhow, chrono,
    // sqlx, uuid — no crate::* references). Safe to re-export without
    // cascading the lib surface. Consumed by
    // `tests/integration_stuck_sessions_window_fn.rs`.
    #[path = "sessions.rs"]
    pub mod sessions;
}
