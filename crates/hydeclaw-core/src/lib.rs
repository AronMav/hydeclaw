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
