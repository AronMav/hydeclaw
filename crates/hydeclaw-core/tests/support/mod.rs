//! Shared test harness for HydeClaw integration tests.
//!
//! Each test binary under `tests/` opts in via `mod support;` and uses the re-exports below.
//!
//! Wave-2 plans (approval_race, sse_lifecycle, shutdown_drain) build on this module.

#![allow(dead_code)] // Each integration binary uses a different subset; silence unused warnings.

pub mod harness;
pub mod migrations;

pub use harness::TestHarness;
