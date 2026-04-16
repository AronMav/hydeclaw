use anyhow::Result;
use hydeclaw_types::{IncomingMessage, Message, MessageRole, ToolDefinition};
use sqlx::PgPool;
use std::sync::{Arc, OnceLock, Weak};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::channel_actions::{ChannelAction, ChannelActionRouter};
use super::providers::LlmProvider;
use super::workspace;
use crate::scheduler::{compute_next_run, Scheduler};
use crate::mcp::McpRegistry;

use super::error_classify;
use super::thinking::{looks_incomplete, maybe_strip_thinking, strip_thinking};
use super::tool_loop::LoopDetector;


// Extracted impl AgentEngine blocks (submodules of engine for full super:: access)
#[path = "engine_handlers.rs"]
mod handlers_impl;
#[path = "engine_subagent.rs"]
mod subagent_impl;
pub use crate::agent::pipeline::parallel::LoopBreak;
pub(crate) use subagent_impl::parse_subagent_timeout;
#[path = "engine_execution.rs"]
mod execution_impl;
#[path = "engine_sse.rs"]
mod sse_impl;

/// Resolves env var names through `SecretsManager` (scoped to agent).
pub(crate) struct SecretsEnvResolver {
    pub(crate) secrets: Arc<crate::secrets::SecretsManager>,
    pub(crate) agent_name: String,
}

#[async_trait::async_trait]
impl crate::tools::yaml_tools::EnvResolver for SecretsEnvResolver {
    async fn resolve(&self, key: &str) -> Option<String> {
        self.secrets.get_scoped(key, &self.agent_name).await
    }
}

/// Status phases emitted during message processing.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ProcessingPhase {
    Thinking,
    CallingTool(String),
    Composing,
}

impl ProcessingPhase {
    /// Convert to wire format: (`phase_name`, `optional_tool_name`).
    pub fn to_wire(&self) -> (String, Option<String>) {
        match self {
            ProcessingPhase::Thinking => ("thinking".to_string(), None),
            ProcessingPhase::CallingTool(name) => ("calling_tool".to_string(), Some(name.clone())),
            ProcessingPhase::Composing => ("composing".to_string(), None),
        }
    }
}

/// Events emitted during SSE streaming (AI SDK UI Message Stream Protocol v1).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum StreamEvent {
    /// Session ID resolved/created by `build_context` — emitted first so the UI can track it.
    SessionId(String),
    MessageStart { message_id: String },
    StepStart { step_id: String },
    TextDelta(String),
    ToolCallStart { id: String, name: String },
    ToolCallArgs { id: String, args_text: String },
    ToolResult { id: String, result: String },
    StepFinish { step_id: String, finish_reason: String },
    /// Rich card embedded inline in the message stream (tables, metrics, etc.).
    RichCard { card_type: String, data: serde_json::Value },
    /// File/media attachment (image, audio, etc.) — displayed inline in UI chat.
    File { url: String, media_type: String },
    Finish { finish_reason: String, continuation: bool },
    /// Approval needed: a tool call is waiting for human approval.
    ApprovalNeeded {
        approval_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
        timeout_ms: u64,
    },
    /// Approval resolved: a pending approval was approved, rejected, or timed out.
    ApprovalResolved {
        approval_id: String,
        action: String, // "approved" | "rejected" | "timeout_rejected"
        modified_input: Option<serde_json::Value>,
    },
    /// Internal event: signals that a different agent is now responding (multi-agent session).
    /// Converter task updates `current_responding_agent`; no SSE is emitted to the client.
    /// Retained for API compatibility — not currently emitted.
    AgentSwitch { agent_name: String },
    Error(String),
}

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
    /// `None` for subagent engines — they are lightweight copies without lifecycle tracking.
    pub state: Option<Arc<crate::agent::agent_state::AgentState>>,
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

/// YAML tools whose results are cached per-engine to avoid duplicate HTTP calls.
const CACHEABLE_SEARCH_TOOLS: &[&str] = &["searxng_search", "brave_search"];

/// Hash a search query for cache lookup (case-insensitive).
fn search_cache_key(query: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    query.to_lowercase().hash(&mut h);
    h.finish()
}

/// Result of a tool-call approval request.
#[derive(Debug)]
pub enum ApprovalResult {
    Approved,
    ApprovedWithModifiedArgs(serde_json::Value),
    Rejected(String),
}

/// RAII guard: inserts into processing tracker on creation, removes + broadcasts "end" on drop.
/// Uses `session_id` as tracker key (not `agent_name`) to support concurrent sessions per agent.
struct ProcessingGuard {
    tx: Option<tokio::sync::broadcast::Sender<String>>,
    processing_tracker: Option<crate::gateway::ProcessingTracker>,
    agent_name: String,
    /// Tracker key — `session_id` for unique identification across concurrent sessions.
    tracker_key: String,
    session_id: Option<String>,
}

impl ProcessingGuard {
    fn new(
        tx: Option<tokio::sync::broadcast::Sender<String>>,
        tracker: Option<crate::gateway::ProcessingTracker>,
        agent_name: String,
        start_event: &serde_json::Value,
    ) -> Self {
        let session_id = start_event.get("session_id").and_then(|v| v.as_str()).map(std::string::ToString::to_string);
        // Use session_id as key (supports multiple concurrent sessions for same agent).
        // Fallback to agent_name if session_id is missing (shouldn't happen).
        let tracker_key = session_id.clone().unwrap_or_else(|| agent_name.clone());
        if let Some(ref t) = tracker
            && let Ok(mut map) = t.write() {
                map.insert(tracker_key.clone(), start_event.clone());
                tracing::debug!(agent = %agent_name, key = %tracker_key, "processing_tracker: inserted");
            }
        Self { tx, processing_tracker: tracker, agent_name, tracker_key, session_id }
    }
}

impl Drop for ProcessingGuard {
    fn drop(&mut self) {
        if let Some(ref tracker) = self.processing_tracker
            && let Ok(mut map) = tracker.write() {
                map.remove(&self.tracker_key);
            }
        if let Some(ref tx) = self.tx {
            let mut event = serde_json::json!({
                "type": "agent_processing",
                "agent": self.agent_name,
                "status": "end",
            });
            if let Some(ref sid) = self.session_id {
                event["session_id"] = serde_json::Value::String(sid.clone());
            }
            tx.send(event.to_string()).ok();
        }
    }
}

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
    /// Panics if called on an engine without AgentState (subagent lightweight copies).
    pub fn state(&self) -> &crate::agent::agent_state::AgentState {
        self.state
            .as_ref()
            .expect("state not set — engine was not constructed with AgentState")
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
    #[inline]
    pub(crate) fn sse_event_tx(&self) -> &Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<StreamEvent>>>> {
        &self.tex().sse_event_tx
    }

    /// Invalidate the cached YAML tool definitions so the next request reloads from disk.
    pub(crate) async fn invalidate_yaml_tools_cache(&self) {
        *self.tex().yaml_tools_cache.write().await = (
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(60)).unwrap(),
            std::sync::Arc::new(std::collections::HashMap::new()),
        );
    }

    pub(crate) async fn check_search_cache(&self, query: &str) -> Option<String> {
        let cache = self.tex().search_cache.read().await;
        if let Some((result, expiry)) = cache.get(&search_cache_key(query))
            && *expiry > std::time::Instant::now()
        {
            tracing::debug!(query, "search cache hit");
            return Some(result.clone());
        }
        None
    }

    pub(crate) async fn store_search_cache(&self, query: &str, result: &str) {
        let mut cache = self.tex().search_cache.write().await;
        cache.insert(search_cache_key(query), (
            result.to_string(),
            std::time::Instant::now() + std::time::Duration::from_secs(300),
        ));
        if cache.len() > 100 {
            let now = std::time::Instant::now();
            cache.retain(|_, (_, exp)| *exp > now);
        }
    }

    /// Broadcast a UI event to connected WebSocket clients.
    fn broadcast_ui_event(&self, event: serde_json::Value) {
        if let Some(ref tx) = self.state().ui_event_tx {
            tx.send(event.to_string()).ok();
        }
    }

    /// Check if a tool requires approval before execution.
    fn needs_approval(&self, tool_name: &str) -> bool {
        super::pipeline::dispatch::needs_approval(self.cfg().agent.approval.as_ref(), tool_name)
    }

    /// Resolve a pending approval (called from API/callback handler).
    pub async fn resolve_approval(&self, approval_id: Uuid, approved: bool, resolved_by: &str, modified_input: Option<serde_json::Value>) -> anyhow::Result<()> {
        let status = if approved { "approved" } else { "rejected" };
        let updated = crate::db::approvals::resolve_approval(&self.cfg().db, approval_id, status, resolved_by).await?;
        if !updated {
            anyhow::bail!("approval {approval_id} not found or already resolved");
        }

        self.audit(crate::db::audit::event_types::APPROVAL_RESOLVED, Some(resolved_by), serde_json::json!({
            "approval_id": approval_id.to_string(), "status": status
        }));

        self.broadcast_ui_event(serde_json::json!({
            "type": "approval_resolved",
            "approval_id": approval_id.to_string(),
            "agent": self.cfg().agent.name,
            "status": status,
        }));

        // Emit SSE event for inline approval resolution in chat UI
        let action_str = if approved { "approved" } else { "rejected" };
        if let Some(tx) = self.sse_event_tx().lock().await.as_ref() {
            tx.send(StreamEvent::ApprovalResolved {
                approval_id: approval_id.to_string(),
                action: action_str.to_string(),
                modified_input: modified_input.clone(),
            }).ok();
        }

        // Wake up the waiting tool execution
        let mut waiters = self.cfg().approval_manager.waiters().write().await;
        if let Some((tx, _created_at)) = waiters.remove(&approval_id) {
            let result = if approved {
                match modified_input {
                    Some(args) => ApprovalResult::ApprovedWithModifiedArgs(args),
                    None => ApprovalResult::Approved,
                }
            } else {
                ApprovalResult::Rejected(format!("rejected by {resolved_by}"))
            };
            tx.send(result).ok();
        }

        // Opportunistic cleanup: remove stale waiters (>5 min old, oneshot already dropped)
        let stale_threshold = std::time::Duration::from_secs(300);
        waiters.retain(|id, (_tx, created_at)| {
            let stale = created_at.elapsed() > stale_threshold;
            if stale {
                tracing::debug!(approval_id = %id, "cleaning up stale approval waiter");
            }
            !stale
        });

        Ok(())
    }


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

    /// Handle an incoming message: build context, call LLM, execute tools, return response.
    pub async fn handle(&self, msg: &IncomingMessage) -> Result<String> {
        self.handle_with_status(msg, None, None).await
    }

    /// Handle a message in a fully isolated session (no history from previous runs).
    /// Used by cron dynamic jobs to prevent context accumulation across invocations.
    pub async fn handle_isolated(&self, msg: &IncomingMessage) -> Result<String> {
        // Hook: BeforeMessage
        if let super::hooks::HookAction::Block(reason) = self.hooks().fire(&super::hooks::HookEvent::BeforeMessage) {
            anyhow::bail!("blocked by hook: {reason}");
        }

        let sm = SessionManager::new(self.cfg().db.clone());
        let session_id = sm.create_isolated(&self.cfg().agent.name, &msg.user_id, &msg.channel).await?;

        let ctx = self.build_context(msg, true, Some(session_id), false).await?;
        let mut messages = ctx.messages;
        let mut available_tools = ctx.tools;
        // session_id already bound above (create_isolated result)

        // Apply cron job tool policy override if present
        if let Some(ref policy_json) = msg.tool_policy_override
            && let Ok(override_policy) = serde_json::from_value::<crate::config::AgentToolPolicy>(policy_json.clone()) {
                let before = available_tools.len();
                available_tools = self.apply_tool_policy_override(available_tools, &override_policy);
                if available_tools.len() != before {
                    tracing::info!(
                        agent = %self.cfg().agent.name,
                        before,
                        after = available_tools.len(),
                        "cron tool policy override applied"
                    );
                }
            }

        // invite_agent removed (v3.0) — agent is the inter-agent tool

        let user_text = msg.text.clone().unwrap_or_default();
        let enriched_text = self.enrich_message_text(&user_text, &msg.attachments).await;

        messages.push(Message {
            role: MessageRole::User,
            content: enriched_text,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        });

        // For inter-agent messages (user_id starts with "agent:"), save the sender agent_id
        let sender_agent_id = if msg.user_id.starts_with("agent:") { Some(msg.user_id.trim_start_matches("agent:")) } else { None };
        sm.save_message_ex(session_id, "user", &user_text, None, None, sender_agent_id, None, None).await?;

        // Context compaction if needed (model-aware token budget)
        self.compact_messages(&mut messages, None).await;

        // LLM loop (with tool calls)
        let mut final_response = String::new();
        let loop_config = self.tool_loop_config();
        let mut detector = LoopDetector::new(&loop_config);
        let mut loop_nudge_count: usize = 0;
        let mut did_reset_session = false;
        let mut empty_retry_count: u8 = 0;
        let mut auto_continue_count: u8 = 0;
        let mut context_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        let mut consecutive_failures: usize = 0;
        let mut using_fallback = false;
        let mut fallback_provider: Option<Arc<dyn super::providers::LlmProvider>> = None;

        for iteration in 0..loop_config.effective_max_iterations() {
            self.compact_tool_results(&mut messages, &mut context_chars);
            let llm_result = if let Some(ref fb) = fallback_provider {
                self.chat_with_transient_retry_using(fb, &mut messages, &available_tools).await
            } else {
                self.chat_with_transient_retry(&mut messages, &available_tools).await
            };
            let response = match llm_result {
                Ok(r) => {
                    consecutive_failures = 0;
                    r
                }
                Err(e) => {
                    if error_classify::classify(&e) == error_classify::LlmErrorClass::SessionCorruption && !did_reset_session {
                        did_reset_session = true;
                        tracing::warn!(error = %e, "session corrupted, resetting context");
                        messages.retain(|m| m.role == MessageRole::System);
                        messages.push(Message { role: MessageRole::User, content: user_text.clone(), tool_calls: None, tool_call_id: None, thinking_blocks: vec![] });
                        context_chars = messages.iter().map(|m| m.content.chars().count()).sum();
                        continue;
                    }
                    consecutive_failures += 1;
                    if !using_fallback && consecutive_failures >= loop_config.max_consecutive_failures {
                        if fallback_provider.is_none() {
                            fallback_provider = self.create_fallback_provider().await;
                        }
                        if fallback_provider.is_some() {
                            using_fallback = true;
                            consecutive_failures = 0;
                            tracing::warn!(
                                agent = %self.cfg().agent.name,
                                iteration,
                                "switching to fallback provider after consecutive failures"
                            );
                            continue;
                        }
                    }
                    tracing::error!(error = %e, iteration, "isolated LLM call failed, returning fallback");
                    self.hooks().fire(&super::hooks::HookEvent::OnError);
                    final_response = error_classify::format_user_error(&e);
                    break;
                }
            };
            self.record_usage(&response, Some(session_id));

            if response.tool_calls.is_empty() {
                final_response = strip_thinking(&response.content);

                // Auto-continue: if LLM described remaining work, nudge it to execute
                if auto_continue_count < loop_config.max_auto_continues && !final_response.is_empty() && looks_incomplete(&final_response) {
                    auto_continue_count += 1;
                    tracing::info!(iteration, count = auto_continue_count, max = loop_config.max_auto_continues, "auto-continue: response looks incomplete, nudging LLM");
                    {
                        let db = self.cfg().db.clone();
                        let agent_name = self.cfg().agent.name.clone();
                        let cnt = auto_continue_count;
                        let max = loop_config.max_auto_continues;
                        if let Some(ref ui_tx) = self.state().ui_event_tx {
                            let tx = ui_tx.clone();
                            tokio::spawn(async move {
                                crate::gateway::notify(
                                    &db, &tx, "auto_continue",
                                    &format!("Auto-continue: {agent_name}"),
                                    &format!("Agent continued unfinished task (attempt {cnt}/{max})"),
                                    serde_json::json!({"agent": agent_name}),
                                ).await.ok();
                            });
                        }
                    }
                    messages.push(Message {
                        role: MessageRole::User,
                        content: AUTO_CONTINUE_NUDGE.to_string(),
                        tool_calls: None,
                        tool_call_id: None,
                        thinking_blocks: vec![],
                    });
                    context_chars += AUTO_CONTINUE_NUDGE.len(); // all ASCII
                    continue;
                }

                if final_response.is_empty() && empty_retry_count < 1 {
                    empty_retry_count += 1;
                    tracing::warn!(iteration, "LLM returned empty response, retrying once");
                    continue;
                }
                if final_response.is_empty() {
                    tracing::warn!(iteration, "LLM returned empty response after retry");
                }
                break;
            }

            tracing::info!(
                iteration,
                max = loop_config.effective_max_iterations(),
                tools = response.tool_calls.len(),
                "isolated job: executing tool calls"
            );

            let cleaned_content = strip_thinking(&response.content);

            messages.push(Message {
                role: MessageRole::Assistant,
                content: cleaned_content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
                thinking_blocks: vec![],
            });
            context_chars += cleaned_content.chars().count();

            // Save assistant message with tool_calls to DB
            let tc_json = serde_json::to_value(&response.tool_calls).ok();
            if let Err(e) = sm.save_message(
                session_id, "assistant", &cleaned_content,
                tc_json.as_ref(), None,
            ).await {
                tracing::warn!(error = %e, session_id = %session_id, "failed to save assistant message to DB");
            }

            let loop_broken = match self.execute_tool_calls_partitioned(
                &response.tool_calls, &msg.context, session_id, &msg.channel,
                messages.iter().map(|m| m.content.len()).sum(),
                &mut detector, loop_config.detect_loops,
            ).await {
                Ok(results) => {
                    for (tc_id, tool_result) in &results {
                        messages.push(Message {
                            role: MessageRole::Tool,
                            content: tool_result.clone(),
                            tool_calls: None,
                            tool_call_id: Some(tc_id.clone()),
                            thinking_blocks: vec![],
                        });
                        context_chars += tool_result.chars().count();
                        if let Err(e) = sm.save_message(
                            session_id, "tool", tool_result, None, Some(tc_id),
                        ).await {
                            tracing::warn!(error = %e, session_id = %session_id, "failed to save tool result to DB");
                        }
                    }
                    false
                }
                Err(LoopBreak(reason)) => {
                    if loop_nudge_count < loop_config.max_loop_nudges {
                        let nudge_desc = reason.as_deref().unwrap_or("repeating pattern");
                        let nudge_msg = format!(
                            "LOOP DETECTED: You have repeated the same sequence of actions ({nudge_desc}). \
                             Change your approach entirely. If the task is too large for a single session, \
                             tell the user and suggest breaking it into smaller steps. Do NOT retry the same approach."
                        );
                        messages.push(Message {
                            role: MessageRole::System,
                            content: nudge_msg,
                            tool_calls: None,
                            tool_call_id: None,
                            thinking_blocks: vec![],
                        });
                        loop_nudge_count += 1;
                        detector.reset();
                        tracing::warn!(
                            agent = %self.cfg().agent.name,
                            nudge_count = loop_nudge_count,
                            reason = ?reason,
                            "loop nudge injected, giving model another chance"
                        );
                        false // continue loop
                    } else {
                        tracing::error!(
                            agent = %self.cfg().agent.name,
                            nudge_count = loop_nudge_count,
                            "max loop nudges reached, force-stopping agent"
                        );
                        true // broken
                    }
                }
            };

            if loop_broken || iteration == loop_config.effective_max_iterations() - 1 {
                // Notify if hitting iteration limit (not loop break)
                if !loop_broken && iteration == loop_config.effective_max_iterations() - 1 {
                    tracing::warn!(
                        agent = %self.cfg().agent.name,
                        max_iterations = loop_config.effective_max_iterations(),
                        "agent reached iteration limit"
                    );
                    if let Some(ref ui_tx) = self.state().ui_event_tx {
                        let db = self.cfg().db.clone();
                        let tx = ui_tx.clone();
                        let agent_name = self.cfg().agent.name.clone();
                        let max_iter = loop_config.effective_max_iterations();
                        tokio::spawn(async move {
                            crate::gateway::notify(
                                &db, &tx, "iteration_limit",
                                &format!("Iteration limit: {agent_name}"),
                                &format!("Agent {agent_name} reached its iteration limit ({max_iter} iterations). The task may be incomplete."),
                                serde_json::json!({"agent": agent_name, "max_iterations": max_iter}),
                            ).await.ok();
                        });
                    }
                }
                // Notify if loop was broken after max nudges
                if loop_broken && loop_nudge_count >= loop_config.max_loop_nudges
                    && let Some(ref ui_tx) = self.state().ui_event_tx {
                        let db = self.cfg().db.clone();
                        let tx = ui_tx.clone();
                        let agent_name = self.cfg().agent.name.clone();
                        let sid = session_id;
                        tokio::spawn(async move {
                            crate::gateway::notify(
                                &db, &tx, "agent_loop_detected",
                                &format!("Agent stuck in loop: {agent_name}"),
                                &format!("Agent {agent_name} was stopped after detecting a repeating pattern. Session: {sid}"),
                                serde_json::json!({"agent": agent_name, "session_id": sid.to_string()}),
                            ).await.ok();
                        });
                    }
                match self.cfg().provider.chat(&messages, &[]).await {
                    Ok(forced) => {
                        final_response = strip_thinking(&forced.content);
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "isolated forced final LLM call failed");
                        final_response = error_classify::format_user_error(&e);
                    }
                }
                break;
            }
        }

        sm.save_message_ex(session_id, "assistant", &final_response, None, None, Some(&self.cfg().agent.name), None, None)
            .await?;

        // Post-session knowledge extraction (background, non-blocking)
        if messages.len() >= 5 {
            let db = self.cfg().db.clone();
            let provider = self.cfg().provider.clone();
            let memory = self.cfg().memory_store.clone();
            let agent_name = self.cfg().agent.name.clone();
            tokio::spawn(async move {
                crate::agent::knowledge_extractor::extract_and_save(
                    db, session_id, agent_name, provider, memory,
                ).await;
            });
        }

        // Hook: AfterResponse
        self.hooks().fire(&super::hooks::HookEvent::AfterResponse);

        Ok(final_response)
    }

    /// Build runtime context for system prompt injection.
    fn runtime_context(&self, msg: &IncomingMessage) -> workspace::RuntimeContext {
        workspace::RuntimeContext {
            agent_name: self.cfg().agent.name.clone(),
            owner_id: self.cfg().agent.access.as_ref().and_then(|a| a.owner_id.clone()),
            channel: msg.channel.clone(),
            model: self.cfg().provider.current_model(),
            datetime_display: workspace::format_local_datetime(&self.cfg().default_timezone),
            formatting_prompt: msg.formatting_prompt.clone(),
            channels: vec![], // populated async in build_context
        }
    }

    /// Get channel info for this agent (cached, refreshed on `channels_changed`).
    async fn get_channel_info(&self) -> Vec<workspace::ChannelInfo> {
        // Check cache first
        {
            let cache = self.state().channel_info_cache.read().await;
            if let Some(ref cached) = *cache {
                return cached.clone();
            }
        }
        // Cache miss — load from DB
        let info = self.load_channel_info_from_db().await;
        {
            let mut cache = self.state().channel_info_cache.write().await;
            *cache = Some(info.clone());
        }
        info
    }

    /// Invalidate channel info cache (called on channel CRUD).
    pub async fn invalidate_channel_cache(&self) {
        let mut cache = self.state().channel_info_cache.write().await;
        *cache = None;
    }

    async fn load_channel_info_from_db(&self) -> Vec<workspace::ChannelInfo> {
        let has_connected_channel = self.state().channel_router.is_some();
        let rows = sqlx::query_as::<_, (sqlx::types::Uuid, String, String, String)>(
            "SELECT id, channel_type, display_name, status FROM agent_channels WHERE agent_name = $1",
        )
        .bind(&self.cfg().agent.name)
        .fetch_all(&self.cfg().db)
        .await
        .unwrap_or_default();

        rows.into_iter().map(|(id, ch_type, name, status)| {
            workspace::ChannelInfo {
                channel_id: id.to_string(),
                channel_type: ch_type,
                display_name: name,
                online: status == "running" && has_connected_channel,
            }
        }).collect()
    }

    // ── Memory helpers (from engine_memory.rs) ──────────────────────────────

    /// Build L0 memory context: load pinned chunks for this agent.
    pub(super) async fn build_memory_context(&self, budget_tokens: u32) -> crate::agent::pipeline::memory::MemoryContext {
        crate::agent::pipeline::memory::build_memory_context(
            self.cfg().memory_store.as_ref(),
            &self.cfg().agent.name,
            budget_tokens,
        ).await
    }

    /// Index extracted facts into memory (called after session compaction via /compact).
    pub(super) async fn index_facts_to_memory(&self, facts: &[String]) {
        crate::agent::pipeline::memory::index_facts_to_memory(
            self.cfg().memory_store.as_ref(),
            &self.cfg().agent.name,
            facts,
        ).await
    }

    // ── Context helpers (from engine_context.rs) ─────────────────────────────

    /// Build common context: session, messages, system prompt.
    pub(super) async fn build_context(
        &self,
        msg: &IncomingMessage,
        include_tools: bool,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<crate::agent::context_builder::ContextSnapshot> {
        let cb = self.context_builder.get()
            .expect("context_builder not initialized — call set_context_builder after engine Arc creation");
        crate::agent::pipeline::context::build_context(cb.as_ref(), msg, include_tools, resume_session_id, force_new_session).await
    }

    /// Build a SecretsEnvResolver for YAML tool env resolution.
    pub(super) fn make_resolver(&self) -> SecretsEnvResolver {
        crate::agent::pipeline::context::make_resolver(self.secrets(), &self.cfg().agent.name)
    }

    /// Build OAuthContext for provider-based YAML tool auth (e.g. `oauth_provider: github`).
    pub(super) fn make_oauth_context(&self) -> Option<crate::tools::yaml_tools::OAuthContext> {
        crate::agent::pipeline::context::make_oauth_context(self.oauth().as_ref(), &self.cfg().agent.name)
    }

    /// Format a tool error as structured JSON for better LLM parsing.
    pub(super) fn format_tool_error(tool_name: &str, error: &str) -> String {
        crate::agent::pipeline::context::format_tool_error(tool_name, error)
    }

    /// Truncate a string to `max` chars with "..." suffix, preserving char boundaries.
    pub(super) fn truncate_preview(s: &str, max: usize) -> String {
        crate::agent::pipeline::context::truncate_preview(s, max)
    }

    /// Replace old tool results with "[compacted]" when context exceeds 70% of model window.
    pub(super) fn compact_tool_results(&self, messages: &mut [Message], context_chars: &mut usize) {
        crate::agent::pipeline::context::compact_tool_results(
            &self.cfg().agent.model,
            self.cfg().agent.compaction.as_ref(),
            messages,
            context_chars,
        )
    }

    /// Get compaction parameters from agent config.
    #[allow(dead_code)]
    pub(super) fn compaction_params(&self) -> (usize, usize) {
        crate::agent::pipeline::context::compaction_params(&self.cfg().agent.model, self.cfg().agent.compaction.as_ref())
    }

    /// Run compaction on messages if token budget exceeded, indexing extracted facts to memory.
    pub(super) async fn compact_messages(&self, messages: &mut Vec<Message>, detector: Option<&LoopDetector>) {
        let engine = self;
        let cfg = engine.cfg();
        crate::agent::pipeline::context::compact_messages(
            &cfg.agent.model,
            cfg.agent.compaction.as_ref(),
            &cfg.agent.language,
            cfg.provider.as_ref(),
            cfg.compaction_provider.as_deref(),
            &cfg.db,
            engine.state().ui_event_tx.as_ref(),
            &cfg.agent.name,
            &cfg.audit_queue,
            messages,
            detector,
            |facts| async move { engine.index_facts_to_memory(&facts).await },
        )
        .await
    }

    /// Compact a specific session's messages via API.
    pub async fn compact_session(&self, session_id: uuid::Uuid) -> Result<(usize, usize)> {
        let engine = self;
        let cfg = engine.cfg();
        crate::agent::pipeline::context::compact_session(
            &cfg.db,
            cfg.provider.as_ref(),
            cfg.compaction_provider.as_deref(),
            &cfg.agent.language,
            &cfg.agent.name,
            session_id,
            &cfg.audit_queue,
            |facts| async move { engine.index_facts_to_memory(&facts).await },
        )
        .await
    }

    // ── Command handler (from engine_commands.rs) ────────────────────────────

    /// Handle /slash commands. Returns Some(result) if a command matched, None otherwise.
    pub(super) async fn handle_command(&self, text: &str, msg: &IncomingMessage) -> Option<Result<String>> {
        let dm_scope = self.cfg().agent.session.as_ref()
            .map(|s| s.dm_scope.as_str())
            .unwrap_or("per-channel-peer");

        let ctx = crate::agent::pipeline::commands::CommandContext {
            agent_name: &self.cfg().agent.name,
            agent_language: &self.cfg().agent.language,
            agent_model: &self.cfg().agent.model,
            dm_scope,
            max_history_messages: self.cfg().agent.max_history_messages,
            compaction_config: self.cfg().agent.compaction.as_ref(),
            db: &self.cfg().db,
            provider: self.cfg().provider.as_ref(),
            compaction_provider: self.cfg().compaction_provider.as_deref(),
            thinking_level: &self.state().thinking_level,
            memory_store: self.cfg().memory_store.as_ref(),
        };

        crate::agent::pipeline::commands::handle_command(
            &ctx,
            text,
            msg,
            || async { self.invalidate_yaml_tools_cache().await },
        ).await
    }

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
            subagent_impl::SUBAGENT_DENIED_TOOLS,
            allowed_tools,
        )
    }
}

/// All system (internal) tool names — single source of truth.
pub fn all_system_tool_names() -> &'static [&'static str] {
    crate::agent::pipeline::tool_defs::all_system_tool_names()
}

// ── Extracted submodules ─────────────────────────────────────────────────────
#[path = "engine_dispatch.rs"]
mod dispatch_impl;

// ── ContextBuilderDeps impl ───────────────────────────────────────────────────

#[async_trait::async_trait]
impl crate::agent::context_builder::ContextBuilderDeps for AgentEngine {
    async fn session_resume(&self, sid: Uuid) -> Result<Uuid> {
        SessionManager::new(self.cfg().db.clone()).resume(sid).await
    }

    async fn session_create_new(&self, user_id: &str, channel: &str) -> Result<Uuid> {
        SessionManager::new(self.cfg().db.clone())
            .create_new(&self.cfg().agent.name, user_id, channel)
            .await
    }

    async fn session_get_or_create(
        &self,
        user_id: &str,
        channel: &str,
        dm_scope: &str,
    ) -> Result<Uuid> {
        SessionManager::new(self.cfg().db.clone())
            .get_or_create(&self.cfg().agent.name, user_id, channel, dm_scope)
            .await
    }

    async fn session_load_messages(
        &self,
        session_id: Uuid,
        limit: i64,
    ) -> Result<Vec<crate::db::sessions::MessageRow>> {
        SessionManager::new(self.cfg().db.clone())
            .load_messages(session_id, Some(limit))
            .await
    }

    async fn session_load_branch_messages(
        &self,
        session_id: Uuid,
        leaf_message_id: Uuid,
    ) -> Result<Vec<crate::db::sessions::MessageRow>> {
        crate::db::sessions::load_branch_messages(&self.cfg().db, session_id, leaf_message_id).await
    }

    async fn session_insert_missing_tool_results(
        &self,
        session_id: Uuid,
        call_ids: &[String],
    ) -> Result<()> {
        SessionManager::new(self.cfg().db.clone())
            .insert_missing_tool_results(session_id, call_ids)
            .await
    }

    async fn session_get_participants(&self, session_id: Uuid) -> Result<Vec<String>> {
        crate::db::sessions::get_participants(&self.cfg().db, session_id).await
    }

    fn agent_name(&self) -> &str {
        &self.cfg().agent.name
    }

    fn agent_base(&self) -> bool {
        self.cfg().agent.base
    }

    fn agent_language(&self) -> &str {
        &self.cfg().agent.language
    }

    fn agent_max_history_messages(&self) -> i64 {
        self.cfg().agent.max_history_messages.unwrap_or(50) as i64
    }

    fn agent_dm_scope(&self) -> &str {
        self.cfg().agent.session.as_ref()
            .map_or("per-channel-peer", |s| s.dm_scope.as_str())
    }

    fn agent_prune_tool_output_after_turns(&self) -> Option<usize> {
        self.cfg().agent.session.as_ref()
            .and_then(|s| s.prune_tool_output_after_turns)
    }

    fn agent_max_tools_in_context(&self) -> Option<usize> {
        self.cfg().agent.max_tools_in_context
    }

    async fn load_workspace_prompt(&self) -> Result<String> {
        workspace::load_workspace_prompt(&self.cfg().workspace_dir, &self.cfg().agent.name).await
    }

    async fn mcp_tool_definitions(&self) -> Vec<hydeclaw_types::ToolDefinition> {
        if let Some(mcp) = self.mcp() {
            mcp.all_tool_definitions().await
        } else {
            vec![]
        }
    }

    async fn has_tool(&self, name: &str) -> bool {
        AgentEngine::has_tool(self, name).await
    }

    fn memory_is_available(&self) -> bool {
        self.cfg().memory_store.is_available()
    }

    fn channel_router_present(&self) -> bool {
        self.state().channel_router.is_some()
    }

    fn scheduler_present(&self) -> bool {
        self.cfg().scheduler.is_some()
    }

    fn sandbox_absent(&self) -> bool {
        self.tex().sandbox.is_none()
    }

    fn runtime_context(&self, msg: &IncomingMessage) -> workspace::RuntimeContext {
        AgentEngine::runtime_context(self, msg)
    }

    async fn get_channel_info(&self) -> Vec<workspace::ChannelInfo> {
        AgentEngine::get_channel_info(self).await
    }

    fn pinned_budget_tokens(&self) -> u32 {
        self.cfg().app_config.memory.pinned_budget_tokens
    }

    async fn build_memory_context(&self, budget_tokens: u32) -> (String, Vec<String>) {
        let ctx = AgentEngine::build_memory_context(self, budget_tokens).await;
        (ctx.pinned_text, ctx.pinned_ids)
    }

    async fn store_pinned_chunk_ids(&self, ids: Vec<String>) {
        *self.tex().pinned_chunk_ids.lock().await = ids;
    }

    fn internal_tool_definitions(&self) -> Vec<hydeclaw_types::ToolDefinition> {
        AgentEngine::internal_tool_definitions(self)
    }

    async fn load_yaml_tools_cached(&self) -> Vec<crate::tools::yaml_tools::YamlToolDef> {
        let cache = self.tex().yaml_tools_cache.read().await;
        if cache.0.elapsed() < std::time::Duration::from_secs(30) && !cache.1.is_empty() {
            return cache.1.values().cloned().collect();
        }
        drop(cache);
        let loaded = crate::tools::yaml_tools::load_yaml_tools(&self.cfg().workspace_dir, false).await;
        let map: std::collections::HashMap<String, crate::tools::yaml_tools::YamlToolDef> =
            loaded.iter().cloned().map(|t| (t.name.clone(), t)).collect();
        *self.tex().yaml_tools_cache.write().await = (std::time::Instant::now(), std::sync::Arc::new(map));
        loaded
    }

    async fn tool_penalties(&self) -> std::collections::HashMap<String, f32> {
        self.tex().penalty_cache.get_penalties().await
    }

    fn filter_tools_by_policy(&self, tools: Vec<hydeclaw_types::ToolDefinition>) -> Vec<hydeclaw_types::ToolDefinition> {
        AgentEngine::filter_tools_by_policy(self, tools)
    }

    async fn select_top_k_tools_semantic(
        &self,
        tools: Vec<hydeclaw_types::ToolDefinition>,
        query: &str,
        k: usize,
    ) -> Vec<hydeclaw_types::ToolDefinition> {
        AgentEngine::select_top_k_tools_semantic(self, tools, query, k).await
    }
}

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
            subagent_impl::parse_subagent_timeout(&self.cfg().app_config.subagents.in_process_timeout)
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
        // 1. Build tool list (same as build_context but without session)
        let yaml_tools = crate::tools::yaml_tools::load_yaml_tools(&self.cfg().workspace_dir, false).await;
        let mut raw_tools = self.internal_tool_definitions();
        raw_tools.extend(yaml_tools.into_iter().map(|t| t.to_tool_definition()));
        if let Some(mcp) = self.mcp() {
            raw_tools.extend(mcp.all_tool_definitions().await);
        }
        let available_tools = self.filter_tools_by_policy(raw_tools);

        // 2. Determine the last user query for memory context
        let _last_user_text = openai_messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .and_then(|m| m.content.as_deref())
            .unwrap_or("");

        // 3. Convert OpenAI messages -> internal Message format.
        //    If the caller didn't provide a system message, prepend the agent's system prompt.
        let has_system = openai_messages.iter().any(|m| m.role == "system");
        let mut messages: Vec<Message> = Vec::with_capacity(openai_messages.len() + 1);

        if !has_system {
            let ws_prompt =
                workspace::load_workspace_prompt(&self.cfg().workspace_dir, &self.cfg().agent.name)
                    .await
                    .unwrap_or_default();

            let mcp_schemas: Vec<String> = if let Some(mcp) = self.mcp() {
                let defs = mcp.all_tool_definitions().await;
                defs.iter()
                    .map(|t| {
                        format!(
                            "- **{}**: {}\n  Parameters: {}",
                            t.name,
                            t.description,
                            serde_json::to_string(&t.input_schema).unwrap_or_default()
                        )
                    })
                    .collect()
            } else {
                vec![]
            };

            let capabilities = workspace::CapabilityFlags {
                has_search: self.has_tool("search_web").await || self.has_tool("search_web_fresh").await,
                has_memory: self.cfg().memory_store.is_available(),
                has_message_actions: false, // no channel adapter in API mode
                has_cron: self.cfg().scheduler.is_some(),
                has_yaml_tools: true,
                has_browser: crate::agent::pipeline::canvas::browser_renderer_url() != "disabled",
                has_host_exec: self.cfg().agent.base && self.sandbox().is_none(),
                is_base: self.cfg().agent.base,
            };

            let runtime = workspace::RuntimeContext {
                agent_name: self.cfg().agent.name.clone(),
                owner_id: self.cfg().agent.access.as_ref().and_then(|a| a.owner_id.clone()),
                channel: "api".to_string(),
                model: self.cfg().provider.current_model(),
                datetime_display: workspace::format_local_datetime(&self.cfg().default_timezone),
                formatting_prompt: None,
                channels: vec![],
            };
            let system_prompt = workspace::build_system_prompt(
                &ws_prompt,
                &mcp_schemas,
                &capabilities,
                &self.cfg().agent.language,
                &runtime,
            );

            messages.push(Message {
                role: MessageRole::System,
                content: system_prompt,
                tool_calls: None,
                tool_call_id: None,
                thinking_blocks: vec![],
            });
        }

        for m in openai_messages {
            messages.push(Message {
                role: match m.role.as_str() {
                    "system" => MessageRole::System,
                    "assistant" => MessageRole::Assistant,
                    "tool" => MessageRole::Tool,
                    _ => MessageRole::User,
                },
                content: m.content.clone().unwrap_or_default(),
                tool_calls: None,
                tool_call_id: None,
                thinking_blocks: vec![],
            });
        }

        // 4. Tool execution loop (no DB saves)
        let mut final_response = String::new();
        let mut last_usage: Option<hydeclaw_types::TokenUsage> = None;
        let loop_config = self.tool_loop_config();
        let mut detector = LoopDetector::new(&loop_config);
        let mut tools_used_acc: Vec<String> = Vec::new();
        let mut final_iteration: u32 = 0;

        for iteration in 0..loop_config.effective_max_iterations() {
            let response = if loop_config.compact_on_overflow {
                self.chat_with_overflow_recovery(&mut messages, &available_tools).await?
            } else {
                self.cfg().provider.chat(&messages, &available_tools).await?
            };
            last_usage = response.usage.clone();

            if response.tool_calls.is_empty() {
                final_response = response.content.clone();
                break;
            }

            // Accumulate tool names for API response
            for tc in &response.tool_calls {
                if !tools_used_acc.contains(&tc.name) {
                    tools_used_acc.push(tc.name.clone());
                }
            }
            final_iteration = iteration as u32 + 1;

            tracing::info!(
                iteration,
                max = loop_config.effective_max_iterations(),
                tools = response.tool_calls.len(),
                "openai api: executing tool calls"
            );

            messages.push(Message {
                role: MessageRole::Assistant,
                content: response.content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
                thinking_blocks: vec![],
            });

            let loop_broken = match self.execute_tool_calls_partitioned(
                &response.tool_calls, &serde_json::Value::Null, uuid::Uuid::nil(), crate::agent::channel_kind::channel::INTER_AGENT,
                messages.iter().map(|m| m.content.len()).sum(),
                &mut detector, loop_config.detect_loops,
            ).await {
                Ok(results) => {
                    for (tc_id, tool_result) in &results {
                        messages.push(Message {
                            role: MessageRole::Tool,
                            content: tool_result.clone(),
                            tool_calls: None,
                            tool_call_id: Some(tc_id.clone()),
                            thinking_blocks: vec![],
                        });
                    }
                    false
                }
                Err(_) => true,
            };

            if loop_broken || iteration == loop_config.effective_max_iterations() - 1 {
                let forced = self.cfg().provider.chat(&messages, &[]).await?;
                last_usage = forced.usage.clone();
                final_response = forced.content.clone();
                break;
            }
        }

        let final_response = strip_thinking(&final_response);

        // Send to chunk consumer if streaming requested (MiniMax sends full response at once)
        if let Some(ref tx) = chunk_tx
            && !final_response.is_empty() {
                tx.send(final_response.clone()).ok();
            }

        Ok(hydeclaw_types::LlmResponse {
            content: final_response,
            tool_calls: vec![],
            usage: last_usage,
            finish_reason: None,
            model: None,
            provider: None,
            fallback_notice: None,
            tools_used: tools_used_acc,
            iterations: final_iteration,
            thinking_blocks: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_cache_key_case_insensitive() {
        assert_eq!(search_cache_key("Bitcoin Price"), search_cache_key("bitcoin price"));
        assert_eq!(search_cache_key("HELLO"), search_cache_key("hello"));
    }

    #[test]
    fn search_cache_key_different_queries_different_keys() {
        assert_ne!(search_cache_key("bitcoin"), search_cache_key("ethereum"));
    }

    #[test]
    fn search_cache_key_deterministic() {
        let k1 = search_cache_key("test query");
        let k2 = search_cache_key("test query");
        assert_eq!(k1, k2);
    }

    #[test]
    fn cacheable_search_tools_contains_expected() {
        assert!(CACHEABLE_SEARCH_TOOLS.contains(&"searxng_search"));
        assert!(CACHEABLE_SEARCH_TOOLS.contains(&"brave_search"));
        assert!(!CACHEABLE_SEARCH_TOOLS.contains(&"memory_search"));
    }

    #[test]
    fn agent_in_system_tool_names() {
        let names = all_system_tool_names();
        assert!(names.contains(&"agent"), "agent must be in all_system_tool_names()");
        assert!(!names.contains(&"handoff"), "handoff should be removed");
        assert!(!names.contains(&"subagent"), "subagent should be removed");
    }
}

