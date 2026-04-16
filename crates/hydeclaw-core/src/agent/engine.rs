use anyhow::Result;
use hydeclaw_types::{IncomingMessage, Message, MessageRole, ToolDefinition};
use sqlx::PgPool;
use std::sync::{Arc, OnceLock, Weak};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::channel_actions::{ChannelAction, ChannelActionRouter};
use super::history;
use super::providers::LlmProvider;
use super::workspace;
use crate::config::AgentSettings;
use crate::db::sessions;
use crate::scheduler::{compute_next_run, Scheduler};
use crate::mcp::McpRegistry;
use crate::tools::ToolRegistry;

use super::error_classify;
use super::openapi::{discover_base_url, extract_openapi_tools};
use super::thinking::{looks_incomplete, maybe_strip_thinking, strip_thinking};
use super::tool_loop::LoopDetector;
use super::url_tools::{enrich_with_attachments, extract_readable_text, extract_urls};

// Extracted impl AgentEngine blocks (submodules of engine for full super:: access)
#[path = "engine_commands.rs"]
mod commands_impl;
#[path = "engine_sessions.rs"]
mod sessions_impl;
#[path = "engine_memory.rs"]
mod memory_impl;
#[path = "engine_tools.rs"]
mod tools_impl;
#[path = "engine_handlers.rs"]
mod handlers_impl;
#[path = "engine_tool_defs.rs"]
mod tool_defs_impl;
pub use tool_defs_impl::all_system_tool_names;
#[path = "engine_subagent.rs"]
mod subagent_impl;
#[path = "engine_parallel.rs"]
mod parallel_impl;
pub use parallel_impl::LoopBreak;
pub(crate) use subagent_impl::parse_subagent_timeout;
#[path = "engine_sandbox.rs"]
mod sandbox_impl;
#[path = "engine_execution.rs"]
mod execution_impl;
#[path = "engine_sse.rs"]
mod sse_impl;
#[path = "engine_agent_tool.rs"]
mod agent_tool_impl;

/// Resolves env var names through `SecretsManager` (scoped to agent).
struct SecretsEnvResolver {
    secrets: Arc<crate::secrets::SecretsManager>,
    agent_name: String,
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

pub struct AgentEngine {
    pub provider: Arc<dyn LlmProvider>,
    pub agent: AgentSettings,
    pub db: PgPool,
    pub tools: ToolRegistry,
    pub workspace_dir: String,
    /// Memory service abstraction (pgvector queries + external embedding endpoint).
    /// Held as a trait object so unit tests can inject a `MockMemoryService`.
    pub memory_store: Arc<dyn crate::agent::memory_service::MemoryService>,
    /// Embedding service for vector generation (shared with MemoryStore).
    pub embedder: Arc<dyn crate::memory::EmbeddingService>,
    /// Multi-channel router for sending actions to channel adapters.
    pub channel_router: Option<ChannelActionRouter>,
    /// Scheduler for dynamic cron jobs.
    pub scheduler: Option<Arc<Scheduler>>,
    /// Map of all running agents for inter-agent communication (None for subagents).
    pub agent_map: Option<crate::gateway::AgentMap>,
    /// Weak self-reference for hot-scheduling cron jobs. Set once after Arc creation.
    pub self_ref: OnceLock<Weak<AgentEngine>>,
    /// Broadcast channel for UI events (`agent_processing` start/end).
    pub ui_event_tx: Option<tokio::sync::broadcast::Sender<String>>,
    /// Shared tracker for currently processing agents (for WS reconnection).
    pub processing_tracker: Option<crate::gateway::ProcessingTracker>,
    /// Default timezone parsed from USER.md at startup (fallback: Europe/Samara).
    pub default_timezone: String,
    /// Last formatting prompt received from a connected channel adapter (e.g. Telegram).
    /// Used by cron/heartbeat to format output correctly for the channel.
    pub channel_formatting_prompt: tokio::sync::RwLock<Option<String>>,
    /// Cached channel info for system prompt injection (invalidated on channel CRUD).
    pub channel_info_cache: tokio::sync::RwLock<Option<Vec<workspace::ChannelInfo>>>,
    /// Thinking display level (0=off, 1=minimal, 2=low, 3=medium, 4=high, 5=max).
    pub thinking_level: std::sync::atomic::AtomicU8,
    /// Global app config for reading [agent.defaults] and other system-level settings.
    pub app_config: std::sync::Arc<crate::config::AppConfig>,
    /// Dedicated LLM provider for context compaction (cheap model). None = use primary provider.
    pub compaction_provider: Option<Arc<dyn LlmProvider>>,
    /// Context builder — builds session/messages/tools for each LLM call.
    /// Initialized via `set_context_builder` after engine Arc creation (mirrors `self_ref` pattern).
    /// Holds `Arc<dyn ContextBuilder>` for testability (`MockContextBuilder` in plan 02).
    pub context_builder: OnceLock<Arc<dyn crate::agent::context_builder::ContextBuilder>>,
    /// Tool executor — owns tool-only state (sandbox, caches, subagent registry, etc.).
    /// Stored as concrete `Arc<DefaultToolExecutor>` for direct field access in engine methods.
    /// Initialized via `set_tool_executor` after engine Arc creation.
    pub tool_executor: OnceLock<Arc<crate::agent::tool_executor::DefaultToolExecutor>>,
    /// Bounded audit event queue (tool execution + quality recording).
    pub audit_queue: std::sync::Arc<crate::db::audit_queue::AuditQueue>,
    /// Approval workflow manager (DB records, channel notifications, waiter map).
    pub approval_manager: Arc<super::approval_manager::ApprovalManager>,
    /// Session-scoped agent pools (None for subagents / isolated engines).
    pub session_pools: Option<crate::agent::session_agent_pool::SessionPoolsMap>,
    /// Per-agent mutable state (cancel/drain for shutdown and SIGHUP).
    /// `None` for subagent engines — they are lightweight copies without lifecycle tracking.
    pub state: Option<Arc<crate::agent::agent_state::AgentState>>,
}

/// Snapshot of what's currently displayed on the canvas.
#[derive(Debug, Clone)]
pub struct CanvasContent {
    pub content_type: String,
    pub content: String,
    pub title: Option<String>,
}


/// Maximum canvas content size (5 MB) to protect constrained environments.
const CANVAS_MAX_BYTES: usize = 5 * 1024 * 1024;

/// In-band marker prefix for rich card tool results.
const RICH_CARD_PREFIX: &str = "__rich_card__:";

/// In-band marker prefix for file/media tool results (image, audio, etc.).
/// Format: `__file__:{"url":"...","mediaType":"image/png"}`
const FILE_PREFIX: &str = "__file__:";

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

    /// Agent name (from config).
    pub fn name(&self) -> &str {
        &self.agent.name
    }

    /// Primary model name (from config).
    pub fn model_name(&self) -> String {
        self.agent.model.clone()
    }

    /// Borrow the database pool.
    pub fn db_pool(&self) -> &PgPool {
        &self.db
    }

    /// Clone the LLM provider Arc for use outside the engine.
    pub fn provider_arc(&self) -> Arc<dyn LlmProvider> {
        self.provider.clone()
    }

    /// Read the current channel formatting prompt.
    pub async fn formatting_prompt(&self) -> Option<String> {
        self.channel_formatting_prompt.read().await.clone()
    }

    /// Borrow the channel action router, if configured.
    pub fn channel_router_ref(&self) -> Option<&ChannelActionRouter> {
        self.channel_router.as_ref()
    }

    /// Borrow the agent access config, if set.
    pub fn agent_access(&self) -> Option<&crate::config::AgentAccessConfig> {
        self.agent.access.as_ref()
    }

    /// Delegate model override to the underlying provider.
    pub fn set_model_override(&self, model: Option<String>) {
        self.provider.set_model_override(model);
    }

    /// Return the current active model name from the provider.
    pub fn current_model(&self) -> String {
        self.provider.current_model()
    }

    // ── Lifecycle ──────────────────────────────────────────────────

    /// Store a weak self-reference after the engine is wrapped in Arc.
    /// Used by cron tool to hot-schedule jobs without restart.
    pub fn set_self_ref(&self, arc: &Arc<AgentEngine>) {
        let _ = self.self_ref.set(Arc::downgrade(arc));
    }

    /// Initialize the context builder after engine Arc creation.
    /// Must be called once, mirrors `set_self_ref` pattern.
    pub fn set_context_builder(&self, arc: &Arc<AgentEngine>) {
        use crate::agent::context_builder::{ContextBuilderDeps, DefaultContextBuilder};
        let deps = arc.clone() as Arc<dyn ContextBuilderDeps>;
        let builder = Arc::new(DefaultContextBuilder::new(deps))
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
        if let Some(ref tx) = self.ui_event_tx {
            tx.send(event.to_string()).ok();
        }
    }

    /// Check if a tool requires approval before execution.
    fn needs_approval(&self, tool_name: &str) -> bool {
        let approval = match &self.agent.approval {
            Some(a) if a.enabled => a,
            _ => return false,
        };

        // Check explicit tool names
        if approval.require_for.iter().any(|t| t == tool_name) {
            return true;
        }

        // Check categories
        if !approval.require_for_categories.is_empty() {
            let category = super::channel_kind::ToolCategory::classify(tool_name);
            if approval.require_for_categories.iter().any(|c| c == category.as_str()) {
                return true;
            }
        }

        false
    }

    /// Resolve a pending approval (called from API/callback handler).
    pub async fn resolve_approval(&self, approval_id: Uuid, approved: bool, resolved_by: &str, modified_input: Option<serde_json::Value>) -> anyhow::Result<()> {
        let status = if approved { "approved" } else { "rejected" };
        let updated = crate::db::approvals::resolve_approval(&self.db, approval_id, status, resolved_by).await?;
        if !updated {
            anyhow::bail!("approval {approval_id} not found or already resolved");
        }

        self.audit(crate::db::audit::event_types::APPROVAL_RESOLVED, Some(resolved_by), serde_json::json!({
            "approval_id": approval_id.to_string(), "status": status
        }));

        self.broadcast_ui_event(serde_json::json!({
            "type": "approval_resolved",
            "approval_id": approval_id.to_string(),
            "agent": self.agent.name,
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
        let mut waiters = self.approval_manager.waiters().write().await;
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

    /// Enrich tool arguments with `_context` (message context + `session_id`).
    /// Uses `insert` (not `or_insert`) intentionally — LLM must not be able to
    /// forge `_context` (e.g., spoofing `chat_id` for channel actions).
    fn enrich_tool_args(args: &serde_json::Value, context: &serde_json::Value, session_id: Uuid, channel: &str) -> serde_json::Value {
        let mut args = args.clone();
        if let Some(obj) = args.as_object_mut() {
            // If context is Null, create an empty object so session_id can be injected.
            let mut ctx = if context.is_null() {
                serde_json::json!({})
            } else {
                context.clone()
            };
            if let Some(ctx_obj) = ctx.as_object_mut() {
                ctx_obj.insert("session_id".to_string(), serde_json::json!(session_id.to_string()));
                ctx_obj.insert("_channel".to_string(), serde_json::json!(channel));
            }
            obj.insert("_context".to_string(), ctx);
        }
        args
    }

    /// Check if an enabled YAML tool exists in workspace/tools/ (shared tools).
    async fn has_tool(&self, name: &str) -> bool {
        let dir = std::path::Path::new(&self.workspace_dir).join("tools");
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
        if let Some(max) = self.agent.session.as_ref().and_then(|s| {
            if s.max_messages > 0 { Some(s.max_messages) } else { None }
        }) {
            let sm = SessionManager::new(self.db.clone());
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

        let sm = SessionManager::new(self.db.clone());
        let session_id = sm.create_isolated(&self.agent.name, &msg.user_id, &msg.channel).await?;

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
                        agent = %self.agent.name,
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
                                agent = %self.agent.name,
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
                        let db = self.db.clone();
                        let agent_name = self.agent.name.clone();
                        let cnt = auto_continue_count;
                        let max = loop_config.max_auto_continues;
                        if let Some(ref ui_tx) = self.ui_event_tx {
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
                Err(parallel_impl::LoopBreak(reason)) => {
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
                            agent = %self.agent.name,
                            nudge_count = loop_nudge_count,
                            reason = ?reason,
                            "loop nudge injected, giving model another chance"
                        );
                        false // continue loop
                    } else {
                        tracing::error!(
                            agent = %self.agent.name,
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
                        agent = %self.agent.name,
                        max_iterations = loop_config.effective_max_iterations(),
                        "agent reached iteration limit"
                    );
                    if let Some(ref ui_tx) = self.ui_event_tx {
                        let db = self.db.clone();
                        let tx = ui_tx.clone();
                        let agent_name = self.agent.name.clone();
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
                    && let Some(ref ui_tx) = self.ui_event_tx {
                        let db = self.db.clone();
                        let tx = ui_tx.clone();
                        let agent_name = self.agent.name.clone();
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
                match self.provider.chat(&messages, &[]).await {
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

        sm.save_message_ex(session_id, "assistant", &final_response, None, None, Some(&self.agent.name), None, None)
            .await?;

        // Post-session knowledge extraction (background, non-blocking)
        if messages.len() >= 5 {
            let db = self.db.clone();
            let provider = self.provider.clone();
            let memory = self.memory_store.clone();
            let agent_name = self.agent.name.clone();
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
            agent_name: self.agent.name.clone(),
            owner_id: self.agent.access.as_ref().and_then(|a| a.owner_id.clone()),
            channel: msg.channel.clone(),
            model: self.provider.current_model(),
            datetime_display: workspace::format_local_datetime(&self.default_timezone),
            formatting_prompt: msg.formatting_prompt.clone(),
            channels: vec![], // populated async in build_context
        }
    }

    /// Get channel info for this agent (cached, refreshed on `channels_changed`).
    async fn get_channel_info(&self) -> Vec<workspace::ChannelInfo> {
        // Check cache first
        {
            let cache = self.channel_info_cache.read().await;
            if let Some(ref cached) = *cache {
                return cached.clone();
            }
        }
        // Cache miss — load from DB
        let info = self.load_channel_info_from_db().await;
        {
            let mut cache = self.channel_info_cache.write().await;
            *cache = Some(info.clone());
        }
        info
    }

    /// Invalidate channel info cache (called on channel CRUD).
    pub async fn invalidate_channel_cache(&self) {
        let mut cache = self.channel_info_cache.write().await;
        *cache = None;
    }

    async fn load_channel_info_from_db(&self) -> Vec<workspace::ChannelInfo> {
        let has_connected_channel = self.channel_router.is_some();
        let rows = sqlx::query_as::<_, (sqlx::types::Uuid, String, String, String)>(
            "SELECT id, channel_type, display_name, status FROM agent_channels WHERE agent_name = $1",
        )
        .bind(&self.agent.name)
        .fetch_all(&self.db)
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
}

// ── Extracted submodules ─────────────────────────────────────────────────────
#[path = "engine_context.rs"]
mod context_impl;
#[path = "engine_provider.rs"]
mod provider_impl;
#[path = "engine_dispatch.rs"]
mod dispatch_impl;

// ── ContextBuilderDeps impl ───────────────────────────────────────────────────

#[async_trait::async_trait]
impl crate::agent::context_builder::ContextBuilderDeps for AgentEngine {
    async fn session_resume(&self, sid: Uuid) -> Result<Uuid> {
        SessionManager::new(self.db.clone()).resume(sid).await
    }

    async fn session_create_new(&self, user_id: &str, channel: &str) -> Result<Uuid> {
        SessionManager::new(self.db.clone())
            .create_new(&self.agent.name, user_id, channel)
            .await
    }

    async fn session_get_or_create(
        &self,
        user_id: &str,
        channel: &str,
        dm_scope: &str,
    ) -> Result<Uuid> {
        SessionManager::new(self.db.clone())
            .get_or_create(&self.agent.name, user_id, channel, dm_scope)
            .await
    }

    async fn session_load_messages(
        &self,
        session_id: Uuid,
        limit: i64,
    ) -> Result<Vec<crate::db::sessions::MessageRow>> {
        SessionManager::new(self.db.clone())
            .load_messages(session_id, Some(limit))
            .await
    }

    async fn session_load_branch_messages(
        &self,
        session_id: Uuid,
        leaf_message_id: Uuid,
    ) -> Result<Vec<crate::db::sessions::MessageRow>> {
        crate::db::sessions::load_branch_messages(&self.db, session_id, leaf_message_id).await
    }

    async fn session_insert_missing_tool_results(
        &self,
        session_id: Uuid,
        call_ids: &[String],
    ) -> Result<()> {
        SessionManager::new(self.db.clone())
            .insert_missing_tool_results(session_id, call_ids)
            .await
    }

    async fn session_get_participants(&self, session_id: Uuid) -> Result<Vec<String>> {
        crate::db::sessions::get_participants(&self.db, session_id).await
    }

    fn agent_name(&self) -> &str {
        &self.agent.name
    }

    fn agent_base(&self) -> bool {
        self.agent.base
    }

    fn agent_language(&self) -> &str {
        &self.agent.language
    }

    fn agent_max_history_messages(&self) -> i64 {
        self.agent.max_history_messages.unwrap_or(50) as i64
    }

    fn agent_dm_scope(&self) -> &str {
        self.agent.session.as_ref()
            .map_or("per-channel-peer", |s| s.dm_scope.as_str())
    }

    fn agent_prune_tool_output_after_turns(&self) -> Option<usize> {
        self.agent.session.as_ref()
            .and_then(|s| s.prune_tool_output_after_turns)
    }

    fn agent_max_tools_in_context(&self) -> Option<usize> {
        self.agent.max_tools_in_context
    }

    async fn load_workspace_prompt(&self) -> Result<String> {
        workspace::load_workspace_prompt(&self.workspace_dir, &self.agent.name).await
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
        self.memory_store.is_available()
    }

    fn channel_router_present(&self) -> bool {
        self.channel_router.is_some()
    }

    fn scheduler_present(&self) -> bool {
        self.scheduler.is_some()
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
        self.app_config.memory.pinned_budget_tokens
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
        let loaded = crate::tools::yaml_tools::load_yaml_tools(&self.workspace_dir, false).await;
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
    ) -> Result<Vec<(String, String)>, parallel_impl::LoopBreak> {
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
}

