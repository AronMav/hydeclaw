//! Session entry, user-message persist, ProcessingGuard, slash-command detection.
//!
//! See docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md §3, §5.

use crate::agent::engine::stream::{ProcessingGuard, ProcessingPhase};
use crate::agent::pipeline::sink::{EventSink, PipelineEvent};
use crate::agent::session_manager::{SessionLifecycleGuard, SessionManager};
use crate::agent::tool_loop::LoopDetector;
use hydeclaw_types::{IncomingMessage, Message, MessageRole};
use uuid::Uuid;

// ── Public types ──────────────────────────────────────────────────────────────

/// Outcome of the bootstrap phase — passed directly to the execute phase.
///
/// `lifecycle_guard` is wrapped in `Option` so the adapter can `.take()` it
/// before forwarding `BootstrapOutcome` to `execute()` (avoids partial-move).
// Tasks 7-9 consume this struct; allow dead_code until those are wired up.
#[allow(dead_code)]
pub struct BootstrapOutcome {
    pub session_id: Uuid,
    /// Raw user text after PII redaction / URL enrichment (TODO: Task 10 inlines enrichment).
    pub enriched_text: String,
    pub messages: Vec<Message>,
    pub tools: Vec<hydeclaw_types::ToolDefinition>,
    pub loop_detector: LoopDetector,
    pub processing_guard: ProcessingGuard,
    /// Option so the adapter can take() it before passing BootstrapOutcome to execute().
    pub lifecycle_guard: Option<SessionLifecycleGuard>,
    /// Non-None when the user message was a slash-command that was already handled.
    pub command_output: Option<String>,
    /// ID of the user message just persisted; used by finalize as parent for the assistant reply.
    pub user_message_id: Uuid,
}

/// Input context for the bootstrap phase.
// Tasks 7-9 construct this; allow dead_code until those are wired up.
#[allow(dead_code)]
pub struct BootstrapContext<'a> {
    pub msg: &'a IncomingMessage,
    pub resume_session_id: Option<Uuid>,
    pub force_new_session: bool,
    pub use_history: bool,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Log a WAL "running" event with a single retry on failure.
pub(crate) async fn log_wal_running_with_retry(sm: &SessionManager, session_id: Uuid) {
    if let Err(e) = sm.log_wal_event(session_id, "running", None).await {
        tracing::warn!(session_id = %session_id, error = %e, "failed to log WAL running event, retrying");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Err(e2) = sm.log_wal_event(session_id, "running", None).await {
            tracing::error!(session_id = %session_id, error = %e2, "WAL running event retry also failed");
        }
    }
}

/// Extract the sender agent ID from an inter-agent message.
///
/// Returns `Some("AgentName")` when `user_id` starts with `"agent:"`, `None` otherwise.
fn extract_sender_agent_id(user_id: &str) -> Option<&str> {
    if user_id.starts_with("agent:") {
        Some(user_id.trim_start_matches("agent:"))
    } else {
        None
    }
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Bootstrap a session: build context, mark running, emit first Phase event,
/// persist the user message, arm the loop detector, and detect slash-commands.
///
/// Callers (Tasks 7-9) call this at the top of their handler, then pass
/// `BootstrapOutcome` to the execute phase.
// Tasks 7-9 call this; allow dead_code until those are wired up.
#[allow(dead_code)]
pub async fn bootstrap<S: EventSink>(
    engine: &crate::agent::engine::AgentEngine,
    ctx: BootstrapContext<'_>,
    sink: &mut S,
) -> anyhow::Result<BootstrapOutcome> {
    // 1. Build context (session_id + message history + tool definitions)
    let crate::agent::context_builder::ContextSnapshot {
        session_id,
        mut messages,
        tools,
    } = engine
        .build_context(
            ctx.msg,
            ctx.use_history,
            ctx.resume_session_id,
            ctx.force_new_session,
        )
        .await?;

    // 2. Mark session as running in DB + WAL
    let sm = SessionManager::new(engine.cfg().db.clone());
    if let Err(e) = sm.set_run_status(session_id, "running").await {
        tracing::warn!(
            session_id = %session_id,
            error = %e,
            "set_run_status(running) failed"
        );
    }
    log_wal_running_with_retry(&sm, session_id).await;

    // 3. Emit first Phase event (silently dropped by SseSink; routed by ChannelStatusSink)
    let _ = sink.emit(PipelineEvent::Phase(ProcessingPhase::Thinking)).await;

    // 4. Lifecycle guard (kept in Option so the adapter can .take() it for finalize)
    let lifecycle_guard = Some(SessionLifecycleGuard::new(engine.cfg().db.clone(), session_id));

    // 5. ProcessingGuard — broadcasts "typing" via ui_event_tx (independent of sink)
    let start_event = serde_json::json!({
        "type": "agent_processing",
        "agent": engine.cfg().agent.name,
        "session_id": session_id.to_string(),
    });
    let processing_guard = ProcessingGuard::new(
        engine.state().ui_event_tx.clone(),
        engine.state().processing_tracker.clone(),
        engine.cfg().agent.name.clone(),
        &start_event,
    );

    // 6. Enrich + persist user message
    //    Full enrichment (URL fetch, attachment descriptions) requires HTTP clients
    //    injected from the caller — delegated to Task 10 which inlines the body.
    //    For now we use the raw text (PII redaction lives inside enrich_message_text).
    let user_text = ctx.msg.text.clone().unwrap_or_default();
    let enriched_text = user_text.clone();

    let sender_agent_id = extract_sender_agent_id(&ctx.msg.user_id);
    // parent_message_id = leaf_message_id: threads the new user message onto
    // the active conversation path so reload-from-active-path can find it.
    // user_message_id is then used as parent for the assistant reply in finalize.
    // Regression fixed 2026-04-20 (pipeline unification had dropped both).
    let user_message_id = sm
        .save_message_ex(
            session_id,
            "user",
            &enriched_text,
            None,
            None,
            sender_agent_id,
            None,
            ctx.msg.leaf_message_id,
        )
        .await?;

    // 7. LoopDetector — resets on each session entry (spec §7)
    let loop_config = engine.tool_loop_config();
    let loop_detector = LoopDetector::new(&loop_config);

    // 8. Slash-command detection (spec §11.1 — future extension point for richer outputs)
    let command_output = match engine.handle_command(&user_text, ctx.msg).await {
        Some(result) => Some(result?),
        None => None,
    };

    // 9. Push user message into message history for the LLM
    messages.push(Message {
        role: MessageRole::User,
        content: user_text,
        tool_calls: None,
        tool_call_id: None,
        thinking_blocks: vec![],
    });

    Ok(BootstrapOutcome {
        session_id,
        enriched_text,
        messages,
        tools,
        loop_detector,
        processing_guard,
        lifecycle_guard,
        command_output,
        user_message_id,
    })
}
