// ── AgentConfig — immutable snapshot of agent configuration ─────────────────
//
// Phase 1 of the AgentEngine decomposition. This struct captures the 16
// immutable fields that today live directly on `AgentEngine`.  It is created
// once per engine instantiation and never mutated afterwards.
//
// Created alongside the existing `AgentEngine` — not yet wired in.

use std::sync::Arc;

use sqlx::PgPool;

use crate::agent::approval_manager::ApprovalManager;
use crate::agent::context_builder::ContextBuilder;
use crate::agent::memory_service::MemoryService;
use crate::agent::providers::LlmProvider;
use crate::agent::session_agent_pool::SessionPoolsMap;
use crate::agent::tool_executor::DefaultToolExecutor;
use crate::config::{AgentSettings, AppConfig};
use crate::db::audit_queue::AuditQueue;
use crate::gateway::state::AgentMap;
use crate::memory::EmbeddingService;
use crate::scheduler::Scheduler;
use crate::tools::ToolRegistry;

/// Immutable snapshot of everything an agent needs to operate.
///
/// Grouped into five concern areas: identity, LLM, data, tools, and infra.
/// All fields are either `Clone`-cheap (`Arc`, `PgPool`) or small value types.
///
/// Phase 1: defined but not yet wired into `AgentEngine`. The allow(dead_code)
/// is removed once the struct is consumed in later phases.
#[allow(dead_code)]
pub struct AgentConfig {
    // ── Identity ────────────────────────────────────────────────────────
    pub agent: AgentSettings,
    pub workspace_dir: String,
    pub default_timezone: String,
    pub app_config: Arc<AppConfig>,

    // ── LLM ─────────────────────────────────────────────────────────────
    pub provider: Arc<dyn LlmProvider>,
    pub compaction_provider: Option<Arc<dyn LlmProvider>>,

    // ── Data ────────────────────────────────────────────────────────────
    pub db: PgPool,
    pub memory_store: Arc<dyn MemoryService>,
    pub embedder: Arc<dyn EmbeddingService>,

    // ── Tools ───────────────────────────────────────────────────────────
    pub tools: ToolRegistry,
    pub tool_executor: Arc<DefaultToolExecutor>,
    pub context_builder: Arc<dyn ContextBuilder>,
    pub approval_manager: Arc<ApprovalManager>,

    // ── Infra ───────────────────────────────────────────────────────────
    pub scheduler: Option<Arc<Scheduler>>,
    pub agent_map: Option<AgentMap>,
    pub session_pools: Option<SessionPoolsMap>,
    pub audit_queue: Arc<AuditQueue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time assertion: `AgentConfig` must be `Send + Sync` so it can
    /// live inside `Arc` and be shared across tokio tasks.
    #[test]
    fn agent_config_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AgentConfig>();
    }
}
