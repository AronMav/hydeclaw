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
}
