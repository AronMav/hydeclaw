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
#[path = "engine_handoff.rs"]
mod handoff_impl;

/// Resolves env var names through SecretsManager (scoped to agent).
struct SecretsEnvResolver {
    secrets: Arc<crate::secrets::SecretsManager>,
    agent_name: String,
}

impl crate::tools::yaml_tools::EnvResolver for SecretsEnvResolver {
    fn resolve(&self, key: &str) -> Option<String> {
        // SAFETY (REL-03): block_in_place is used here because the EnvResolver trait
        // is synchronous (fn resolve(&self, key: &str) -> Option<String>), and making
        // it async would propagate through the entire YAML tool execution pipeline.
        //
        // This is safe because:
        // 1. HydeClaw uses the multi-thread tokio runtime (#[tokio::main]), so
        //    block_in_place moves other tasks off this worker thread (no deadlock).
        // 2. The cache read lock (tokio::sync::RwLock) is held for microseconds
        //    (HashMap lookup by (String, String) key).
        // 3. Write contention is minimal: secrets are written only via human-initiated
        //    API calls (SecretsManager::set), which are rare.
        // 4. Secret reads are per-tool-call (seconds apart), not in a hot loop.
        let cache = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.secrets.cache_ref().read())
        });
        // Try agent-scoped first
        if let Some(val) = cache.get(&(key.to_string(), self.agent_name.clone())) {
            return Some(val.clone());
        }
        // Then global
        if let Some(val) = cache.get(&(key.to_string(), String::new())) {
            return Some(val.clone());
        }
        // env::var fallback is handled by resolve_env in yaml_tools
        None
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
    /// Convert to wire format: (phase_name, optional_tool_name).
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
    /// Session ID resolved/created by build_context — emitted first so the UI can track it.
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
    Finish { finish_reason: String },
    /// Internal event: signals that a different agent is now responding (multi-agent turn loop).
    /// Converter task updates current_responding_agent; no SSE is emitted to the client.
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
    pub mcp: Option<Arc<McpRegistry>>,
    pub workspace_dir: String,
    pub http_client: reqwest::Client,
    /// SSRF-safe HTTP client for user-supplied URLs (custom DNS resolver blocks private IPs).
    pub ssrf_http_client: reqwest::Client,
    /// Native memory store (pgvector queries + external embedding endpoint).
    pub memory_store: Arc<crate::memory::MemoryStore>,
    /// Limits concurrent in-process subagents to prevent API token exhaustion.
    pub subagent_semaphore: Arc<tokio::sync::Semaphore>,
    /// Registry of async subagents for status/logs/kill management.
    pub subagent_registry: super::subagent_state::SubagentRegistry,
    /// Multi-channel router for sending actions to channel adapters.
    pub channel_router: Option<ChannelActionRouter>,
    /// Scheduler for dynamic cron jobs.
    pub scheduler: Option<Arc<Scheduler>>,
    /// Code execution sandbox (Docker). None when sandbox is disabled or Docker unavailable.
    pub sandbox: Option<Arc<crate::containers::sandbox::CodeSandbox>>,
    /// In-memory cache for tool embeddings (semantic top-K selection).
    pub tool_embed_cache: Arc<crate::tools::embedding::ToolEmbeddingCache>,
    /// Secrets vault for resolving auth keys in YAML tools.
    pub secrets: Arc<crate::secrets::SecretsManager>,
    /// Map of all running agents for inter-agent communication (None for subagents).
    pub agent_map: Option<crate::gateway::AgentMap>,
    /// Weak self-reference for hot-scheduling cron jobs. Set once after Arc creation.
    pub self_ref: OnceLock<Weak<AgentEngine>>,
    /// In-memory waiters for pending tool-call approvals (approval_id -> (result_sender, created_at)).
    #[allow(clippy::type_complexity)]
    pub approval_waiters: Arc<tokio::sync::RwLock<std::collections::HashMap<Uuid, (tokio::sync::oneshot::Sender<ApprovalResult>, std::time::Instant)>>>,
    /// Broadcast channel for UI events (agent_processing start/end).
    pub ui_event_tx: Option<tokio::sync::broadcast::Sender<String>>,
    /// Current session ID being processed (set during handle_sse/handle_with_status, cleared on finish).
    /// Used by tools that need session context (e.g., handoff).
    pub processing_session_id: Arc<tokio::sync::Mutex<Option<Uuid>>>,
    /// Set by handoff tool during execution; read and cleared by turn loop in chat.rs.
    pub handoff_target: Arc<tokio::sync::Mutex<Option<HandoffRequest>>>,
    /// Shared tracker for currently processing agents (for WS reconnection).
    pub processing_tracker: Option<crate::gateway::ProcessingTracker>,
    /// Default timezone parsed from USER.md at startup (fallback: Europe/Samara).
    pub default_timezone: String,
    /// Mutex for atomic MEMORY.md read-modify-write operations.
    pub memory_md_lock: tokio::sync::Mutex<()>,
    /// IDs of L0 pinned chunks loaded in the current context build (for L2 dedup).
    pub(crate) pinned_chunk_ids: tokio::sync::Mutex<Vec<String>>,
    /// Last formatting prompt received from a connected channel adapter (e.g. Telegram).
    /// Used by cron/heartbeat to format output correctly for the channel.
    pub channel_formatting_prompt: tokio::sync::RwLock<Option<String>>,
    /// Cached channel info for system prompt injection (invalidated on channel CRUD).
    pub channel_info_cache: tokio::sync::RwLock<Option<Vec<workspace::ChannelInfo>>>,
    /// Thinking display level (0=off, 1=minimal, 2=low, 3=medium, 4=high, 5=max).
    pub thinking_level: std::sync::atomic::AtomicU8,
    /// Current canvas content for eval/snapshot (content_type, content, title).
    pub canvas_state: tokio::sync::RwLock<Option<CanvasContent>>,
    /// Cached YAML tool definitions with TTL (avoids per-batch disk reads in parallel execution).
    pub yaml_tools_cache: tokio::sync::RwLock<(std::time::Instant, std::collections::HashMap<String, crate::tools::yaml_tools::YamlToolDef>)>,
    /// Background processes started by `process_start` tool (base agents only).
    pub bg_processes: Arc<tokio::sync::Mutex<std::collections::HashMap<String, BgProcess>>>,
    /// OAuth 2.0 connection manager for provider-based YAML tool auth.
    pub oauth: Option<Arc<crate::oauth::OAuthManager>>,
    /// Event hooks for policy enforcement and logging.
    pub hooks: Arc<super::hooks::HookRegistry>,
    /// Tool quality penalty cache for adaptive tool ranking.
    pub penalty_cache: Arc<crate::db::tool_quality::PenaltyCache>,
    /// Per-engine web search cache (query_hash → (result, expiry)).
    /// TTL: 5 minutes. Prevents duplicate HTTP calls for identical queries.
    pub(crate) search_cache: tokio::sync::RwLock<std::collections::HashMap<u64, (String, std::time::Instant)>>,
    /// Global app config for reading [agent.defaults] and other system-level settings.
    pub app_config: std::sync::Arc<crate::config::AppConfig>,
    /// Dedicated LLM provider for context compaction (cheap model). None = use primary provider.
    pub compaction_provider: Option<Arc<dyn LlmProvider>>,
}

/// Snapshot of what's currently displayed on the canvas.
#[derive(Debug, Clone)]
pub struct CanvasContent {
    pub content_type: String,
    pub content: String,
    pub title: Option<String>,
}

/// Handoff request set by `handoff` tool, consumed by turn loop in chat.rs.
#[derive(Debug, Clone)]
pub struct HandoffRequest {
    pub target_agent: String,
    pub task: String,
    pub context: String,
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
    Rejected(String),
}

/// RAII guard: inserts into processing tracker on creation, removes + broadcasts "end" on drop.
/// Uses session_id as tracker key (not agent_name) to support concurrent sessions per agent.
struct ProcessingGuard {
    tx: Option<tokio::sync::broadcast::Sender<String>>,
    processing_tracker: Option<crate::gateway::ProcessingTracker>,
    agent_name: String,
    /// Tracker key — session_id for unique identification across concurrent sessions.
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
        let session_id = start_event.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_string());
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

/// Outcome of a session lifecycle — used by `SessionLifecycleGuard`.
#[allow(dead_code)]
enum SessionOutcome {
    Running,
    Done,
    Failed(String),
}

/// RAII guard that marks a session as 'failed' if dropped without an explicit done/fail call.
///
/// Usage: call `done().await` on success or `fail(reason).await` on known errors.
/// If neither is called (e.g. early `?` return), `Drop` fires a best-effort fallback
/// via `tokio::spawn` to mark the session as 'failed'.
struct SessionLifecycleGuard {
    db: PgPool,
    session_id: Uuid,
    outcome: SessionOutcome,
}

impl SessionLifecycleGuard {
    fn new(db: PgPool, session_id: Uuid) -> Self {
        Self { db, session_id, outcome: SessionOutcome::Running }
    }

    /// Mark session as done in DB. Sets outcome to `Done` only on DB success;
    /// on failure logs a warning and leaves `Running` so `Drop` fires fallback.
    async fn done(&mut self) {
        match crate::db::sessions::set_session_run_status(&self.db, self.session_id, "done").await {
            Ok(()) => self.outcome = SessionOutcome::Done,
            Err(e) => tracing::warn!(session_id = %self.session_id, error = %e, "failed to mark session done in DB"),
        }
    }

    /// Mark session as failed in DB with a reason. Sets outcome to `Failed` only on DB success;
    /// on failure logs a warning and leaves `Running` so `Drop` fires fallback.
    async fn fail(&mut self, reason: &str) {
        match crate::db::sessions::set_session_run_status(&self.db, self.session_id, "failed").await {
            Ok(()) => self.outcome = SessionOutcome::Failed(reason.to_string()),
            Err(e) => tracing::warn!(session_id = %self.session_id, error = %e, reason, "failed to mark session failed in DB"),
        }
    }
}

impl Drop for SessionLifecycleGuard {
    fn drop(&mut self) {
        if matches!(self.outcome, SessionOutcome::Running) {
            tracing::warn!(session_id = %self.session_id, "session guard dropped while still Running — spawning fallback mark-failed");
            let db = self.db.clone();
            let sid = self.session_id;
            tokio::spawn(async move {
                if let Err(e) = crate::db::sessions::set_session_run_status(&db, sid, "failed").await {
                    tracing::warn!(error = %e, session_id = %sid, "failed to mark session as failed in Drop guard");
                }
            });
        }
    }
}

/// Convert a DB MessageRow into a typed Message.
/// Parses tool_calls JSON exactly once per row (ENG-02).
fn row_to_message(row: &crate::db::sessions::MessageRow) -> Message {
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

/// Strip `<minimax:tool_call>…</minimax:tool_call>` blocks from a string.
///
/// Used to sanitize tool results that contain MiniMax XML tool calls before they are
/// included in the LLM context. Prevents MiniMax from learning the XML format from
/// its own previous outputs and reproducing it endlessly.
fn strip_minimax_xml(s: &str) -> String {
    const OPEN: &str = "<minimax:tool_call>";
    const CLOSE: &str = "</minimax:tool_call>";
    if !s.contains(OPEN) {
        return s.to_string();
    }
    let mut result = String::new();
    let mut rest = s;
    loop {
        match rest.find(OPEN) {
            None => {
                result.push_str(rest);
                break;
            }
            Some(start) => {
                result.push_str(&rest[..start]);
                let after = &rest[start + OPEN.len()..];
                rest = match after.find(CLOSE) {
                    Some(end) => &after[end + CLOSE.len()..],
                    None => break, // unclosed — discard rest
                };
            }
        }
    }
    result.trim().to_string()
}

/// Proactively strip tool result content from old turns to reduce LLM context on load.
///
/// Complements `compact_tool_results` (reactive, fires during tool loop at 70% threshold).
/// This fires once in `build_context` — before the first LLM call — based on turn count.
///
/// A "turn" = one user message + the assistant+tool messages that follow it.
/// Keeps the last `keep_turns` complete turns intact; replaces older tool results with
/// a "[output omitted, N chars]" placeholder (preserving empty results untouched).
fn prune_old_tool_outputs(messages: &[Message], keep_turns: usize) -> Vec<Message> {
    let user_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == MessageRole::User)
        .map(|(i, _)| i)
        .collect();

    if user_indices.len() <= keep_turns {
        return messages.to_vec();
    }

    let cutoff = user_indices[user_indices.len() - keep_turns];

    messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if i < cutoff && m.role == MessageRole::Tool && !m.content.is_empty() {
                let hidden = m.content.len();
                Message {
                    content: format!("[output omitted, {} chars]", hidden),
                    ..m.clone()
                }
            } else {
                m.clone()
            }
        })
        .collect()
}

impl AgentEngine {
    /// Store a weak self-reference after the engine is wrapped in Arc.
    /// Used by cron tool to hot-schedule jobs without restart.
    pub fn set_self_ref(&self, arc: &Arc<AgentEngine>) {
        let _ = self.self_ref.set(Arc::downgrade(arc));
    }

    /// Invalidate the cached YAML tool definitions so the next request reloads from disk.
    pub(crate) async fn invalidate_yaml_tools_cache(&self) {
        *self.yaml_tools_cache.write().await = (
            std::time::Instant::now() - std::time::Duration::from_secs(60),
            std::collections::HashMap::new(),
        );
    }

    pub(crate) async fn check_search_cache(&self, query: &str) -> Option<String> {
        let cache = self.search_cache.read().await;
        if let Some((result, expiry)) = cache.get(&search_cache_key(query))
            && *expiry > std::time::Instant::now()
        {
            tracing::debug!(query, "search cache hit");
            return Some(result.clone());
        }
        None
    }

    pub(crate) async fn store_search_cache(&self, query: &str, result: &str) {
        let mut cache = self.search_cache.write().await;
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
    pub async fn resolve_approval(&self, approval_id: Uuid, approved: bool, resolved_by: &str) -> anyhow::Result<()> {
        let status = if approved { "approved" } else { "rejected" };
        let updated = crate::db::approvals::resolve_approval(&self.db, approval_id, status, resolved_by).await?;
        if !updated {
            anyhow::bail!("approval {} not found or already resolved", approval_id);
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

        // Wake up the waiting tool execution
        let mut waiters = self.approval_waiters.write().await;
        if let Some((tx, _created_at)) = waiters.remove(&approval_id) {
            let result = if approved {
                ApprovalResult::Approved
            } else {
                ApprovalResult::Rejected(format!("rejected by {}", resolved_by))
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

    /// Enrich tool arguments with `_context` (message context + session_id).
    /// Uses `insert` (not `or_insert`) intentionally — LLM must not be able to
    /// forge `_context` (e.g., spoofing chat_id for channel actions).
    fn enrich_tool_args(args: &serde_json::Value, context: &serde_json::Value, session_id: Uuid, channel: &str) -> serde_json::Value {
        let mut args = args.clone();
        if let Some(obj) = args.as_object_mut() {
            let mut ctx = context.clone();
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

    /// Trim session messages if max_messages is configured.
    async fn maybe_trim_session(&self, session_id: Uuid) {
        if let Some(max) = self.agent.session.as_ref().and_then(|s| {
            if s.max_messages > 0 { Some(s.max_messages) } else { None }
        })
            && let Err(e) = sessions::trim_session_messages(&self.db, session_id, max).await {
                tracing::warn!(error = %e, "failed to trim session messages");
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
        if let super::hooks::HookAction::Block(reason) = self.hooks.fire(&super::hooks::HookEvent::BeforeMessage) {
            anyhow::bail!("blocked by hook: {}", reason);
        }

        let session_id = sessions::create_isolated_session_with_user(
            &self.db,
            &self.agent.name,
            &msg.user_id,
            &msg.channel,
        )
        .await?;

        let (_, mut messages, mut available_tools) =
            self.build_context(msg, true, Some(session_id), false).await?;

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

        // invite_agent removed (v3.0) — handoff is the only inter-agent tool

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
        sessions::save_message_ex(&self.db, session_id, "user", &user_text, None, None, sender_agent_id, None).await?;

        // Context compaction if needed (model-aware token budget)
        self.compact_messages(&mut messages).await;

        // LLM loop (with tool calls)
        let mut final_response = String::new();
        let loop_config = self.tool_loop_config();
        let mut detector = LoopDetector::new(&loop_config);
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
                    self.hooks.fire(&super::hooks::HookEvent::OnError);
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
                                    &format!("Auto-continue: {}", agent_name),
                                    &format!("Agent continued unfinished task (attempt {}/{})", cnt, max),
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
            if let Err(e) = sessions::save_message(
                &self.db, session_id, "assistant", &cleaned_content,
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
                        if let Err(e) = sessions::save_message(
                            &self.db, session_id, "tool", tool_result, None, Some(tc_id),
                        ).await {
                            tracing::warn!(error = %e, session_id = %session_id, "failed to save tool result to DB");
                        }
                    }
                    false
                }
                Err(_) => true,
            };

            if loop_broken || iteration == loop_config.effective_max_iterations() - 1 {
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

        sessions::save_message_ex(&self.db, session_id, "assistant", &final_response, None, None, Some(&self.agent.name), None)
            .await?;

        // Hook: AfterResponse
        self.hooks.fire(&super::hooks::HookEvent::AfterResponse);

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

    /// Get channel info for this agent (cached, refreshed on channels_changed).
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

    /// Build common context: session, messages, system prompt.
    /// Returns (session_id, messages, available_tools).
    /// If `resume_session_id` is Some, reuses that session instead of creating/finding one.
    async fn build_context(
        &self,
        msg: &IncomingMessage,
        include_tools: bool,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<(Uuid, Vec<Message>, Vec<ToolDefinition>)> {
        // 1. Get or create session (or resume existing)
        let session_id = if let Some(sid) = resume_session_id {
            sessions::resume_session(&self.db, sid).await?
        } else if force_new_session {
            sessions::create_new_session(
                &self.db,
                &self.agent.name,
                &msg.user_id,
                &msg.channel,
            )
            .await?
        } else {
            {
                let dm_scope = self.agent.session.as_ref()
                    .map(|s| s.dm_scope.as_str())
                    .unwrap_or("per-channel-peer");
                sessions::get_or_create_session(
                    &self.db,
                    &self.agent.name,
                    &msg.user_id,
                    &msg.channel,
                    dm_scope,
                )
                .await?
            }
        };

        // 2. Load conversation history
        let limit = self.agent.max_history_messages.unwrap_or(50) as i64;
        let history = sessions::load_messages(&self.db, session_id, Some(limit)).await?;

        // 3. Build system prompt with MCP tool schemas
        let ws_prompt =
            workspace::load_workspace_prompt(&self.workspace_dir, &self.agent.name).await?;

        // MCP tool schemas in system prompt: name + description only.
        // Full parameter schemas are provided via native tool definitions (section 6).
        let mcp_schemas: Vec<String> = if let Some(ref mcp) = self.mcp {
            let defs = mcp.all_tool_definitions().await;
            defs.iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect()
        } else {
            vec![]
        };

        // 4. Capabilities + system prompt
        let user_text = msg.text.clone().unwrap_or_default();

        let capabilities = workspace::CapabilityFlags {
            has_search: self.has_tool("search_web").await || self.has_tool("search_web_fresh").await,
            has_memory: self.memory_store.is_available(),
            has_message_actions: self.channel_router.is_some(),
            has_cron: self.scheduler.is_some(),
            has_yaml_tools: true,
            has_browser: Self::browser_renderer_url() != "disabled",
            has_host_exec: self.agent.base && self.sandbox.is_none(),
            is_base: self.agent.base,
        };

        let mut runtime = self.runtime_context(msg);
        runtime.channels = self.get_channel_info().await;
        let mut system_prompt = workspace::build_system_prompt(
            &ws_prompt,
            &mcp_schemas,
            &capabilities,
            &self.agent.language,
            &runtime,
        );

        // 4b. Skill matching removed — skills are now loaded on-demand via skill_use tool.

        // 4c. Skill capture prompt — if user requests saving approach as a skill
        {
            let msg_lower = user_text.to_lowercase();
            let is_capture_request =
                (msg_lower.contains("save") && msg_lower.contains("skill"))
                || (msg_lower.contains("сохрани") && (msg_lower.contains("навык") || msg_lower.contains("скилл")));
            if is_capture_request {
                system_prompt.push_str(
                    "\n\n## Skill Capture\n\
                     The user wants to save the approach from the previous task as a reusable skill.\n\
                     Use workspace_write to create a file in workspace/skills/ with YAML frontmatter \
                     (name, description, triggers, tools_required) and markdown body.\n\
                     Extract the strategy, not specific data.\n"
                );
            }
        }

        // 4d. Multi-agent session context (Phase 19: CTXA-02, CTXA-03)
        // When session has multiple participants, inform agent about collaboration context
        if let Ok(participants) = sessions::get_participants(&self.db, session_id).await
            && participants.len() > 1
        {
            system_prompt.push_str("\n\n## Multi-Agent Session\n");
            system_prompt.push_str("You are in a collaborative multi-agent session.\n\n");
            system_prompt.push_str("**Participants:** ");
            system_prompt.push_str(&participants.join(", "));
            system_prompt.push_str("\n\n");
            system_prompt.push_str("**CRITICAL RULE:** When another agent hands off to you or mentions you, ");
            system_prompt.push_str("you MUST respond to the question or task directly. ");
            system_prompt.push_str("Do NOT redirect back to the agent who called you. ");
            system_prompt.push_str("Do NOT say 'ask them directly'. Answer the question yourself.\n\n");
            system_prompt.push_str("**Forward handoff:** If the task requires ANOTHER agent's expertise ");
            system_prompt.push_str("(not the one who called you), use the `handoff` tool to delegate forward. ");
            system_prompt.push_str("Example: Agent A asks you to get info from Agent C — use handoff to Agent C.\n");
            system_prompt.push_str("Provide: `agent` (target name), `task` (what they should do), ");
            system_prompt.push_str("`context` (relevant facts — keep concise).\n");
        }

        // L0: Always-on pinned memory chunks — injected on every build_context call (CTX-01, CTX-02)
        let pinned_budget = self.app_config.memory.pinned_budget_tokens;
        let memory_ctx = self.build_memory_context(pinned_budget).await;
        if !memory_ctx.pinned_text.is_empty() {
            system_prompt.push_str(&memory_ctx.pinned_text);
        }
        // Store pinned IDs for L2 dedup (CTX-04)
        *self.pinned_chunk_ids.lock().await = memory_ctx.pinned_ids;

        tracing::info!(
            agent = %self.agent.name,
            prompt_bytes = system_prompt.len(),
            prompt_approx_tokens = system_prompt.len() / 4,
            "system_prompt_size"
        );

        // 5. Assemble messages
        let mut messages: Vec<Message> = vec![Message {
            role: MessageRole::System,
            content: system_prompt,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        }];

        for row in &history {
            // Filter out heartbeat-related messages from multi-agent context
            // These pollute the conversation history and confuse agents
            let content_lower = row.content.to_lowercase();
            if content_lower.contains("heartbeat_ok")
                || content_lower.contains("heartbeat ok")
                || (content_lower.contains("nothing to announce") && content_lower.len() < 100)
            {
                continue;
            }

            messages.push(row_to_message(row));
        }

        // Transcript repair — differential append scoped to last dangling assistant (ENG-01):
        // Instead of clearing messages + reloading from DB, extract missing call_ids from
        // the already-parsed messages and append synthetic results directly.
        if let Some(last_idx) = messages.iter().rposition(|m| {
            m.role == MessageRole::Assistant && m.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty())
        }) {
            let has_results = messages[last_idx + 1..].iter().any(|m| m.role == MessageRole::Tool);
            if !has_results {
                // Extract tool_call_ids from the last dangling assistant (already parsed by row_to_message / ENG-02)
                let all_call_ids: Vec<String> = messages[last_idx]
                    .tool_calls
                    .as_ref()
                    .map(|tcs| tcs.iter().map(|tc| tc.id.clone()).collect())
                    .unwrap_or_default();

                // Filter out any that already have a matching Tool message after last_idx
                let existing_ids: std::collections::HashSet<&str> = messages[last_idx + 1..]
                    .iter()
                    .filter(|m| m.role == MessageRole::Tool)
                    .filter_map(|m| m.tool_call_id.as_deref())
                    .collect();
                let missing_ids: Vec<String> = all_call_ids
                    .into_iter()
                    .filter(|id| !existing_ids.contains(id.as_str()))
                    .collect();

                if !missing_ids.is_empty() {
                    tracing::warn!(
                        session_id = %session_id,
                        count = missing_ids.len(),
                        "dangling tool calls detected — inserting synthetic results"
                    );

                    // Persist synthetic rows to DB (narrowed — no session-wide scan)
                    if let Err(e) = crate::db::sessions::insert_missing_tool_results(
                        &self.db, session_id, &missing_ids
                    ).await {
                        tracing::warn!(error = %e, "failed to insert synthetic tool results");
                    }

                    // Append synthetic tool results directly — no DB reload, no re-parse (ENG-01)
                    for call_id in missing_ids {
                        messages.push(Message {
                            role: MessageRole::Tool,
                            content: "[interrupted] Tool execution was interrupted (process restart). Result unavailable.".to_string(),
                            tool_calls: None,
                            tool_call_id: Some(call_id),
                            thinking_blocks: vec![],
                        });
                    }
                }
            }
        }

        // Sanitize any MiniMax XML tool calls that leaked into stored tool results.
        // Prevents old sessions with corrupt context from causing cascading XML loops.
        if messages.iter().any(|m| m.role == MessageRole::Tool && m.content.contains("<minimax:tool_call>")) {
            messages = messages
                .into_iter()
                .map(|mut m| {
                    if m.role == MessageRole::Tool {
                        m.content = strip_minimax_xml(&m.content);
                    }
                    m
                })
                .collect();
            tracing::warn!("sanitized MiniMax XML tool calls from session context");
        }

        // Proactive tool output pruning (turn-based) — before first LLM call.
        // Complements compact_tool_results (reactive, fires at 70% threshold in the tool loop).
        if let Some(keep_turns) = self.agent.session.as_ref().and_then(|s| s.prune_tool_output_after_turns)
            && keep_turns > 0 {
                messages = prune_old_tool_outputs(&messages, keep_turns);
                tracing::debug!(keep_turns, "proactive tool output pruning applied");
            }

        // 6. Available tools (if requested)
        let available_tools = if include_tools {
            let mut tools = self.internal_tool_definitions();
            // Shared YAML tools (workspace/tools/*.yaml) — use 30s cache to avoid per-request disk reads.
            let yaml_tools: Vec<crate::tools::yaml_tools::YamlToolDef> = {
                let cache = self.yaml_tools_cache.read().await;
                if cache.0.elapsed() < std::time::Duration::from_secs(30) && !cache.1.is_empty() {
                    cache.1.values().cloned().collect()
                } else {
                    drop(cache);
                    let loaded = crate::tools::yaml_tools::load_yaml_tools(&self.workspace_dir, false).await;
                    let map: std::collections::HashMap<String, crate::tools::yaml_tools::YamlToolDef> =
                        loaded.iter().cloned().map(|t| (t.name.clone(), t)).collect();
                    *self.yaml_tools_cache.write().await = (std::time::Instant::now(), map);
                    loaded
                }
            };
            let is_base = self.agent.base;
            let penalties = self.penalty_cache.get_penalties().await;
            let mut yaml_filtered: Vec<_> = yaml_tools
                .into_iter()
                .filter(|t| !t.required_base || is_base)
                .collect();
            yaml_filtered.sort_by(|a, b| {
                let pa = penalties.get(&a.name).copied().unwrap_or(1.0);
                let pb = penalties.get(&b.name).copied().unwrap_or(1.0);
                pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal)
            });
            tools.extend(yaml_filtered.into_iter().map(|t| t.to_tool_definition()));
            if let Some(ref mcp) = self.mcp {
                tools.extend(mcp.all_tool_definitions().await);
            }
            let mut all_tools = self.filter_tools_by_policy(tools);

            // Dynamic top-K: if configured and tool count exceeds the limit, select most relevant
            if let Some(max_k) = self.agent.max_tools_in_context
                && all_tools.len() > max_k && !user_text.is_empty() {
                    all_tools = self.select_top_k_tools_semantic(all_tools, &user_text, max_k).await;
                }

            all_tools
        } else {
            vec![]
        };

        Ok((session_id, messages, available_tools))
    }

    /// Handle with optional status callback for real-time phase updates.
    /// `chunk_tx` — optional channel for streaming response chunks to the caller (e.g. progressive display).
    pub async fn handle_with_status(
        &self,
        msg: &IncomingMessage,
        status_tx: Option<mpsc::UnboundedSender<ProcessingPhase>>,
        chunk_tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<String> {
        // Sweep stale approval waiters (older than 10 minutes)
        {
            let mut waiters = self.approval_waiters.write().await;
            let now = std::time::Instant::now();
            waiters.retain(|id, (_, created)| {
                let stale = now.duration_since(*created) > std::time::Duration::from_secs(600);
                if stale {
                    tracing::debug!(approval_id = %id, "evicting stale approval waiter (>10min)");
                }
                !stale
            });
        }

        // Pause graph extraction worker while chat is active
        let _chat_guard = crate::graph_worker::ChatActiveGuard::new();

        // Hook: BeforeMessage
        if let super::hooks::HookAction::Block(reason) = self.hooks.fire(&super::hooks::HookEvent::BeforeMessage) {
            anyhow::bail!("blocked by hook: {}", reason);
        }

        let (session_id, mut messages, available_tools) =
            self.build_context(msg, true, None, false).await?;

        // Store session_id for tool handlers that need session context (e.g., handoff)
        *self.processing_session_id.lock().await = Some(session_id);

        // Mark session as running — watchdog and startup cleanup use this
        if let Err(e) = crate::db::sessions::set_session_run_status(&self.db, session_id, "running").await {
            tracing::warn!(session_id = %session_id, error = %e, "failed to mark session as running");
        }
        // RAII guard: if we exit early via `?` (error path), mark session as 'failed'.
        let mut lifecycle_guard = SessionLifecycleGuard::new(self.db.clone(), session_id);

        // Broadcast processing start to UI (typing indicator) + guard broadcasts end on drop
        let start_event = serde_json::json!({
            "type": "agent_processing",
            "agent": self.agent.name,
            "session_id": session_id.to_string(),
            "status": "start",
            "channel": msg.channel,
        });
        self.broadcast_ui_event(start_event.clone());
        let _processing_guard = ProcessingGuard::new(
            self.ui_event_tx.clone(),
            self.processing_tracker.clone(),
            self.agent.name.clone(),
            &start_event,
        );

        // invite_agent removed (v3.0) — handoff is the only inter-agent tool

        // Add current message, auto-fetch URLs if present
        let user_text = msg.text.clone().unwrap_or_default();

        // Slash commands — handle without LLM
        if let Some(result) = self.handle_command(&user_text, msg).await {
            lifecycle_guard.done().await;
            return result;
        }

        let enriched_text = self.enrich_message_text(&user_text, &msg.attachments).await;

        // Prompt injection detection (logging-only)
        let injections = crate::tools::content_security::detect_prompt_injection(&enriched_text);
        if !injections.is_empty() {
            tracing::warn!(patterns = ?injections, "potential prompt injection detected");
            let preview = Self::truncate_preview(&enriched_text, 200);
            self.audit(crate::db::audit::event_types::PROMPT_INJECTION, msg.context.get("user_id").and_then(|v| v.as_str()), serde_json::json!({
                "patterns": injections, "text_preview": preview
            }));
        }

        messages.push(Message {
            role: MessageRole::User,
            content: enriched_text,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        });

        // Save user message (original, not enriched)
        // For inter-agent messages (user_id starts with "agent:"), save the sender agent_id
        let sender_agent_id = if msg.user_id.starts_with("agent:") { Some(msg.user_id.trim_start_matches("agent:")) } else { None };
        sessions::save_message_ex(&self.db, session_id, "user", &user_text, None, None, sender_agent_id, None).await?;

        // Context compaction if needed (model-aware token budget)
        self.compact_messages(&mut messages).await;

        // LLM loop (with tool calls)
        let mut final_response = String::new();
        let mut final_thinking_blocks: Vec<hydeclaw_types::ThinkingBlock> = vec![];
        let mut streamed_via_chunk_tx = false;
        let mut total_input_tokens: u32 = 0;
        let mut total_output_tokens: u32 = 0;
        let mut tool_iterations: u32 = 0;
        let loop_config = self.tool_loop_config();
        let mut detector = LoopDetector::new(&loop_config);
        let mut did_reset_session = false;
        let mut empty_retry_count: u8 = 0;
        let mut auto_continue_count: u8 = 0;
        let mut context_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        let mut consecutive_failures: usize = 0;
        let mut using_fallback = false;
        let mut fallback_provider: Option<Arc<dyn super::providers::LlmProvider>> = None;

        for iteration in 0..loop_config.effective_max_iterations() {
            if let Some(ref tx) = status_tx {
                tx.send(ProcessingPhase::Thinking).ok();
            }

            // Compact old tool results if context is getting full
            self.compact_tool_results(&mut messages, &mut context_chars);

            // Use streaming if chunk_tx available (enables progressive display)
            let llm_result = if let Some(tx) = &chunk_tx {
                if let Some(ref fb) = fallback_provider {
                    self.chat_stream_with_transient_retry_using(fb, &mut messages, &available_tools, tx.clone()).await
                } else {
                    self.chat_stream_with_transient_retry(&mut messages, &available_tools, tx.clone()).await
                }
            } else if let Some(ref fb) = fallback_provider {
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
                    let class = error_classify::classify(&e);
                    // Auto-reset on session corruption (once)
                    if class == error_classify::LlmErrorClass::SessionCorruption && !did_reset_session {
                        did_reset_session = true;
                        tracing::warn!(error = %e, "session corrupted, resetting context and retrying");
                        messages.retain(|m| m.role == MessageRole::System);
                        messages.push(Message {
                            role: MessageRole::User,
                            content: user_text.clone(),
                            tool_calls: None,
                            tool_call_id: None,
                            thinking_blocks: vec![],
                        });
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
                    tracing::error!(error = %e, iteration, "LLM call failed, returning fallback");
                    final_response = error_classify::format_user_error(&e);
                    break;
                }
            };
            self.record_usage(&response, Some(session_id));
            if let Some(ref u) = response.usage {
                total_input_tokens += u.input_tokens;
                total_output_tokens += u.output_tokens;
            }

            if response.tool_calls.is_empty() {
                final_response = if let Some(notice) = &response.fallback_notice {
                    format!("{}\n\n{}", notice, response.content)
                } else {
                    response.content.clone()
                };
                final_thinking_blocks = response.thinking_blocks.clone();
                if final_response.is_empty() && empty_retry_count < 1 {
                    empty_retry_count += 1;
                    tracing::warn!(iteration, "LLM returned empty response, retrying once");
                    continue;
                }
                if final_response.is_empty() {
                    tracing::warn!(iteration, "LLM returned empty response after retry");
                }

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
                                    &format!("Auto-continue: {}", agent_name),
                                    &format!("Agent continued unfinished task (attempt {}/{})", cnt, max),
                                    serde_json::json!({"agent": agent_name}),
                                ).await.ok();
                            });
                        }
                    }
                    if let Some(ref tx) = chunk_tx {
                        tx.send("\n\n...".to_string()).ok();
                    }
                    messages.push(Message {
                        role: MessageRole::User,
                        content: AUTO_CONTINUE_NUDGE.to_string(),
                        tool_calls: None,
                        tool_call_id: None,
                        thinking_blocks: vec![],
                    });
                    context_chars += AUTO_CONTINUE_NUDGE.len();
                    continue;
                }

                if chunk_tx.is_some() {
                    streamed_via_chunk_tx = true;
                }
                break;
            }

            tracing::info!(
                iteration,
                max = loop_config.effective_max_iterations(),
                tools = response.tool_calls.len(),
                "executing tool calls"
            );

            let cleaned_content = strip_thinking(&response.content);

            // Send intermediate text to channel (so Telegram shows progress)
            if let Some(ref tx) = chunk_tx
                && !cleaned_content.is_empty() {
                    tx.send(cleaned_content.clone()).ok();
                }

            messages.push(Message {
                role: MessageRole::Assistant,
                content: cleaned_content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
                thinking_blocks: vec![],
            });
            context_chars += cleaned_content.chars().count();

            // Save assistant message with tool_calls to DB (thinking stripped)
            let tc_json = serde_json::to_value(&response.tool_calls).ok();
            if let Err(e) = sessions::save_message(
                &self.db,
                session_id,
                "assistant",
                &cleaned_content,
                tc_json.as_ref(),
                None,
            )
            .await {
                tracing::warn!(error = %e, session_id = %session_id, "failed to save assistant message to DB");
            }

            if let Some(ref tx) = status_tx
                && let Some(tc) = response.tool_calls.first() {
                    tx.send(ProcessingPhase::CallingTool(tc.name.clone())).ok();
                }
            tool_iterations += 1;
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
                        if let Err(e) = sessions::save_message(
                            &self.db, session_id, "tool", tool_result, None, Some(tc_id),
                        ).await {
                            tracing::warn!(error = %e, session_id = %session_id, "failed to save tool result to DB");
                        }
                    }
                    false
                }
                Err(_) => true,
            };

            if loop_broken || iteration == loop_config.effective_max_iterations() - 1 {
                // Forced final call — use streaming if chunk_tx is available
                let forced_result = if let Some(ref tx) = chunk_tx {
                    self.provider.chat_stream(&messages, &[], tx.clone()).await
                } else {
                    self.provider.chat(&messages, &[]).await
                };
                match forced_result {
                    Ok(forced) => {
                        final_response = forced.content;
                        if chunk_tx.is_some() { streamed_via_chunk_tx = true; }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "forced final LLM call failed");
                        final_response = error_classify::format_user_error(&e);
                    }
                }
                break;
            }
        }

        if let Some(ref tx) = status_tx {
            tx.send(ProcessingPhase::Composing).ok();
        }

        // Skill capture hint: suggest saving multi-step approach as a skill
        if tool_iterations >= 3
            && !super::channel_kind::channel::is_automated(&msg.channel)
            && !final_response.is_empty()
        {
            final_response.push_str(
                "\n\n---\n_This task used a multi-step approach not covered by any skill. \
                To save it for reuse, say: \"save as skill\" / \"сохрани как навык\"._"
            );
        }

        let thinking_level = self.thinking_level.load(std::sync::atomic::Ordering::Relaxed);
        let final_response = maybe_strip_thinking(&final_response, msg, thinking_level);

        // Send final response to chunk consumer (if not already streamed)
        if let Some(ref tx) = chunk_tx
            && !streamed_via_chunk_tx && !final_response.is_empty() {
                tx.send(final_response.clone()).ok();
            }

        let thinking_json = if final_thinking_blocks.is_empty() {
            None
        } else {
            serde_json::to_value(&final_thinking_blocks).ok()
        };
        sessions::save_message_ex(&self.db, session_id, "assistant", &final_response, None, None, Some(&self.agent.name), thinking_json.as_ref())
            .await?;
        self.maybe_trim_session(session_id).await;

        // Append usage footer (only for non-streaming, not saved to DB)
        let with_footer = if total_input_tokens > 0 && chunk_tx.is_none() {
            format!("{}\n\n---\n📊 {}→{} tokens", final_response, total_input_tokens, total_output_tokens)
        } else {
            final_response
        };

        lifecycle_guard.done().await;

        // Clear processing session context
        *self.processing_session_id.lock().await = None;

        // Post-session graph extraction (background, non-blocking)
        if self.memory_store.is_available() && messages.len() >= 5 {
            let db = self.db.clone();
            let provider = self.provider.clone();
            let sid = session_id;
            let msgs = std::sync::Arc::new(messages);
            tokio::spawn(async move {
                if let Err(e) = memory_impl::extract_session_to_graph(&db, &provider, sid, msgs).await {
                    tracing::debug!(session = %sid, error = %e, "post-session graph extraction skipped");
                }
            });
        }

        Ok(with_footer)
    }

    /// Handle with streaming: sends content chunks via mpsc channel for SSE or progressive display.
    pub async fn handle_streaming(
        &self,
        msg: &IncomingMessage,
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let thinking_level = self.thinking_level.load(std::sync::atomic::Ordering::Relaxed);
        let (session_id, mut messages, _) = self.build_context(msg, false, None, false).await?;

        // Lifecycle tracking
        if let Err(e) = crate::db::sessions::set_session_run_status(&self.db, session_id, "running").await {
            tracing::warn!(session_id = %session_id, error = %e, "failed to mark streaming session as running");
        }
        let mut lifecycle_guard = SessionLifecycleGuard::new(self.db.clone(), session_id);

        let user_text = msg.text.clone().unwrap_or_default();
        messages.push(Message {
            role: MessageRole::User,
            content: user_text.clone(),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        });

        // For inter-agent messages (user_id starts with "agent:"), save the sender agent_id
        let sender_agent_id = if msg.user_id.starts_with("agent:") { Some(msg.user_id.trim_start_matches("agent:")) } else { None };
        sessions::save_message_ex(&self.db, session_id, "user", &user_text, None, None, sender_agent_id, None).await?;

        // Stream LLM response (no tools for streaming — simple text response)
        let (final_response, stream_thinking_json) = match self.provider.chat_stream(&messages, &[], chunk_tx).await {
            Ok(response) => {
                let tb_json = if response.thinking_blocks.is_empty() {
                    None
                } else {
                    serde_json::to_value(&response.thinking_blocks).ok()
                };
                (maybe_strip_thinking(&response.content, msg, thinking_level), tb_json)
            }
            Err(e) => {
                tracing::error!(error = %e, "streaming LLM call failed, returning fallback");
                let reason_str = format!("streaming LLM call failed: {e}");
                lifecycle_guard.fail(&reason_str).await;
                {
                    let db = self.db.clone();
                    let agent_name = self.agent.name.clone();
                    let rs = reason_str.clone();
                    if let Some(ref ui_tx) = self.ui_event_tx {
                        let tx = ui_tx.clone();
                        tokio::spawn(async move {
                            crate::gateway::notify(
                                &db, &tx, "agent_error",
                                "Agent Error",
                                &format!("Agent {} run failed: {}", agent_name, rs),
                                serde_json::json!({"agent": agent_name, "reason": rs}),
                            ).await.ok();
                        });
                    }
                }
                (error_classify::format_user_error(&e), None)
            }
        };

        sessions::save_message_ex(&self.db, session_id, "assistant", &final_response, None, None, Some(&self.agent.name), stream_thinking_json.as_ref())
            .await?;
        self.maybe_trim_session(session_id).await;

        lifecycle_guard.done().await;

        // Post-session graph extraction (background)
        if self.memory_store.is_available() && messages.len() >= 5 {
            let db = self.db.clone();
            let provider = self.provider.clone();
            let sid = session_id;
            let msgs = std::sync::Arc::new(messages);
            tokio::spawn(async move {
                if let Err(e) = memory_impl::extract_session_to_graph(&db, &provider, sid, msgs).await {
                    tracing::debug!(session = %sid, error = %e, "post-session graph extraction skipped");
                }
            });
        }

        Ok(final_response)
    }

    /// Handle message via SSE: emits StreamEvents for AI SDK UI Message Stream Protocol v1.
    /// Supports tool execution, session continuation, and real-time status updates.
    pub async fn handle_sse(
        &self,
        msg: &IncomingMessage,
        event_tx: mpsc::UnboundedSender<StreamEvent>,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<()> {
        let _chat_guard = crate::graph_worker::ChatActiveGuard::new();

        // Hook: BeforeMessage
        if let super::hooks::HookAction::Block(reason) = self.hooks.fire(&super::hooks::HookEvent::BeforeMessage) {
            anyhow::bail!("blocked by hook: {}", reason);
        }

        // Handle slash commands (no LLM needed)
        let user_text = msg.text.clone().unwrap_or_default();
        if let Some(result) = self.handle_command(&user_text, msg).await {
            let text = result?;
            let msg_id = format!("msg_{}", Uuid::new_v4());
            if event_tx.send(StreamEvent::MessageStart { message_id: msg_id }).is_err() {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }
            if event_tx.send(StreamEvent::TextDelta(text)).is_err() {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }
            if event_tx.send(StreamEvent::Finish { finish_reason: "command".to_string() }).is_err() {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }
            return Ok(());
        }

        let thinking_level = self.thinking_level.load(std::sync::atomic::Ordering::Relaxed);

        let (session_id, mut messages, available_tools) =
            self.build_context(msg, true, resume_session_id, force_new_session).await?;

        // Store session_id for tool handlers that need session context (e.g., handoff)
        *self.processing_session_id.lock().await = Some(session_id);

        // Lifecycle tracking: mark running, RAII guard marks 'failed' on early exit
        if let Err(e) = crate::db::sessions::set_session_run_status(&self.db, session_id, "running").await {
            tracing::warn!(session_id = %session_id, error = %e, "failed to mark SSE session as running");
        }
        let mut lifecycle_guard = SessionLifecycleGuard::new(self.db.clone(), session_id);

        // Emit session ID so the UI can track which session is active
        if event_tx.send(StreamEvent::SessionId(session_id.to_string())).is_err() {
            tracing::debug!("SSE event channel closed, engine continues for DB save");
        }

        // Broadcast processing start + guard broadcasts end on drop
        let start_event = serde_json::json!({
            "type": "agent_processing",
            "agent": self.agent.name,
            "session_id": session_id.to_string(),
            "status": "start",
            "channel": msg.channel,
        });
        self.broadcast_ui_event(start_event.clone());
        let _processing_guard = ProcessingGuard::new(
            self.ui_event_tx.clone(),
            self.processing_tracker.clone(),
            self.agent.name.clone(),
            &start_event,
        );

        // Add current message, auto-fetch URLs if present
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
        sessions::save_message_ex(&self.db, session_id, "user", &user_text, None, None, sender_agent_id, None).await?;

        // Context compaction if needed (model-aware token budget)
        self.compact_messages(&mut messages).await;

        // Emit message start
        let message_id = format!("msg_{}", Uuid::new_v4());
        if event_tx
            .send(StreamEvent::MessageStart {
                message_id: message_id.clone(),
            })
            .is_err()
        {
            tracing::debug!("SSE event channel closed, engine continues for DB save");
        }

        // LLM loop with tool calls
        let mut final_response = String::new();
        let mut final_thinking_blocks: Vec<hydeclaw_types::ThinkingBlock> = vec![];
        let loop_config = self.tool_loop_config();
        let mut detector = LoopDetector::new(&loop_config);
        let mut did_reset_session = false;
        let mut empty_retry_count: u8 = 0;
        let mut auto_continue_count: u8 = 0;
        let mut context_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        let mut consecutive_failures: usize = 0;
        let mut using_fallback = false;
        let mut fallback_provider: Option<Arc<dyn super::providers::LlmProvider>> = None;

        for iteration in 0..loop_config.effective_max_iterations() {
            let step_id = format!("step_{}", iteration);
            if event_tx
                .send(StreamEvent::StepStart {
                    step_id: step_id.clone(),
                })
                .is_err()
            {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }

            self.compact_tool_results(&mut messages, &mut context_chars);
            // Per-iteration streaming channel: forwards LLM token chunks to SSE event stream.
            // chat_stream sends tokens only for text responses (not tool-call responses).
            let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<String>();
            let event_tx_fwd = event_tx.clone();
            tokio::spawn(async move {
                while let Some(chunk) = chunk_rx.recv().await {
                    if event_tx_fwd.send(StreamEvent::TextDelta(chunk)).is_err() {
                        tracing::debug!("SSE forwarder: event channel closed");
                    }
                }
            });
            let llm_result = if let Some(ref fb) = fallback_provider {
                self.chat_stream_with_transient_retry_using(fb, &mut messages, &available_tools, chunk_tx).await
            } else {
                self.chat_stream_with_transient_retry(&mut messages, &available_tools, chunk_tx).await
            };
            let response = match llm_result {
                Ok(r) => {
                    consecutive_failures = 0;
                    r
                }
                Err(e) => {
                    if error_classify::classify(&e) == error_classify::LlmErrorClass::SessionCorruption && !did_reset_session {
                        did_reset_session = true;
                        tracing::warn!(error = %e, "SSE session corrupted, resetting context");
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
                                "switching to fallback provider after consecutive failures (SSE)"
                            );
                            if event_tx.send(StreamEvent::StepFinish { step_id, finish_reason: "fallback".into() }).is_err() {
                                tracing::debug!("SSE event channel closed, engine continues for DB save");
                            }
                            continue;
                        }
                    }
                    // AUDIT:SSE-02 (verified 2026-03-30): LLM errors mid-stream are delivered
                    // as TextDelta (not StreamEvent::Error) via format_user_error(). This is
                    // intentional: the error appears inline in chat history so the user sees it
                    // as a visible message. The engine then sends Finish, ensuring the SSE stream
                    // terminates cleanly. StreamEvent::Error is reserved for top-level handle_sse
                    // failures (caught in chat.rs spawned task).
                    tracing::error!(error = %e, iteration, "SSE LLM call failed, returning fallback");
                    let fallback = error_classify::format_user_error(&e);
                    if event_tx.send(StreamEvent::TextDelta(fallback.clone())).is_err() {
                        tracing::debug!("SSE event channel closed, engine continues for DB save");
                    }
                    final_response = fallback;
                    if event_tx.send(StreamEvent::StepFinish { step_id, finish_reason: "error".into() }).is_err() {
                        tracing::debug!("SSE event channel closed, engine continues for DB save");
                    }
                    let reason_str = format!("SSE LLM call failed: {e}");
                    lifecycle_guard.fail(&reason_str).await;
                    {
                        let db = self.db.clone();
                        let agent_name = self.agent.name.clone();
                        let rs = reason_str.clone();
                        if let Some(ref ui_tx) = self.ui_event_tx {
                            let tx = ui_tx.clone();
                            tokio::spawn(async move {
                                crate::gateway::notify(
                                    &db, &tx, "agent_error",
                                    "Agent Error",
                                    &format!("Agent {} run failed: {}", agent_name, rs),
                                    serde_json::json!({"agent": agent_name, "reason": rs}),
                                ).await.ok();
                            });
                        }
                    }
                    break;
                }
            };
            self.record_usage(&response, Some(session_id));

            if response.tool_calls.is_empty() {
                // Final text response — tokens already streamed via chunk_tx forwarder.
                // Only strip thinking for DB save; do NOT re-send as TextDelta.
                final_response = maybe_strip_thinking(&response.content, msg, thinking_level);
                final_thinking_blocks = response.thinking_blocks.clone();

                // Auto-continue: if LLM described remaining work, nudge it to execute.
                // In SSE mode, the "incomplete" text was already streamed — send visible marker.
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
                                    &format!("Auto-continue: {}", agent_name),
                                    &format!("Agent continued unfinished task (attempt {}/{})", cnt, max),
                                    serde_json::json!({"agent": agent_name}),
                                ).await.ok();
                            });
                        }
                    }
                    let _ = event_tx.send(StreamEvent::TextDelta("\n\n...".to_string()));
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
                if event_tx
                    .send(StreamEvent::StepFinish {
                        step_id,
                        finish_reason: "stop".into(),
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }
                break;
            }

            tracing::info!(
                iteration,
                max = loop_config.effective_max_iterations(),
                tools = response.tool_calls.len(),
                "executing tool calls (SSE)"
            );

            // Strip <think> blocks for DB save and LLM context.
            // NOTE: content was already streamed token-by-token via chunk_tx forwarder above;
            // do NOT re-send as TextDelta here — it would duplicate text in the UI.
            let cleaned_content = maybe_strip_thinking(&response.content, msg, thinking_level);

            messages.push(Message {
                role: MessageRole::Assistant,
                content: cleaned_content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
                thinking_blocks: vec![],
            });
            context_chars += cleaned_content.chars().count();

            let tc_json = serde_json::to_value(&response.tool_calls).ok();
            if let Err(e) = sessions::save_message(
                &self.db,
                session_id,
                "assistant",
                &cleaned_content,
                tc_json.as_ref(),
                None,
            )
            .await {
                tracing::warn!(error = %e, session_id = %session_id, "failed to save assistant message to DB");
            }

            // Emit ToolCallStart/ToolCallArgs for ALL tools before executing
            for tc in &response.tool_calls {
                if event_tx
                    .send(StreamEvent::ToolCallStart {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }
                let args_text = serde_json::to_string(&tc.arguments).unwrap_or_default();
                if event_tx
                    .send(StreamEvent::ToolCallArgs {
                        id: tc.id.clone(),
                        args_text,
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }
            }

            let loop_broken = match self.execute_tool_calls_partitioned(
                &response.tool_calls, &msg.context, session_id, &msg.channel,
                messages.iter().map(|m| m.content.len()).sum(),
                &mut detector, loop_config.detect_loops,
            ).await {
                Ok(results) => {
                    for (tc_id, tool_result) in &results {
                        // Extract RichCard / File markers for SSE events
                        let (display_result, db_result) = if let Some(json_str) = tool_result.strip_prefix(RICH_CARD_PREFIX) {
                            if let Ok(data) = serde_json::from_str::<serde_json::Value>(json_str) {
                                let card_type = data.get("card_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("table")
                                    .to_string();
                                if event_tx.send(StreamEvent::RichCard { card_type, data }).is_err() {
                                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                                }
                            }
                            ("Rich card displayed".to_string(), "Rich card displayed".to_string())
                        } else if tool_result.contains(FILE_PREFIX) {
                            let db_result = tool_result.clone();
                            let mut clean_lines = Vec::new();
                            for line in tool_result.lines() {
                                if let Some(json_str) = line.strip_prefix(FILE_PREFIX) {
                                    if let Ok(meta) = serde_json::from_str::<serde_json::Value>(json_str) {
                                        let url = meta.get("url").and_then(|v| v.as_str()).unwrap_or("");
                                        let media_type = meta.get("mediaType").and_then(|v| v.as_str()).unwrap_or("application/octet-stream");
                                        if !url.is_empty()
                                            && event_tx.send(StreamEvent::File { url: url.to_string(), media_type: media_type.to_string() }).is_err() {
                                                tracing::debug!("SSE event channel closed, engine continues for DB save");
                                            }
                                    }
                                } else {
                                    clean_lines.push(line.as_ref());
                                }
                            }
                            let text = clean_lines.join("\n");
                            let display_result = if text.is_empty() { "Image displayed inline in the chat. Do NOT use canvas or other tools to show it again.".to_string() } else { text };
                            (display_result, db_result)
                        } else {
                            (tool_result.clone(), tool_result.clone())
                        };

                        if event_tx
                            .send(StreamEvent::ToolResult {
                                id: tc_id.clone(),
                                result: display_result.clone(),
                            })
                            .is_err()
                        {
                            tracing::debug!("SSE event channel closed, engine continues for DB save");
                        }

                        let display_len = display_result.chars().count();
                        messages.push(Message {
                            role: MessageRole::Tool,
                            content: display_result,
                            tool_calls: None,
                            tool_call_id: Some(tc_id.clone()),
                            thinking_blocks: vec![],
                        });
                        context_chars += display_len;

                        if let Err(e) = sessions::save_message(
                            &self.db, session_id, "tool", &db_result, None, Some(tc_id),
                        ).await {
                            tracing::warn!(error = %e, session_id = %session_id, "failed to save tool result to DB");
                        }
                    }
                    false
                }
                Err(_) => true,
            };

            if event_tx
                .send(StreamEvent::StepFinish {
                    step_id,
                    finish_reason: "tool-calls".into(),
                })
                .is_err()
            {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }

            // Forced final call on last iteration or loop break
            if loop_broken || iteration == loop_config.effective_max_iterations() - 1 {
                let step_id = format!("step_{}", iteration + 1);
                if event_tx
                    .send(StreamEvent::StepStart {
                        step_id: step_id.clone(),
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }

                match self.provider.chat(&messages, &[]).await {
                    Ok(forced) => {
                        self.record_usage(&forced, Some(session_id));
                        let text = maybe_strip_thinking(&forced.content, msg, thinking_level);
                        if !text.is_empty()
                            && event_tx.send(StreamEvent::TextDelta(text.clone())).is_err() {
                                tracing::debug!("SSE event channel closed, engine continues for DB save");
                            }
                        final_response = text;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "SSE forced final LLM call failed");
                        let fallback = error_classify::format_user_error(&e);
                        if event_tx.send(StreamEvent::TextDelta(fallback.clone())).is_err() {
                            tracing::debug!("SSE event channel closed, engine continues for DB save");
                        }
                        final_response = fallback;
                        let reason_str = format!("SSE forced final LLM call failed: {e}");
                        lifecycle_guard.fail(&reason_str).await;
                        {
                            let db = self.db.clone();
                            let agent_name = self.agent.name.clone();
                            let rs = reason_str.clone();
                            if let Some(ref ui_tx) = self.ui_event_tx {
                                let tx = ui_tx.clone();
                                tokio::spawn(async move {
                                    crate::gateway::notify(
                                        &db, &tx, "agent_error",
                                        "Agent Error",
                                        &format!("Agent {} run failed: {}", agent_name, rs),
                                        serde_json::json!({"agent": agent_name, "reason": rs}),
                                    ).await.ok();
                                });
                            }
                        }
                    }
                }
                if event_tx
                    .send(StreamEvent::StepFinish {
                        step_id,
                        finish_reason: "stop".into(),
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }
                break;
            }
        }

        // Save final response with agent_id for multi-agent identity
        let thinking_json = if final_thinking_blocks.is_empty() {
            None
        } else {
            serde_json::to_value(&final_thinking_blocks).ok()
        };
        sessions::save_message_ex(&self.db, session_id, "assistant", &final_response, None, None, Some(&self.agent.name), thinking_json.as_ref())
            .await?;
        self.maybe_trim_session(session_id).await;

        if event_tx
            .send(StreamEvent::Finish {
                finish_reason: "stop".into(),
            })
            .is_err()
        {
            tracing::debug!("SSE event channel closed, engine continues for DB save");
        }

        lifecycle_guard.done().await;

        // Clear processing session context
        *self.processing_session_id.lock().await = None;

        // Post-session graph extraction (background)
        if self.memory_store.is_available() && messages.len() >= 5 {
            let db = self.db.clone();
            let provider = self.provider.clone();
            let sid = session_id;
            let msgs = std::sync::Arc::new(messages);
            tokio::spawn(async move {
                if let Err(e) = memory_impl::extract_session_to_graph(&db, &provider, sid, msgs).await {
                    tracing::debug!(session = %sid, error = %e, "post-session graph extraction skipped");
                }
            });
        }

        Ok(())
    }

    /// Build a SecretsEnvResolver for YAML tool env resolution.
    fn make_resolver(&self) -> SecretsEnvResolver {
        SecretsEnvResolver {
            secrets: self.secrets.clone(),
            agent_name: self.agent.name.clone(),
        }
    }

    /// Build OAuthContext for provider-based YAML tool auth (e.g. `oauth_provider: github`).
    fn make_oauth_context(&self) -> Option<crate::tools::yaml_tools::OAuthContext> {
        self.oauth.as_ref().map(|mgr| crate::tools::yaml_tools::OAuthContext {
            manager: mgr.clone(),
            agent_id: self.agent.name.clone(),
        })
    }

    /// Truncate a string to `max` chars with "..." suffix, preserving char boundaries.
    /// Format a tool error as structured JSON for better LLM parsing.
    fn format_tool_error(tool_name: &str, error: &str) -> String {
        serde_json::json!({"status": "error", "tool": tool_name, "error": error}).to_string()
    }

    fn truncate_preview(s: &str, max: usize) -> String {
        if s.len() > max {
            format!("{}...", &s[..s.floor_char_boundary(max)])
        } else {
            s.to_string()
        }
    }

    /// Truncate a tool result to fit within remaining context budget.
    /// Preserves head + tail (tail may contain errors/JSON closing).
    /// Budget: 50% of remaining context, floor 2000 chars.
    fn truncate_tool_result(&self, result: &str, current_context_chars: usize) -> String {
        let model_max_chars = Self::default_context_for_model(&self.agent.model) * 4;
        let remaining = model_max_chars.saturating_sub(current_context_chars);
        let limit = (remaining * 50 / 100).max(2000);
        if result.len() <= limit {
            return result.to_string();
        }
        let tail_region = &result[result.len().saturating_sub(1500)..];
        let tail_has_error = tail_region.contains("error") || tail_region.contains("Error")
            || tail_region.contains("failed") || tail_region.contains("exception");
        let tail_size = if tail_has_error { 1500 } else { 500 };
        let marker = format!("\n\n[... truncated {} → {} chars ...]\n\n", result.len(), limit);
        let head_size = limit.saturating_sub(tail_size).saturating_sub(marker.len());
        let head = &result[..result.floor_char_boundary(head_size)];
        let tail = &result[result.floor_char_boundary(result.len().saturating_sub(tail_size))..];
        tracing::debug!(original = result.len(), truncated = limit, tail_has_error, "tool result truncated");
        format!("{}{}{}", head, marker, tail)
    }

    /// Replace old tool results with "[compacted]" when context exceeds 70% of model window.
    /// Preserves the last `preserve_n` tool results and the system message.
    /// `context_chars` is a running total of character counts across all messages,
    /// maintained incrementally by the caller. Updated in place after compaction.
    fn compact_tool_results(&self, messages: &mut [Message], context_chars: &mut usize) {
        let context_window = Self::default_context_for_model(&self.agent.model) * 4;
        let threshold = context_window * 70 / 100;
        if *context_chars <= threshold {
            return;
        }
        let preserve_n = self.agent.compaction.as_ref()
            .map(|c| c.preserve_last_n as usize)
            .unwrap_or(10);

        // Count tool messages, compact oldest ones
        let tool_indices: Vec<usize> = messages.iter().enumerate()
            .filter(|(_, m)| m.role == MessageRole::Tool)
            .map(|(i, _)| i)
            .collect();
        let to_compact = tool_indices.len().saturating_sub(preserve_n);
        if to_compact == 0 { return; }

        let mut compacted = 0usize;
        let mut chars_removed = 0usize;
        for &idx in tool_indices.iter().take(to_compact) {
            let old_len = messages[idx].content.chars().count();
            if old_len > 100 {
                let replacement = "[tool result compacted]";
                let new_len = replacement.len(); // 23 chars, all ASCII
                chars_removed += old_len - new_len;
                messages[idx].content = replacement.to_string();
                compacted += 1;
            }
        }
        if compacted > 0 {
            let old_total = *context_chars;
            *context_chars = context_chars.saturating_sub(chars_removed);
            tracing::info!(compacted, old_chars = old_total, new_chars = *context_chars, "compacted old tool results");
        }
    }

    /// Get compaction parameters from agent config.
    fn compaction_params(&self) -> (usize, usize) {
        let max_tokens = self.agent.compaction.as_ref()
            .and_then(|c| c.max_context_tokens)
            .map(|t| t as usize)
            .unwrap_or_else(|| Self::default_context_for_model(&self.agent.model));
        let preserve_last_n = self.agent.compaction.as_ref()
            .map(|c| c.preserve_last_n as usize)
            .unwrap_or(10);
        (max_tokens, preserve_last_n)
    }

    /// Run compaction on messages if token budget exceeded, indexing extracted facts to memory.
    async fn compact_messages(&self, messages: &mut Vec<Message>) {
        let (max_tokens, preserve_last_n) = self.compaction_params();
        if let Ok(Some(facts)) = history::compact_if_needed(
            messages,
            self.provider.as_ref(),
            self.compaction_provider.as_deref(),
            max_tokens,
            preserve_last_n,
            Some(&self.agent.language),
        )
        .await
        {
            tracing::info!(facts = facts.len(), "extracted facts during compaction");
            self.audit(crate::db::audit::event_types::COMPACTION, None, serde_json::json!({"facts": facts.len(), "max_tokens": max_tokens}));
            self.index_facts_to_memory(&facts).await;
            // Notify user about compaction
            if let Some(ref ui_tx) = self.ui_event_tx {
                let db = self.db.clone();
                let tx = ui_tx.clone();
                let agent_name = self.agent.name.clone();
                tokio::spawn(async move {
                    crate::gateway::notify(
                        &db, &tx, "context_compaction",
                        &format!("Context compacted: {}", agent_name),
                        &format!("Agent {} session was compacted to stay within token budget", agent_name),
                        serde_json::json!({"agent": agent_name}),
                    ).await.ok();
                });
            }
        }
    }

    /// Compact a specific session's messages via API.
    /// Returns `(facts_extracted, new_message_count)`.
    pub async fn compact_session(&self, session_id: uuid::Uuid) -> Result<(usize, usize)> {
        let rows = crate::db::sessions::load_messages(&self.db, session_id, Some(2000)).await?;
        if rows.len() < 4 {
            anyhow::bail!("session too short to compact ({} messages)", rows.len());
        }

        let mut messages: Vec<Message> = rows.iter().map(row_to_message).collect();

        // Force compaction by using max_tokens=1 (threshold=0, always exceeds)
        let facts = history::compact_if_needed(
            &mut messages,
            self.provider.as_ref(),
            self.compaction_provider.as_deref(),
            1, // force: any token count > 0 triggers compaction
            2,
            Some(&self.agent.language),
        )
        .await?;

        let facts_count = facts.as_ref().map(|f| f.len()).unwrap_or(0);

        if let Some(ref facts) = facts {
            self.index_facts_to_memory(facts).await;
        }

        // Replace messages in DB (atomic transaction)
        let mut tx = self.db.begin().await?;
        sqlx::query("DELETE FROM messages WHERE session_id = $1")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        for msg in &messages {
            let role = match msg.role {
                hydeclaw_types::MessageRole::User => "user",
                hydeclaw_types::MessageRole::Assistant => "assistant",
                hydeclaw_types::MessageRole::System => "system",
                hydeclaw_types::MessageRole::Tool => "tool",
            };
            let tc_json = msg
                .tool_calls
                .as_ref()
                .and_then(|tc| serde_json::to_value(tc).ok());
            sqlx::query(
                "INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id, agent_id) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(session_id)
            .bind(role)
            .bind(&msg.content)
            .bind(tc_json.as_ref())
            .bind(msg.tool_call_id.as_deref())
            .bind(if role == "assistant" { Some(&self.agent.name) } else { None::<&String> })
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        let new_count = messages.len();
        self.audit(
            crate::db::audit::event_types::COMPACTION,
            Some("api"),
            serde_json::json!({
                "session_id": session_id.to_string(),
                "facts": facts_count,
                "new_messages": new_count,
                "original_messages": rows.len(),
            }),
        );

        tracing::info!(
            session_id = %session_id, facts = facts_count,
            old = rows.len(), new = new_count, "session compacted via API"
        );

        Ok((facts_count, new_count))
    }

    /// Build tool loop config from agent TOML settings (or defaults).
    fn tool_loop_config(&self) -> super::tool_loop::ToolLoopConfig {
        self.agent
            .tool_loop
            .as_ref()
            .map(super::tool_loop::ToolLoopConfig::from)
            .unwrap_or_default()
    }

    /// Create fallback LLM provider from agent config.
    /// Returns None if fallback_provider is not configured or if provider creation fails.
    /// Looks up the connection by name in the providers table and creates a provider from it.
    async fn create_fallback_provider(&self) -> Option<Arc<dyn super::providers::LlmProvider>> {
        let fb_name = self.agent.fallback_provider.as_deref()?;
        match crate::db::providers::get_provider_by_name(&self.db, fb_name).await {
            Ok(Some(row)) => {
                let p = super::providers::create_provider_from_connection(
                    &row,
                    None,
                    self.agent.temperature,
                    self.agent.max_tokens,
                    self.secrets.clone(),
                    self.sandbox.clone(),
                    &self.agent.name,
                    &self.workspace_dir,
                    self.agent.base,
                );
                Some(p)
            }
            Ok(None) => {
                tracing::warn!(
                    agent = %self.agent.name,
                    fallback_provider = %fb_name,
                    "fallback provider not found in providers table"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    agent = %self.agent.name,
                    fallback_provider = %fb_name,
                    error = %e,
                    "failed to look up fallback provider"
                );
                None
            }
        }
    }

    /// Check daily token budget before LLM call.
    async fn check_budget(&self) -> Result<()> {
        let budget = self.agent.daily_budget_tokens;
        if budget == 0 { return Ok(()); }
        let used = crate::db::usage::get_agent_usage_today(&self.db, &self.agent.name)
            .await.unwrap_or(0) as u64;
        if used >= budget {
            anyhow::bail!("Daily token budget exceeded ({}/{} tokens). Resets at midnight.", used, budget);
        }
        Ok(())
    }

    /// Call LLM with automatic context overflow recovery.
    /// On context overflow (400), compacts messages and retries up to 3 times.
    async fn chat_with_overflow_recovery(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        let max_compact_attempts: u8 = 3;
        let mut last_error = None;

        for compact_attempt in 0..=max_compact_attempts {
            let result = self.provider.chat(messages, tools).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) if super::tool_loop::is_context_overflow(&e) && compact_attempt < max_compact_attempts => {
                    tracing::warn!(attempt = compact_attempt + 1, max = max_compact_attempts, "context overflow — compacting");
                    self.compact_messages(messages).await;
                    last_error = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("context overflow after {} compaction attempts", max_compact_attempts)))
    }

    /// Call LLM with exponential backoff retry (up to 5 attempts, 500ms–32s).
    /// Wraps chat_with_overflow_recovery to add engine-level transient retry
    /// when ALL providers (including fallbacks) returned a retryable error.
    /// RateLimit (429) uses full 60s cooldown; Retry-After header overrides both.
    async fn chat_with_transient_retry(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        let config = error_classify::RetryConfig::default();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..config.max_attempts {
            let result = self.chat_with_overflow_recovery(messages, tools).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let class = error_classify::classify(&e);
                    if !error_classify::is_retryable(&class) {
                        return Err(e);
                    }
                    let delay = error_classify::extract_retry_after(&e.to_string())
                        .unwrap_or_else(|| config.retry_delay_for_error(&class, attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = config.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error_class = ?class,
                        error = %e,
                        "retrying LLM call"
                    );
                    last_error = Some(e);
                    if attempt < config.max_attempts - 1 {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM call failed after retries")))
    }

    /// Streaming variant of chat_with_overflow_recovery.
    /// On context overflow (400), compacts messages and retries up to 3 times.
    async fn chat_stream_with_overflow_recovery(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        let max_compact_attempts: u8 = 3;
        let mut last_error = None;

        for compact_attempt in 0..=max_compact_attempts {
            let result = self.provider.chat_stream(messages, tools, chunk_tx.clone()).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) if super::tool_loop::is_context_overflow(&e) && compact_attempt < max_compact_attempts => {
                    tracing::warn!(attempt = compact_attempt + 1, max = max_compact_attempts, "context overflow — compacting (stream)");
                    self.compact_messages(messages).await;
                    last_error = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("context overflow after {} compaction attempts (stream)", max_compact_attempts)))
    }

    /// Streaming variant of chat_with_transient_retry.
    /// Uses identical exponential backoff logic; passes a fresh clone of chunk_tx on each retry.
    async fn chat_stream_with_transient_retry(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        let config = error_classify::RetryConfig::default();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..config.max_attempts {
            let result = self.chat_stream_with_overflow_recovery(messages, tools, chunk_tx.clone()).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let class = error_classify::classify(&e);
                    if !error_classify::is_retryable(&class) {
                        return Err(e);
                    }
                    let delay = error_classify::extract_retry_after(&e.to_string())
                        .unwrap_or_else(|| config.retry_delay_for_error(&class, attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = config.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error_class = ?class,
                        error = %e,
                        "retrying LLM call (stream)"
                    );
                    last_error = Some(e);
                    if attempt < config.max_attempts - 1 {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM stream call failed after retries")))
    }

    /// Variant of chat_with_transient_retry that uses an explicit provider instead of self.provider.
    /// Used for fallback provider switching without modifying engine state.
    async fn chat_with_transient_retry_using(
        &self,
        provider: &Arc<dyn super::providers::LlmProvider>,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        let config = error_classify::RetryConfig::default();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..config.max_attempts {
            let result = match provider.chat(messages, tools).await {
                Ok(resp) => Ok(resp),
                Err(e) if super::tool_loop::is_context_overflow(&e) => {
                    tracing::warn!("context overflow on fallback provider, compacting and retrying");
                    self.compact_messages(messages).await;
                    provider.chat(messages, tools).await
                }
                Err(e) => Err(e),
            };
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let class = error_classify::classify(&e);
                    if !error_classify::is_retryable(&class) {
                        return Err(e);
                    }
                    let delay = error_classify::extract_retry_after(&e.to_string())
                        .unwrap_or_else(|| config.retry_delay_for_error(&class, attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = config.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error_class = ?class,
                        error = %e,
                        "retrying LLM call (fallback provider)"
                    );
                    last_error = Some(e);
                    if attempt < config.max_attempts - 1 {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM call failed after retries (fallback provider)")))
    }

    /// Streaming variant of chat_with_transient_retry_using.
    async fn chat_stream_with_transient_retry_using(
        &self,
        provider: &Arc<dyn super::providers::LlmProvider>,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        let config = error_classify::RetryConfig::default();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..config.max_attempts {
            let result = match provider.chat_stream(messages, tools, chunk_tx.clone()).await {
                Ok(resp) => Ok(resp),
                Err(e) if super::tool_loop::is_context_overflow(&e) => {
                    tracing::warn!("context overflow on fallback provider (stream), compacting and retrying");
                    self.compact_messages(messages).await;
                    provider.chat_stream(messages, tools, chunk_tx.clone()).await
                }
                Err(e) => Err(e),
            };
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let class = error_classify::classify(&e);
                    if !error_classify::is_retryable(&class) {
                        return Err(e);
                    }
                    let delay = error_classify::extract_retry_after(&e.to_string())
                        .unwrap_or_else(|| config.retry_delay_for_error(&class, attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = config.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error_class = ?class,
                        error = %e,
                        "retrying LLM call (fallback provider, stream)"
                    );
                    last_error = Some(e);
                    if attempt < config.max_attempts - 1 {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM stream call failed after retries (fallback provider)")))
    }

    /// Default context window size based on model name.
    fn default_context_for_model(model: &str) -> usize {
        if model.contains("claude") { 200_000 }
        else if model.contains("gpt-4") { 128_000 }
        else if model.contains("MiniMax") || model.contains("M2.5") || model.contains("gemini") { 1_000_000 }
        else { 128_000 }
    }

    /// Fire-and-forget audit event recording.
    fn audit(&self, event_type: &'static str, actor: Option<&str>, details: serde_json::Value) {
        crate::db::audit::audit_spawn(
            self.db.clone(),
            self.agent.name.clone(),
            event_type,
            actor.map(|s| s.to_string()),
            details,
        );
    }

    /// Execute a tool call — routes to internal tools, MCP servers, or ToolRegistry.
    /// Returns a boxed future to allow recursive calls (subagent → execute_tool_call).
    fn execute_tool_call<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let audit_start = std::time::Instant::now();
            let result = self.execute_tool_call_inner(name, arguments).await;

            // Fire-and-forget audit record
            let duration_ms = audit_start.elapsed().as_millis() as i32;
            let is_error = result.contains("\"status\":\"error\"")
                || result.starts_with("Error:")
                || result.starts_with("Tool '") && result.contains("timed out");
            let (status, error_msg) = if is_error {
                ("error", Some(result.clone()))
            } else {
                ("ok", None)
            };

            // Extract session_id from enriched _context
            let session_id = arguments
                .get("_context")
                .and_then(|c| c.get("session_id"))
                .and_then(|s| s.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());

            // Strip _context from parameters before storing (contains internal routing data)
            let clean_params = {
                let mut p = arguments.clone();
                if let Some(obj) = p.as_object_mut() {
                    obj.remove("_context");
                }
                p
            };

            // Hook: AfterToolResult (fire-and-forget, non-blocking)
            self.hooks.fire(&super::hooks::HookEvent::AfterToolResult {
                agent: self.agent.name.clone(),
                tool_name: name.to_string(),
                duration_ms: duration_ms as u64,
            });

            let db = self.db.clone();
            let agent_name = self.agent.name.clone();
            let tool_name = name.to_string();
            let error_msg_for_quality = error_msg.clone();
            tokio::spawn(async move {
                let _ = crate::db::tool_audit::record_tool_execution(
                    &db,
                    &agent_name,
                    session_id,
                    &tool_name,
                    Some(&clean_params),
                    status,
                    Some(duration_ms),
                    error_msg.as_deref(),
                )
                .await;
            });

            // Record tool quality (non-system tools only)
            if !tool_defs_impl::all_system_tool_names().contains(&name) {
                let db2 = self.db.clone();
                let tool_name2 = name.to_string();
                let is_ok = !is_error;
                let dur = duration_ms;
                let err_msg = error_msg_for_quality;
                tokio::spawn(async move {
                    let _ = crate::db::tool_quality::record_tool_result(
                        &db2, &tool_name2, is_ok, dur, err_msg.as_deref(),
                    ).await;
                });
            }

            result
        })
    }

    /// Inner tool dispatch (separated for audit wrapping).
    fn execute_tool_call_inner<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            // 0. Approval check — if tool requires confirmation, wait for owner.
            // Skip approval for automated channels (cron, heartbeat, inter-agent).
            let context = arguments.get("_context").cloned().unwrap_or_default();
            let is_automated = context.get("_channel")
                .and_then(|v| v.as_str())
                .map(super::channel_kind::channel::is_automated)
                .unwrap_or(false);
            let has_interactive_channel = context.get("chat_id").is_some() && !is_automated;
            if self.needs_approval(name) && has_interactive_channel {
                // Skip if tool is in allowlist
                if let Ok(true) = crate::db::approvals::check_allowlist(&self.db, &self.agent.name, name).await {
                    // fall through to execution
                } else {
                let session_id = context.get("session_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok());

                // Create DB record
                let approval_id = match crate::db::approvals::create_approval(
                    &self.db,
                    &self.agent.name,
                    session_id,
                    name,
                    arguments,
                    &context,
                ).await {
                    Ok(id) => {
                        self.audit(crate::db::audit::event_types::APPROVAL_REQUESTED, None, serde_json::json!({
                            "tool": name, "approval_id": id.to_string()
                        }));
                        self.broadcast_ui_event(serde_json::json!({
                            "type": "approval_requested",
                            "approval_id": id.to_string(),
                            "agent": self.agent.name,
                            "tool_name": name,
                        }));
                        if let Some(ref ui_tx) = self.ui_event_tx {
                            let db = self.db.clone();
                            let tx = ui_tx.clone();
                            let tool_name = name.to_string();
                            let agent_name = self.agent.name.clone();
                            let approval_id_str = id.to_string();
                            tokio::spawn(async move {
                                crate::gateway::notify(
                                    &db, &tx, "tool_approval",
                                    "Tool Approval Required",
                                    &format!("Agent {} is requesting approval to use tool: {}", agent_name, tool_name),
                                    serde_json::json!({"agent": agent_name, "tool_name": tool_name, "approval_id": approval_id_str}),
                                ).await.ok();
                            });
                        }
                        id
                    }
                    Err(e) => return format!("Error creating approval: {}", e),
                };

                // Send approval request via channel (adapter formats with localization)
                let clean_args = {
                    let mut args_clone = arguments.clone();
                    if let Some(obj) = args_clone.as_object_mut() {
                        obj.remove("_context");
                    }
                    args_clone
                };

                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                let action = crate::agent::channel_actions::ChannelAction {
                    name: "approval_request".to_string(),
                    params: serde_json::json!({
                        "tool_name": name,
                        "args": clean_args,
                        "approval_id": approval_id.to_string(),
                    }),
                    context: context.clone(),
                    reply: reply_tx,
                    target_channel: None, // approval buttons go to originating channel
                };
                if let Some(ref router) = self.channel_router {
                    if let Err(e) = router.send(action).await {
                        tracing::error!(approval_id = %approval_id, error = %e, "failed to send approval_request to channel");
                    }
                    tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await.ok();
                } else {
                    tracing::warn!(tool = %name, "no channel_router — cannot send approval buttons");
                }

                // Create oneshot channel for waiting
                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                {
                    let mut waiters = self.approval_waiters.write().await;
                    // Opportunistic cleanup: remove expired entries (>5 min).
                    // Dropping the sender causes RecvError on the receiver, handled as "cancelled" below.
                    let cutoff = std::time::Instant::now() - std::time::Duration::from_secs(300);
                    waiters.retain(|_, (_, created_at)| *created_at > cutoff);
                    waiters.insert(approval_id, (result_tx, std::time::Instant::now()));
                }

                // Wait for approval with timeout
                let timeout_secs = self.agent.approval
                    .as_ref()
                    .map(|a| a.timeout_seconds)
                    .unwrap_or(300);

                match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    result_rx,
                ).await {
                    Ok(Ok(ApprovalResult::Approved)) => {
                        tracing::info!(tool = %name, approval_id = %approval_id, "tool approved");
                        // Fall through to normal execution
                    }
                    Ok(Ok(ApprovalResult::Rejected(reason))) => {
                        return format!("Tool `{}` was rejected: {}", name, reason);
                    }
                    Ok(Err(_)) => {
                        // Sender dropped — cleanup waiter
                        let mut waiters = self.approval_waiters.write().await;
                        waiters.remove(&approval_id);
                        return format!("Tool `{}` approval was cancelled.", name);
                    }
                    Err(_) => {
                        // Timeout fired — attempt to mark as timed out in DB.
                        // WHERE status='pending' ensures only one resolution wins.
                        let was_pending = crate::db::approvals::resolve_approval(
                            &self.db, approval_id, "timeout", "system",
                        ).await.unwrap_or(false);

                        let mut waiters = self.approval_waiters.write().await;
                        waiters.remove(&approval_id);

                        if !was_pending {
                            tracing::warn!(
                                tool = %name,
                                approval_id = %approval_id,
                                "approval timeout raced with resolution — timeout takes precedence"
                            );
                        }
                        return format!("Tool `{}` approval timed out after {}s.", name, timeout_secs);
                    }
                }
            } // else: allowlist
            }

            // Hook: BeforeToolCall
            if let super::hooks::HookAction::Block(reason) = self.hooks.fire(&super::hooks::HookEvent::BeforeToolCall {
                agent: self.agent.name.clone(),
                tool_name: name.to_string(),
            }) {
                return format!("Tool blocked by hook: {}", reason);
            }

            // 1. Internal tools
            if name == "workspace_write" {
                return self.handle_workspace_write(arguments).await;
            }
            if name == "workspace_read" {
                return self.handle_workspace_read(arguments).await;
            }
            if name == "workspace_list" {
                return self.handle_workspace_list(arguments).await;
            }
            if name == "workspace_edit" {
                return self.handle_workspace_edit(arguments).await;
            }
            if name == "workspace_delete" {
                return self.handle_workspace_delete(arguments).await;
            }
            if name == "workspace_rename" {
                return self.handle_workspace_rename(arguments).await;
            }
            if name == "memory" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");
                return match action {
                    "search" => self.handle_memory_search(arguments).await,
                    "index" => self.handle_memory_index(arguments).await,
                    "reindex" => self.handle_memory_reindex(arguments).await,
                    "get" => self.handle_memory_get(arguments).await,
                    "delete" => self.handle_memory_delete(arguments).await,
                    "compress" => self.handle_memory_compress(arguments).await,
                    "update" => {
                        // Remap sub_action -> action for handle_memory_update compatibility
                        let mut args = arguments.clone();
                        if let Some(sa) = arguments.get("sub_action").cloned()
                            && let Some(obj) = args.as_object_mut() {
                                obj.insert("action".to_string(), sa);
                            }
                        self.handle_memory_update(&args).await
                    }
                    _ => format!("Error: unknown memory action '{}'. Use: search, index, reindex, get, delete, update, compress.", action),
                };
            }
            if name == "message" {
                return self.handle_message_action(arguments).await;
            }
            // shell_exec removed — use code_exec(language="bash") instead
            if name == "cron" {
                return self.handle_cron(arguments).await;
            }
            if name == "subagent" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");
                return match action {
                    "spawn" => self.handle_spawn_subagent(arguments).await,
                    "status" => self.handle_subagent_status(arguments).await,
                    "logs" => self.handle_subagent_logs(arguments).await,
                    "kill" => self.handle_subagent_kill(arguments).await,
                    _ => format!("Error: unknown subagent action '{}'. Use: spawn, status, logs, kill.", action),
                };
            }
            // invite_agent removed — replaced by handoff tool (v3.0)
            if name == "handoff" {
                return self.handle_handoff(arguments).await;
            }
            if name == "web_fetch" {
                return self.handle_web_fetch(arguments).await;
            }
            if name == "graph_query" {
                return self.handle_graph_query(arguments).await;
            }
            if name == "tool_create" {
                return self.handle_tool_create(arguments).await;
            }
            if name == "tool_list" {
                return self.handle_tool_list(arguments).await;
            }
            if name == "tool_test" {
                return self.handle_tool_test(arguments).await;
            }
            if name == "tool_verify" {
                return self.handle_tool_verify(arguments).await;
            }
            if name == "tool_disable" {
                return self.handle_tool_disable(arguments).await;
            }
            if name == "skill" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");
                return match action {
                    "create" => self.handle_skill_create(arguments).await,
                    "update" => self.handle_skill_update(arguments).await,
                    "list" => self.handle_skill_list(arguments).await,
                    _ => format!("Error: unknown skill action '{}'. Use: create, update, list.", action),
                };
            }
            if name == "skill_use" {
                return self.handle_skill_use(arguments).await;
            }
            if name == "tool_discover" {
                return self.handle_tool_discover(arguments).await;
            }
            if name == "secret_set" {
                return self.handle_secret_set(arguments).await;
            }
            if name == "session" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");
                return match action {
                    "list" => self.handle_sessions_list(arguments).await,
                    "history" => self.handle_sessions_history(arguments).await,
                    "search" => self.handle_session_search(arguments).await,
                    "context" => self.handle_session_context(arguments).await,
                    "send" => self.handle_session_send(arguments).await,
                    "export" => self.handle_session_export(arguments).await,
                    _ => format!("Error: unknown session action '{}'. Use: list, history, search, context, send, export.", action),
                };
            }
            if name == "agents_list" {
                return self.handle_agents_list(arguments).await;
            }
            if name == "browser_action" {
                return self.handle_browser_action(arguments).await;
            }
            // service_manage and service_exec removed — base agent uses code_exec on host
            if name == "code_exec" {
                return self.handle_code_exec(arguments).await;
            }
            if name == "git" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");

                // Clone is special — doesn't need existing git dir
                if action == "clone" {
                    let url = match arguments.get("url").and_then(|v| v.as_str()).filter(|u| !u.is_empty()) {
                        Some(u) => u.to_string(),
                        None => return "Error: url parameter required.".to_string(),
                    };
                    let url = if url.starts_with("https://github.com/") {
                        url.replace("https://github.com/", "git@github.com:")
                    } else { url };
                    let dir_name = arguments.get("directory").and_then(|v| v.as_str()).filter(|d| !d.is_empty())
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| {
                            url.rsplit('/').next().or_else(|| url.rsplit(':').next())
                                .unwrap_or("repo").trim_end_matches(".git").to_string()
                        });
                    let target = std::path::PathBuf::from(&self.workspace_dir).join(&dir_name);
                    if target.exists() {
                        return format!("Error: directory '{}' already exists in workspace.", dir_name);
                    }
                    let output = tokio::process::Command::new("git")
                        .args(["clone", &url, &target.to_string_lossy()])
                        .output().await;
                    return match output {
                        Ok(o) => {
                            let stdout = String::from_utf8_lossy(&o.stdout);
                            let stderr = String::from_utf8_lossy(&o.stderr);
                            if o.status.success() { format!("Cloned {} into {}\n{}{}", url, dir_name, stdout, stderr) }
                            else { format!("git clone failed:\n{}{}", stdout, stderr) }
                        }
                        Err(e) => format!("Error running git clone: {}", e),
                    };
                }

                // All other actions need a git working directory
                let git_dir = match arguments.get("directory").and_then(|v| v.as_str()).filter(|d| !d.is_empty()) {
                    Some(sub) => {
                        let p = std::path::PathBuf::from(&self.workspace_dir).join(sub);
                        if !p.exists() || !p.is_dir() { return format!("Error: directory '{}' not found in workspace.", sub); }
                        p.to_string_lossy().to_string()
                    }
                    None => {
                        let ws = std::path::PathBuf::from(&self.workspace_dir);
                        if !ws.join(".git").exists() {
                            let mut git_dirs = Vec::new();
                            if let Ok(mut entries) = tokio::fs::read_dir(&ws).await {
                                while let Ok(Some(entry)) = entries.next_entry().await {
                                    let p = entry.path();
                                    if p.is_dir() && p.join(".git").exists()
                                        && let Some(dn) = p.file_name().and_then(|n| n.to_str()) { git_dirs.push(dn.to_string()); }
                                }
                            }
                            if !git_dirs.is_empty() {
                                return format!("Error: workspace root is not a git repo. Use directory parameter. Found: {}", git_dirs.join(", "));
                            }
                            return "Error: no git repository found in workspace.".to_string();
                        }
                        ws.to_string_lossy().to_string()
                    }
                };

                return match action {
                    "commit" => {
                        let message = arguments.get("message").and_then(|v| v.as_str()).unwrap_or("chore: update files");
                        match tokio::process::Command::new("git").args(["commit", "-am", message]).current_dir(&git_dir).output().await {
                            Ok(o) => { let s = String::from_utf8_lossy(&o.stdout); let e = String::from_utf8_lossy(&o.stderr);
                                if o.status.success() { s.to_string() } else { format!("git commit failed: {}{}", s, e) } }
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "log" => {
                        let limit = arguments.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
                        let oneline = arguments.get("oneline").and_then(|v| v.as_bool()).unwrap_or(true);
                        let mut args = vec!["log".to_string(), format!("-{}", limit)];
                        if oneline { args.push("--oneline".to_string()); }
                        else { args.push("--format=%h %ad %an: %s".to_string()); args.push("--date=short".to_string()); }
                        match tokio::process::Command::new("git").args(&args).current_dir(&git_dir).output().await {
                            Ok(o) => { let out = String::from_utf8_lossy(&o.stdout).to_string();
                                if out.is_empty() { "No commits found.".to_string() } else { out } }
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "add" => {
                        let files: Vec<String> = arguments.get("files").and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|f| f.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                        if files.is_empty() { return "Error: files parameter required.".to_string(); }
                        let mut args = vec!["add".to_string()]; args.extend(files);
                        match tokio::process::Command::new("git").args(&args).current_dir(&git_dir).output().await {
                            Ok(o) => if o.status.success() { let s = String::from_utf8_lossy(&o.stdout);
                                if s.is_empty() { "Files staged.".to_string() } else { s.to_string() } }
                                else { format!("git add failed: {}", String::from_utf8_lossy(&o.stderr)) }
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "branch" => {
                        let branch_act = arguments.get("branch_action").and_then(|v| v.as_str()).unwrap_or("list");
                        let branch_name = arguments.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let args: Vec<&str> = match branch_act {
                            "list" => vec!["branch", "-a"],
                            "create" => { if branch_name.is_empty() { return "Error: name required.".to_string(); } vec!["checkout", "-b", branch_name] }
                            "switch" => { if branch_name.is_empty() { return "Error: name required.".to_string(); } vec!["checkout", branch_name] }
                            "delete" => { if branch_name.is_empty() { return "Error: name required.".to_string(); } vec!["branch", "-d", branch_name] }
                            _ => return format!("Error: unknown branch_action '{}'.", branch_act),
                        };
                        match tokio::process::Command::new("git").args(&args).current_dir(&git_dir).output().await {
                            Ok(o) => { let mut out = String::from_utf8_lossy(&o.stdout).to_string();
                                let stderr = String::from_utf8_lossy(&o.stderr); if !stderr.is_empty() { out.push_str(&stderr); }
                                if out.is_empty() { format!("Exit code: {}", o.status.code().unwrap_or(-1)) } else { out } }
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "status" | "diff" | "push" | "pull" => {
                        match tokio::process::Command::new("git").args([action]).current_dir(&git_dir).output().await {
                            Ok(o) => { let mut out = String::from_utf8_lossy(&o.stdout).to_string();
                                let stderr = String::from_utf8_lossy(&o.stderr);
                                if !stderr.is_empty() { out.push_str("\n--- stderr ---\n"); out.push_str(&stderr); }
                                if out.is_empty() { format!("Exit code: {}", o.status.code().unwrap_or(-1)) } else { out } }
                            Err(e) => format!("Error running git {}: {}", action, e),
                        }
                    }
                    _ => format!("Error: unknown git action '{}'. Use: status, diff, log, commit, add, push, pull, branch, clone.", action),
                };
            }
            if name == "canvas" {
                return self.handle_canvas(arguments).await;
            }
            if name == "rich_card" {
                return self.handle_rich_card(arguments);
            }
            if name == "process" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");
                return match action {
                    "start" => self.handle_process_start(arguments).await,
                    "status" => self.handle_process_status(arguments).await,
                    "logs" => self.handle_process_logs(arguments).await,
                    "kill" => self.handle_process_kill(arguments).await,
                    _ => format!("Error: unknown process action '{}'. Use: start, status, logs, kill.", action),
                };
            }
            // 2. YAML-defined tools (workspace/tools/) — only VERIFIED may be called directly.
            // Draft tools are blocked here; they can only be invoked through tool_test.
            if let Some(yaml_tool) = crate::tools::yaml_tools::find_yaml_tool(
                &self.workspace_dir,
                name,
            ).await {
                if yaml_tool.status == crate::tools::yaml_tools::ToolStatus::Draft {
                    return format!(
                        "Tool '{}' is in DRAFT status and cannot be called directly. \
                        Use tool_test(tool_name=\"{}\", test_params={{...}}) to test it, \
                        then tool_verify(tool_name=\"{}\") to promote it to verified.",
                        name, name, name
                    );
                }
                if yaml_tool.required_base && !self.agent.base {
                    return format!("Tool '{}' requires base agent.", name);
                }
                // GitHub repo access enforcement: tools starting with "github_" require allowed repo
                if name.starts_with("github_") {
                    let owner = arguments.get("owner").and_then(|v| v.as_str()).unwrap_or("");
                    let repo_name = arguments.get("repo").and_then(|v| v.as_str()).unwrap_or("");
                    if owner.is_empty() || repo_name.is_empty() {
                        return "GitHub tools require 'owner' and 'repo' parameters.".to_string();
                    }
                    match crate::db::github::check_repo_access(&self.db, &self.agent.name, owner, repo_name).await {
                        Ok(true) => {} // allowed
                        Ok(false) => {
                            return format!(
                                "Repository {}/{} is not in the allowed list for agent '{}'. \
                                Add it via POST /api/agents/{}/github/repos",
                                owner, repo_name, self.agent.name, self.agent.name
                            );
                        }
                        Err(e) => {
                            return format!("Error checking repo access: {}", e);
                        }
                    }
                }
                if let Some(ref ca) = yaml_tool.channel_action.clone() {
                    return self.execute_yaml_channel_action(&yaml_tool, arguments, ca).await;
                }
                if CACHEABLE_SEARCH_TOOLS.contains(&name)
                    && let Some(q) = arguments.get("query").and_then(|v| v.as_str())
                    && let Some(cached) = self.check_search_cache(q).await
                {
                    return cached;
                }
                let resolver = self.make_resolver();
                let oauth_ctx = self.make_oauth_context();
                // Internal endpoints (toolgate, searxng, browser-renderer) bypass SSRF filtering
                let client = if crate::tools::ssrf::is_internal_endpoint(&yaml_tool.endpoint) {
                    &self.http_client
                } else {
                    &self.ssrf_http_client
                };
                return match yaml_tool.execute_oauth(arguments, client, Some(&resolver), oauth_ctx.as_ref()).await {
                    Ok(result) => {
                        if CACHEABLE_SEARCH_TOOLS.contains(&name)
                            && let Some(q) = arguments.get("query").and_then(|v| v.as_str())
                        {
                            self.store_search_cache(q, &result).await;
                        }
                        result
                    },
                    Err(e) => Self::format_tool_error(name, &e.to_string()),
                };
            }

            // 3. MCP tools (via MCP)
            if let Some(ref mcp) = self.mcp
                && let Some(mcp_name) = mcp.find_mcp_for_tool(name).await {
                    return match mcp.call_tool(&mcp_name, name, arguments).await {
                        Ok(result) => result,
                        Err(e) => Self::format_tool_error(name, &e.to_string()),
                    };
                }

            // 5. External tools via ToolRegistry (fallback)
            match self.tools.call(name, arguments).await {
                Ok(result) => serde_json::to_string(&result).unwrap_or_default(),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("tool not found") {
                        tracing::warn!(tool = %name, "LLM called non-existent tool");
                        format!("Error: tool '{}' does not exist. Use tool_list to see available tools.", name)
                    } else {
                        Self::format_tool_error(name, &msg)
                    }
                }
            }
        })
    }


    /// Record LLM token usage to the database (fire-and-forget).
    fn record_usage(&self, response: &hydeclaw_types::LlmResponse, session_id: Option<uuid::Uuid>) {
        if let Some(ref usage) = response.usage {
            let db = self.db.clone();
            let agent = self.agent.name.clone();
            let provider = response.provider.clone()
                .unwrap_or_else(|| self.provider.name().to_string());
            let model = response.model.clone().unwrap_or_default();
            let input = usage.input_tokens;
            let output = usage.output_tokens;
            tokio::spawn(async move {
                if let Err(e) = crate::db::usage::record_usage(
                    &db, &agent, &provider, &model, input, output, session_id,
                ).await {
                    tracing::debug!(error = %e, "failed to record usage");
                }
            });
        }
    }

    /// Filter tools based on per-agent allow/deny policy.
    /// Merge a cron-job tool policy override on top of the agent's base policy,
    /// then re-filter the already-filtered tool list.
    ///
    /// Logic:
    ///  - deny list is unioned (base deny ∪ override deny)
    ///  - allow list: if override has non-empty allow, restrict to those tools only (intersection with current list)
    fn apply_tool_policy_override(
        &self,
        tools: Vec<ToolDefinition>,
        override_policy: &crate::config::AgentToolPolicy,
    ) -> Vec<ToolDefinition> {
        let base_deny = self.agent.tools.as_ref().map(|p| &p.deny);

        tools.into_iter().filter(|t| {
            // Union of deny lists
            if override_policy.deny.iter().any(|d| d == &t.name) {
                return false;
            }
            if let Some(bd) = base_deny
                && bd.iter().any(|d| d == &t.name) {
                    return false;
                }
            // If override has a non-empty allow list, restrict to those tools only
            if !override_policy.allow.is_empty() {
                return override_policy.allow.iter().any(|a| a == &t.name);
            }
            true
        }).collect()
    }

    fn filter_tools_by_policy(&self, tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        let policy = match &self.agent.tools {
            Some(p) => p,
            None => return tools,
        };

        let before = tools.len();
        let filtered: Vec<ToolDefinition> = tools
            .into_iter()
            .filter(|t| {
                let name = t.name.as_str();

                // Check deny list first (applies to ALL tools including core)
                if policy.deny.iter().any(|d| d == name) {
                    return false;
                }

                // Core internal tools (workspace, memory, system) always allowed unless denied above
                if matches!(
                    name,
                    "workspace_write" | "workspace_read" | "workspace_list" | "workspace_edit" | "workspace_delete" | "workspace_rename" |
                    "web_fetch" | "subagent" | "handoff" |
                    "message" | "cron" | "code_exec" | "browser_action" |
                    "git" | "session" | "skill" | "skill_use" |
                    "canvas" | "rich_card" | "agents_list" | "secret_set" |
                    "process" | "graph_query"
                ) {
                    return true;
                }

                // Memory tool requires memory_store to be available
                if name == "memory" {
                    return self.memory_store.is_available();
                }

                // Tool management tools
                if name.starts_with("tool_") {
                    return true;
                }
                // allow_all = everything not denied
                if policy.allow_all {
                    return true;
                }
                // deny_all_others = only explicitly allowed
                if policy.deny_all_others {
                    return policy.allow.iter().any(|a| a == &t.name);
                }
                // Non-empty allow list = only those
                if !policy.allow.is_empty() {
                    return policy.allow.iter().any(|a| a == &t.name);
                }
                true
            })
            .collect();

        if filtered.len() != before {
            tracing::info!(
                agent = %self.agent.name,
                before,
                after = filtered.len(),
                "tool policy applied"
            );
        }
        filtered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_guard_outcome_defaults_to_running() {
        let outcome = SessionOutcome::Running;
        assert!(matches!(outcome, SessionOutcome::Running));
        let done = SessionOutcome::Done;
        assert!(matches!(done, SessionOutcome::Done));
        let failed = SessionOutcome::Failed("test".to_string());
        assert!(matches!(failed, SessionOutcome::Failed(_)));
    }

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

