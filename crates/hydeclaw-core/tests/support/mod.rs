//! Shared test harness for HydeClaw integration tests.
//!
//! Each test binary under `tests/` opts in via `mod support;` and uses the re-exports below.
//!
//! Phase 61 Plan 02 adds `mock_provider`. Plan 01 (wave-1, runs in a separate
//! parallel worktree) adds `harness` and `migrations` alongside; when the two
//! plans' worktrees merge, this file should grow two additional `pub mod`
//! declarations + corresponding re-exports. Deferring those decls here keeps
//! Plan 02's worktree independently buildable.

#![allow(dead_code)] // Each integration binary uses a different subset; silence unused warnings.

pub mod mock_provider;

pub use mock_provider::{MockLlmProvider, MockProvider, MockTurn};
