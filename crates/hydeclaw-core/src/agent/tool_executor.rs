//! ToolExecutor trait, ToolExecutorCtx value type, DefaultToolExecutor implementation,
//! and ToolExecutorDeps private trait.
//!
//! Extracted from engine_dispatch.rs and engine_parallel.rs to decouple tool execution
//! from the engine god object, enabling mock injection in tests (TOOL-01..TOOL-03).
//! Follows the same OnceLock + private deps trait pattern as ContextBuilder (Phase 38).

use anyhow::Result;
use async_trait::async_trait;
use hydeclaw_types::ToolCall;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

pub use crate::agent::engine::LoopBreak;
use crate::agent::tool_loop::LoopDetector;

// ── Public types ──────────────────────────────────────────────────────────────

/// Per-call value type carrying the routing context for a single tool execution.
/// Constructed fresh at each execute() call site; NOT stored long-lived.
/// Limited to <=6 fields (TOOL-02).
pub struct ToolExecutorCtx {
    pub session_id: Uuid,
    pub channel: String,
    pub context: serde_json::Value,
    pub current_context_chars: usize,
    pub sse_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::agent::engine::StreamEvent>>,
    pub is_automated: bool,
}

// ── ToolExecutor public trait ─────────────────────────────────────────────────

/// Abstraction over tool execution so unit tests can inject a `MockToolExecutor`
/// without needing a live LLM stack or filesystem.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a single tool call. Returns the tool result string.
    async fn execute(&self, name: &str, arguments: &serde_json::Value) -> String;

    /// Execute a batch of tool calls with loop detection and partitioned parallelism.
    async fn execute_batch(
        &self,
        tool_calls: &[ToolCall],
        context: &serde_json::Value,
        session_id: Uuid,
        channel: &str,
        current_context_chars: usize,
        detector: &mut LoopDetector,
        detect_loops: bool,
    ) -> Result<Vec<(String, String)>, LoopBreak>;
}

// ── ToolExecutorDeps private trait ───────────────────────────────────────────

/// Private trait listing the AgentEngine capabilities consumed by DefaultToolExecutor.
/// `AgentEngine` implements this; the impl delegates to its own fields/methods.
/// This avoids a direct Arc<AgentEngine> dependency from tool_executor.rs back to engine.rs.
#[async_trait]
pub(crate) trait ToolExecutorDeps: Send + Sync {
    /// Tool dispatch — delegates to existing engine_dispatch.rs methods.
    fn execute_tool_call_raw<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = String> + Send + 'a>>;

    /// Batch execution — delegates to engine_parallel.rs.
    async fn execute_tool_calls_partitioned_raw(
        &self,
        tool_calls: &[ToolCall],
        context: &serde_json::Value,
        session_id: Uuid,
        channel: &str,
        current_context_chars: usize,
        detector: &mut LoopDetector,
        detect_loops: bool,
    ) -> Result<Vec<(String, String)>, LoopBreak>;
}

// ── DefaultToolExecutor ───────────────────────────────────────────────────────

/// Concrete implementation of `ToolExecutor` that delegates all engine access
/// through the `ToolExecutorDeps` trait.
pub struct DefaultToolExecutor {
    deps: Arc<dyn ToolExecutorDeps>,
    /// Self-reference for recursive subagent calls (Arc<dyn ToolExecutor>).
    /// Initialized via `set_self_ref` after the executor is wrapped in Arc.
    self_ref: OnceLock<Arc<dyn ToolExecutor>>,
}

impl DefaultToolExecutor {
    pub fn new(deps: Arc<dyn ToolExecutorDeps>) -> Self {
        Self {
            deps,
            self_ref: OnceLock::new(),
        }
    }

    /// Store self-reference for recursive tool calls (mirrors AgentEngine::set_self_ref).
    pub fn set_self_ref(&self, arc: &Arc<dyn ToolExecutor>) {
        let _ = self.self_ref.set(arc.clone());
    }
}

// Compile-time Send safety assertion (PITFALLS.md Pitfall 1)
fn _assert_send() {
    fn _check<T: Send>() {}
    _check::<Box<dyn ToolExecutor>>();
}

#[async_trait]
impl ToolExecutor for DefaultToolExecutor {
    async fn execute(&self, name: &str, arguments: &serde_json::Value) -> String {
        self.deps.execute_tool_call_raw(name, arguments).await
    }

    async fn execute_batch(
        &self,
        tool_calls: &[ToolCall],
        context: &serde_json::Value,
        session_id: Uuid,
        channel: &str,
        current_context_chars: usize,
        detector: &mut LoopDetector,
        detect_loops: bool,
    ) -> Result<Vec<(String, String)>, LoopBreak> {
        self.deps
            .execute_tool_calls_partitioned_raw(
                tool_calls,
                context,
                session_id,
                channel,
                current_context_chars,
                detector,
                detect_loops,
            )
            .await
    }
}
