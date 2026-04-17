use anyhow::Result;
use hydeclaw_types::{IncomingMessage, Message, MessageRole, ToolDefinition};
use sqlx::PgPool;
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::channel_actions::ChannelActionRouter;
use super::providers::LlmProvider;
use crate::mcp::McpRegistry;

use super::error_classify;
use super::thinking::{looks_incomplete, maybe_strip_thinking, strip_thinking};
use super::tool_loop::LoopDetector;


// Extracted impl AgentEngine blocks (submodules of engine for full super:: access)
pub use crate::agent::pipeline::parallel::LoopBreak;
pub(crate) use crate::agent::pipeline::subagent::parse_subagent_timeout;
#[path = "../engine_execution.rs"]
mod execution_impl;
#[path = "../engine_sse.rs"]
mod sse_impl;

// ── REF-01 submodules (populated progressively across tasks 2–7) ────────────
pub mod stream;
pub mod approval_flow;
pub mod yaml_tool_runner;
pub mod context_builder;
pub mod tool_executor;
pub mod loop_detector_integration;

// REF-01 task 2: re-export stream submodule items so external callers keep
// resolving `crate::agent::engine::{ProcessingPhase, StreamEvent}`.
// `ProcessingGuard` / `AUTO_CONTINUE_NUDGE` stay visible to the `#[path]`-included
// engine_execution.rs / engine_sse.rs leaves via `pub(super) use`.
pub use self::stream::{ProcessingPhase, StreamEvent};
pub(crate) use self::stream::ProcessingGuard;

// REF-01 task 3: re-export ApprovalResult so `super::engine::ApprovalResult`
// keeps resolving for `approval_manager.rs` and external callers.
pub use self::approval_flow::ApprovalResult;

// REF-01 task 4: re-export SecretsEnvResolver so pipeline::context and
// pipeline::channel_actions keep resolving via `crate::agent::engine::SecretsEnvResolver`.
pub(crate) use self::yaml_tool_runner::SecretsEnvResolver;

// REF-01 task 4: re-export CACHEABLE_SEARCH_TOOLS so engine_dispatch.rs (a
// `#[path]`-included leaf of engine) keeps seeing it via `use super::*;`.
pub(super) use self::yaml_tool_runner::CACHEABLE_SEARCH_TOOLS;

// ProcessingPhase / StreamEvent — moved to self::stream (REF-01 task 2),
// re-exported above via `pub use self::stream::{ProcessingPhase, StreamEvent}`.

/// A background process started by the `process_start` tool (base agents only).
#[allow(dead_code)]
pub struct BgProcess {
    pub process_id: String,
    pub command: String,
    pub log_path: String,
    pub pid: Option<u32>,
    pub started_at: std::time::Instant,
}

// Step C complete: 6 runtime fields removed — accessed via self.state().
pub struct AgentEngine {
    /// Context builder — builds session/messages/tools for each LLM call.
    /// Initialized via `set_context_builder` after engine Arc creation.
    /// Holds `Arc<dyn ContextBuilder>` for testability (`MockContextBuilder` in plan 02).
    pub context_builder: OnceLock<Arc<dyn crate::agent::context_builder::ContextBuilder>>,
    /// Tool executor — owns tool-only state (sandbox, caches, subagent registry, etc.).
    /// Stored as concrete `Arc<DefaultToolExecutor>` for direct field access in engine methods.
    /// Initialized via `set_tool_executor` after engine Arc creation.
    pub tool_executor: OnceLock<Arc<crate::agent::tool_executor::DefaultToolExecutor>>,
    /// Per-agent mutable state (cancel/drain for shutdown, runtime fields).
    pub state: Arc<crate::agent::agent_state::AgentState>,
    /// Immutable agent configuration snapshot — sole source for agent settings,
    /// DB pool, provider, tools, memory, etc.
    pub cfg: Option<Arc<crate::agent::agent_config::AgentConfig>>,
}

/// Snapshot of what's currently displayed on the canvas.
#[derive(Debug, Clone)]
pub struct CanvasContent {
    pub content_type: String,
    pub content: String,
    pub title: Option<String>,
}


/// Maximum canvas content size (5 MB) to protect constrained environments.
pub(crate) const CANVAS_MAX_BYTES: usize = 5 * 1024 * 1024;

/// In-band marker prefix for rich card tool results.
pub(crate) const RICH_CARD_PREFIX: &str = "__rich_card__:";

/// In-band marker prefix for file/media tool results (image, audio, etc.).
/// Format: `__file__:{"url":"...","mediaType":"image/png"}`
pub(crate) const FILE_PREFIX: &str = "__file__:";

/// Nudge message injected when auto-continue detects incomplete LLM response.
const AUTO_CONTINUE_NUDGE: &str = "[system] You described remaining steps but didn't execute them. Continue and complete the task using tools.";

// CACHEABLE_SEARCH_TOOLS + search_cache_key() — moved to self::yaml_tool_runner (REF-01 task 4).

// ApprovalResult — moved to self::approval_flow (REF-01 task 3), re-exported
// above via `pub use self::approval_flow::ApprovalResult` so
// `approval_manager.rs` keeps importing it via `super::engine::ApprovalResult`.

// ProcessingGuard — moved to self::stream (REF-01 task 2), re-exported above
// via `pub(super) use self::stream::ProcessingGuard` so engine_execution.rs /
// engine_sse.rs keep seeing it through their `use super::*;`.

use crate::agent::session_manager::{SessionLifecycleGuard, SessionManager};

/// Convert a DB `MessageRow` into a typed Message.
/// Parses `tool_calls` JSON exactly once per row (ENG-02).
pub(crate) fn row_to_message(row: &crate::db::sessions::MessageRow) -> Message {
    let tool_calls = row.tool_calls.as_ref().and_then(|tc| {
        serde_json::from_value::<Vec<hydeclaw_types::ToolCall>>(tc.clone()).ok()
    });
    let thinking_blocks = row.thinking_blocks.as_ref()
        .and_then(|tb| serde_json::from_value::<Vec<hydeclaw_types::ThinkingBlock>>(tb.clone()).ok())
        .unwrap_or_default();
    Message {
        role: match row.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => MessageRole::User,
        },
        content: row.content.clone(),
        tool_calls,
        tool_call_id: row.tool_call_id.clone(),
        thinking_blocks,
    }
}

impl AgentEngine {
    // ── Public accessors (sealed API) ──────────────────────────────

    /// Access the immutable config snapshot.
    /// Panics if called on an engine that was not constructed with a config
    /// (should not happen for top-level engines).
    pub fn cfg(&self) -> &crate::agent::agent_config::AgentConfig {
        self.cfg
            .as_ref()
            .expect("cfg not set — engine was not constructed with AgentConfig")
    }

    /// Access the mutable per-agent state (cancel/drain, runtime fields).
    pub fn state(&self) -> &crate::agent::agent_state::AgentState {
        &self.state
    }

    /// Agent name (from config).
    pub fn name(&self) -> &str {
        &self.cfg().agent.name
    }

    /// Primary model name (from config).
    pub fn model_name(&self) -> String {
        self.cfg().agent.model.clone()
    }

    /// Borrow the database pool.
    pub fn db_pool(&self) -> &PgPool {
        &self.cfg().db
    }

    /// Clone the LLM provider Arc for use outside the engine.
    pub fn provider_arc(&self) -> Arc<dyn LlmProvider> {
        self.cfg().provider.clone()
    }

    /// Read the current channel formatting prompt.
    pub async fn formatting_prompt(&self) -> Option<String> {
        self.state().channel_formatting_prompt.read().await.clone()
    }

    /// Borrow the channel action router, if configured.
    pub fn channel_router_ref(&self) -> Option<&ChannelActionRouter> {
        self.state().channel_router.as_ref()
    }

    /// Borrow the agent access config, if set.
    pub fn agent_access(&self) -> Option<&crate::config::AgentAccessConfig> {
        self.cfg().agent.access.as_ref()
    }

    /// Delegate model override to the underlying provider.
    pub fn set_model_override(&self, model: Option<String>) {
        self.cfg().provider.set_model_override(model);
    }

    /// Return the current active model name from the provider.
    pub fn current_model(&self) -> String {
        self.cfg().provider.current_model()
    }

    // ── Lifecycle ──────────────────────────────────────────────────

    /// Initialize the context builder after engine Arc creation.
    /// Must be called once after engine Arc creation.
    /// Uses `Weak<dyn ContextBuilderDeps>` to break Arc reference cycle.
    pub fn set_context_builder(&self, arc: &Arc<AgentEngine>) {
        use crate::agent::context_builder::{ContextBuilderDeps, DefaultContextBuilder};
        let deps_strong = arc.clone() as Arc<dyn ContextBuilderDeps>;
        let deps_weak = Arc::downgrade(&deps_strong);
        let builder = Arc::new(DefaultContextBuilder::new(deps_weak))
            as Arc<dyn crate::agent::context_builder::ContextBuilder>;
        let _ = self.context_builder.set(builder);
    }

    /// Initialize the tool executor after engine Arc creation.
    /// Accepts a pre-built Arc<DefaultToolExecutor> constructed in agents.rs with migrated fields.
    pub fn set_tool_executor(&self, executor: Arc<crate::agent::tool_executor::DefaultToolExecutor>) {
        use crate::agent::tool_executor::ToolExecutor;
        let executor_trait: Arc<dyn ToolExecutor> = executor.clone();
        executor.set_self_ref(&executor_trait);
        let _ = self.tool_executor.set(executor);
    }

    // ── Proxy accessors for fields migrated to DefaultToolExecutor ────────────
    // Engine sub-modules (engine_*.rs) and providers_*.rs use these to access
    // the migrated fields without direct struct field access.

    #[inline]
    pub(crate) fn tex(&self) -> &crate::agent::tool_executor::DefaultToolExecutor {
        self.tool_executor.get().expect("tool_executor not initialized")
    }

    /// Sandbox accessor — delegates to `DefaultToolExecutor`.
    #[inline]
    pub(crate) fn sandbox(&self) -> &Option<Arc<crate::containers::sandbox::CodeSandbox>> {
        &self.tex().sandbox
    }

    /// SSRF-safe HTTP client accessor — delegates to `DefaultToolExecutor`.
    #[inline]
    pub(crate) fn ssrf_http_client(&self) -> &reqwest::Client {
        &self.tex().ssrf_http_client
    }

    /// Tool embed cache accessor — delegates to `DefaultToolExecutor`.
    #[inline]
    pub(crate) fn tool_embed_cache(&self) -> &Arc<crate::tools::embedding::ToolEmbeddingCache> {
        &self.tex().tool_embed_cache
    }

    /// Subagent registry accessor — delegates to `DefaultToolExecutor`.
    #[inline]
    pub(crate) fn subagent_registry(&self) -> &crate::agent::subagent_state::SubagentRegistry {
        &self.tex().subagent_registry
    }

    /// OAuth manager accessor — delegates to `DefaultToolExecutor`.
    #[inline]
    pub(crate) fn oauth(&self) -> &Option<Arc<crate::oauth::OAuthManager>> {
        &self.tex().oauth
    }

    /// Secrets vault accessor — delegates to `DefaultToolExecutor`.
    #[inline]
    pub(crate) fn secrets(&self) -> &Arc<crate::secrets::SecretsManager> {
        &self.tex().secrets
    }

    /// MCP registry accessor — delegates to `DefaultToolExecutor`.
    #[inline]
    pub(crate) fn mcp(&self) -> &Option<Arc<McpRegistry>> {
        &self.tex().mcp
    }

    /// Standard HTTP client accessor — delegates to `DefaultToolExecutor`.
    #[inline]
    pub(crate) fn http_client(&self) -> &reqwest::Client {
        &self.tex().http_client
    }

    /// Hooks registry accessor — delegates to `DefaultToolExecutor`.
    #[inline]
    pub(crate) fn hooks(&self) -> &Arc<super::hooks::HookRegistry> {
        &self.tex().hooks
    }

    /// SSE event TX accessor — delegates to `DefaultToolExecutor`.
    ///
    /// Phase 62 RES-01: stores an `EngineEventSender` (bounded-channel wrapper
    /// that enforces text-delta-droppable / non-text-never-dropped contract).
    #[inline]
    pub(crate) fn sse_event_tx(&self) -> &Arc<tokio::sync::Mutex<Option<crate::agent::engine_event_sender::EngineEventSender>>> {
        &self.tex().sse_event_tx
    }

    // invalidate_yaml_tools_cache / check_search_cache / store_search_cache
    // — moved to self::yaml_tool_runner (REF-01 task 4).

    /// Broadcast a UI event to connected WebSocket clients.
    fn broadcast_ui_event(&self, event: serde_json::Value) {
        if let Some(ref tx) = self.state().ui_event_tx {
            tx.send(event.to_string()).ok();
        }
    }

    // needs_approval() + resolve_approval() — moved to self::approval_flow (REF-01 task 3).


    /// Check if an enabled YAML tool exists in workspace/tools/ (shared tools).
    async fn has_tool(&self, name: &str) -> bool {
        let dir = std::path::Path::new(&self.cfg().workspace_dir).join("tools");
        let path = dir.join(format!("{name}.yaml"));
        let path = if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            path
        } else {
            let yml = dir.join(format!("{name}.yml"));
            if !tokio::fs::try_exists(&yml).await.unwrap_or(false) {
                return false;
            }
            yml
        };
        // Disabled tools should not count as available
        tokio::fs::read_to_string(&path)
            .await
            .map(|c| !c.contains("\nstatus: disabled"))
            .unwrap_or(false)
    }

    /// Trim session messages if `max_messages` is configured.
    async fn maybe_trim_session(&self, session_id: Uuid) {
        if let Some(max) = self.cfg().agent.session.as_ref().and_then(|s| {
            if s.max_messages > 0 { Some(s.max_messages) } else { None }
        }) {
            let sm = SessionManager::new(self.cfg().db.clone());
            if let Err(e) = sm.trim_messages(session_id, max).await {
                tracing::warn!(error = %e, "failed to trim session messages");
            }
        }
    }

    // handle() + handle_isolated() — moved to self::stream (REF-01 task 2).
    //
    // runtime_context / get_channel_info / invalidate_channel_cache /
    // load_channel_info_from_db / build_memory_context / index_facts_to_memory /
    // build_context / compact_tool_results / compaction_params / compact_messages /
    // compact_session / handle_command — moved to self::context_builder (REF-01 task 5).

    // ── Tool definitions (from engine_tool_defs.rs) ──────────────────────────

    /// Resolve tool group settings (from agent config or defaults).
    pub(super) fn tool_groups(&self) -> &crate::config::ToolGroups {
        crate::agent::pipeline::tool_defs::resolve_tool_groups(self.cfg().agent.tools.as_ref())
    }

    /// Return tool definitions for internal tools available to the LLM.
    pub(super) fn internal_tool_definitions(&self) -> Vec<ToolDefinition> {
        let browser_url = crate::agent::pipeline::canvas::browser_renderer_url();
        let ctx = crate::agent::pipeline::tool_defs::ToolDefsContext {
            is_base: self.cfg().agent.base,
            groups: self.tool_groups(),
            default_timezone: &self.cfg().default_timezone,
            has_sandbox: self.sandbox().is_some(),
            browser_renderer_url: &browser_url,
        };
        crate::agent::pipeline::tool_defs::build_internal_tool_definitions(&ctx)
    }

    /// Internal tool definitions filtered for subagent use.
    pub(super) fn internal_tool_definitions_for_subagent(
        &self,
        allowed_tools: Option<&[String]>,
    ) -> Vec<hydeclaw_types::ToolDefinition> {
        crate::agent::pipeline::tool_defs::filter_for_subagent(
            self.internal_tool_definitions(),
            crate::agent::pipeline::subagent::SUBAGENT_DENIED_TOOLS,
            allowed_tools,
        )
    }
}

/// All system (internal) tool names — single source of truth.
pub fn all_system_tool_names() -> &'static [&'static str] {
    crate::agent::pipeline::tool_defs::all_system_tool_names()
}

// ── Extracted submodules ─────────────────────────────────────────────────────
#[path = "../engine_dispatch.rs"]
mod dispatch_impl;

// ── ContextBuilderDeps impl ───────────────────────────────────────────────────

// ContextBuilderDeps for AgentEngine — moved to self::context_builder (REF-01 task 5).

// ── ToolExecutorDeps impl ─────────────────────────────────────────────────────

#[async_trait::async_trait]
impl crate::agent::tool_executor::ToolExecutorDeps for AgentEngine {
    async fn execute_tool_calls_partitioned_raw(
        &self,
        tool_calls: &[hydeclaw_types::ToolCall],
        context: &serde_json::Value,
        session_id: Uuid,
        channel: &str,
        current_context_chars: usize,
        detector: &mut crate::agent::tool_loop::LoopDetector,
        detect_loops: bool,
    ) -> Result<Vec<(String, String)>, LoopBreak> {
        self.execute_tool_calls_partitioned(
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

// ── Inlined from engine_parallel.rs ──────────────────────────────────────────

impl crate::agent::pipeline::parallel::ToolExecutor for AgentEngine {
    fn execute_tool_call<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        self.execute_tool_call(name, arguments)
    }

    fn needs_approval(&self, tool_name: &str) -> bool {
        self.needs_approval(tool_name)
    }
}

impl AgentEngine {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_tool_calls_partitioned(
        &self,
        tool_calls: &[hydeclaw_types::ToolCall],
        context: &serde_json::Value,
        session_id: Uuid,
        channel: &str,
        current_context_chars: usize,
        detector: &mut LoopDetector,
        detect_loops: bool,
    ) -> Result<Vec<(String, String)>, LoopBreak> {
        // Load YAML tools (cached for 30s)
        let yaml_tools: std::sync::Arc<std::collections::HashMap<String, crate::tools::yaml_tools::YamlToolDef>> = {
            let cache = self.tex().yaml_tools_cache.read().await;
            if cache.0.elapsed() < std::time::Duration::from_secs(30) && !cache.1.is_empty() {
                std::sync::Arc::clone(&cache.1)
            } else {
                drop(cache);
                let tools = std::sync::Arc::new(
                    crate::tools::yaml_tools::load_yaml_tools(&self.cfg().workspace_dir, false)
                        .await
                        .into_iter()
                        .map(|t| (t.name.clone(), t))
                        .collect::<std::collections::HashMap<String, crate::tools::yaml_tools::YamlToolDef>>(),
                );
                *self.tex().yaml_tools_cache.write().await =
                    (std::time::Instant::now(), std::sync::Arc::clone(&tools));
                tools
            }
        };

        let subagent_timeout =
            parse_subagent_timeout(&self.cfg().app_config.subagents.in_process_timeout)
                + std::time::Duration::from_secs(10);

        crate::agent::pipeline::parallel::execute_tool_calls_partitioned(
            tool_calls,
            context,
            session_id,
            channel,
            &self.cfg().agent.model,
            current_context_chars,
            detector,
            detect_loops,
            &self.cfg().db,
            &self.cfg().embedder,
            &yaml_tools,
            subagent_timeout,
            self,
        )
        .await
    }
}

// ── Inlined from engine_provider.rs ──────────────────────────────────────────

/// `AgentEngine` acts as its own compactor — delegates to `compact_messages`.
#[async_trait::async_trait]
impl crate::agent::pipeline::llm_call::Compactor for AgentEngine {
    async fn compact(&self, messages: &mut Vec<Message>) {
        self.compact_messages(messages, None).await;
    }
}

impl AgentEngine {
    /// Build tool loop config from agent TOML settings (or defaults).
    pub(crate) fn tool_loop_config(&self) -> crate::agent::tool_loop::ToolLoopConfig {
        self.cfg().agent
            .tool_loop
            .as_ref()
            .map(crate::agent::tool_loop::ToolLoopConfig::from)
            .unwrap_or_default()
    }

    /// Create fallback LLM provider from agent config.
    pub(super) async fn create_fallback_provider(&self) -> Option<Arc<dyn crate::agent::providers::LlmProvider>> {
        crate::agent::pipeline::llm_call::create_fallback_provider(
            &self.cfg().db,
            self.cfg().agent.fallback_provider.as_deref(),
            &self.cfg().agent.name,
            self.cfg().agent.temperature,
            self.cfg().agent.max_tokens,
            self.secrets().clone(),
            self.sandbox().clone(),
            &self.cfg().workspace_dir,
            self.cfg().agent.base,
        )
        .await
    }

    /// Check daily token budget before LLM call.
    pub(super) async fn check_budget(&self) -> Result<()> {
        crate::agent::pipeline::llm_call::check_budget(
            &self.cfg().db,
            &self.cfg().agent.name,
            self.cfg().agent.daily_budget_tokens,
        )
        .await
    }

    /// Call LLM with automatic context overflow recovery.
    pub(super) async fn chat_with_overflow_recovery(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_with_overflow_recovery(
            self.cfg().provider.as_ref(),
            messages,
            tools,
            self,
        )
        .await
    }

    /// Call LLM with exponential backoff retry.
    pub(super) async fn chat_with_transient_retry(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_with_transient_retry(
            self.cfg().provider.as_ref(),
            messages,
            tools,
            self,
        )
        .await
    }

    /// Streaming variant of chat_with_overflow_recovery.
    #[allow(dead_code)]
    pub(super) async fn chat_stream_with_overflow_recovery(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_stream_with_overflow_recovery(
            self.cfg().provider.as_ref(),
            messages,
            tools,
            chunk_tx,
            self,
        )
        .await
    }

    /// Streaming variant of chat_with_transient_retry.
    pub(super) async fn chat_stream_with_transient_retry(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_stream_with_transient_retry(
            self.cfg().provider.as_ref(),
            messages,
            tools,
            chunk_tx,
            self,
        )
        .await
    }

    /// Variant that uses an explicit provider (for fallback switching).
    pub(super) async fn chat_with_transient_retry_using(
        &self,
        provider: &Arc<dyn crate::agent::providers::LlmProvider>,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_with_transient_retry_using(
            provider,
            messages,
            tools,
            self,
        )
        .await
    }

    /// Streaming variant of chat_with_transient_retry_using.
    pub(super) async fn chat_stream_with_transient_retry_using(
        &self,
        provider: &Arc<dyn crate::agent::providers::LlmProvider>,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_stream_with_transient_retry_using(
            provider,
            messages,
            tools,
            chunk_tx,
            self,
        )
        .await
    }

    /// Fire-and-forget audit event recording.
    pub(super) fn audit(&self, event_type: &'static str, actor: Option<&str>, details: serde_json::Value) {
        crate::agent::pipeline::llm_call::audit(
            self.cfg().db.clone(),
            self.cfg().agent.name.clone(),
            event_type,
            actor,
            details,
        );
    }

    // ── OpenAI-compatible API handler ───────────────────────────────────────

    pub async fn handle_openai(
        &self,
        openai_messages: &[crate::gateway::OpenAiMessage],
        chunk_tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        let ctx = crate::agent::pipeline::CommandContext { cfg: self.cfg(), state: self.state(), tex: self.tex() };
        crate::agent::pipeline::openai_compat::handle_openai(&ctx, self, openai_messages, chunk_tx).await
    }
}

// ── Thin wrappers delegating to pipeline free functions (Phase 2) ─────────────

impl AgentEngine {
    pub(super) async fn handle_message_action(&self, args: &serde_json::Value) -> String {
        let ctx = crate::agent::pipeline::CommandContext { cfg: self.cfg(), state: self.state(), tex: self.tex() };
        crate::agent::pipeline::channel_actions::handle_message_action(&ctx, args).await
    }

    pub async fn send_channel_message(&self, channel: &str, chat_id: i64, text: &str) -> anyhow::Result<()> {
        let ctx = crate::agent::pipeline::CommandContext { cfg: self.cfg(), state: self.state(), tex: self.tex() };
        crate::agent::pipeline::channel_actions::send_channel_message(&ctx, channel, chat_id, text).await
    }

    pub(super) async fn execute_yaml_channel_action(
        &self,
        tool: &crate::tools::yaml_tools::YamlToolDef,
        args: &serde_json::Value,
        ca: &crate::tools::yaml_tools::ChannelActionConfig,
    ) -> String {
        let ctx = crate::agent::pipeline::CommandContext { cfg: self.cfg(), state: self.state(), tex: self.tex() };
        crate::agent::pipeline::channel_actions::execute_yaml_channel_action(&ctx, tool, args, ca).await
    }

    pub(super) async fn handle_cron(&self, args: &serde_json::Value) -> String {
        let ctx = crate::agent::pipeline::CommandContext { cfg: self.cfg(), state: self.state(), tex: self.tex() };
        crate::agent::pipeline::cron::handle_cron(&ctx, args).await
    }
}

// ── Thin wrappers delegating to pipeline::subagent_runner (Phase 2) ───────────

impl AgentEngine {
    pub async fn run_subagent(
        &self,
        task: &str,
        max_iterations: usize,
        deadline: Option<std::time::Instant>,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        handle: Option<Arc<tokio::sync::RwLock<crate::agent::subagent_state::SubagentHandle>>>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<String> {
        self.run_subagent_with_session(task, max_iterations, deadline, cancel, handle, allowed_tools, None).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_subagent_with_session(
        &self,
        task: &str,
        max_iterations: usize,
        deadline: Option<std::time::Instant>,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        handle: Option<Arc<tokio::sync::RwLock<crate::agent::subagent_state::SubagentHandle>>>,
        allowed_tools: Option<Vec<String>>,
        session_id: Option<uuid::Uuid>,
    ) -> Result<String> {
        let ctx = crate::agent::pipeline::CommandContext { cfg: self.cfg(), state: self.state(), tex: self.tex() };
        crate::agent::pipeline::subagent_runner::run_subagent_with_session(
            &ctx, self, task, max_iterations, deadline, cancel, handle, allowed_tools, session_id,
        ).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // search_cache_key / CACHEABLE_SEARCH_TOOLS tests — moved to
    // self::yaml_tool_runner (REF-01 task 4).

    #[test]
    fn agent_in_system_tool_names() {
        let names = all_system_tool_names();
        assert!(names.contains(&"agent"), "agent must be in all_system_tool_names()");
        assert!(!names.contains(&"handoff"), "handoff should be removed");
        assert!(!names.contains(&"subagent"), "subagent should be removed");
    }
}

