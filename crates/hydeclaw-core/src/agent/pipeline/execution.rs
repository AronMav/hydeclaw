//! Pipeline step: execution — shared helpers for the main LLM tool loop.
//!
//! These free functions are used by both `handle_with_status` (engine_execution.rs)
//! and `handle_sse` (engine_sse.rs) to avoid duplicating lifecycle, notification,
//! and post-processing logic.

use std::sync::Arc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::agent::session_manager::SessionManager;

// ── WAL lifecycle ──────────────────────────────────────────

/// Log a WAL "running" event with a single retry on failure.
///
/// WAL consistency is important for crash recovery, so we retry once after
/// a short delay if the first attempt fails.
pub async fn log_wal_running_with_retry(sm: &SessionManager, session_id: Uuid) {
    if let Err(e) = sm.log_wal_event(session_id, "running", None).await {
        tracing::warn!(session_id = %session_id, error = %e, "failed to log WAL running event, retrying");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Err(e2) = sm.log_wal_event(session_id, "running", None).await {
            tracing::error!(session_id = %session_id, error = %e2, "WAL running event retry also failed");
        }
    }
}

// ── Post-session ───────────────────────────────────────────

/// Spawn background knowledge extraction if the session has enough messages.
///
/// Non-blocking: fires and forgets a tokio task. Only runs when the
/// conversation has >= `min_messages` entries (default threshold: 5).
pub fn spawn_knowledge_extraction(
    db: PgPool,
    session_id: Uuid,
    agent_name: String,
    provider: Arc<dyn crate::agent::providers::LlmProvider>,
    memory: Arc<dyn crate::agent::memory_service::MemoryService>,
    message_count: usize,
) {
    if message_count >= 5 {
        tokio::spawn(async move {
            crate::agent::knowledge_extractor::extract_and_save(
                db, session_id, agent_name, provider, memory,
            )
            .await;
        });
    }
}

// ── Message helpers ────────────────────────────────────────

/// Extract the sender agent ID from an inter-agent message.
///
/// Returns `Some("AgentName")` when `user_id` starts with `"agent:"`,
/// `None` otherwise.
pub fn extract_sender_agent_id(user_id: &str) -> Option<&str> {
    if user_id.starts_with("agent:") {
        Some(user_id.trim_start_matches("agent:"))
    } else {
        None
    }
}

// ── Loop / limit notifications ─────────────────────────────

/// Build the system nudge message injected when a tool-call loop is detected.
pub fn build_loop_nudge_message(reason: Option<&str>) -> String {
    let nudge_desc = reason.unwrap_or("repeating pattern");
    format!(
        "LOOP DETECTED: You have repeated the same sequence of actions ({desc}). \
         Change your approach entirely. If the task is too large for a single session, \
         tell the user and suggest breaking it into smaller steps. Do NOT retry the same approach.",
        desc = nudge_desc
    )
}

/// Spawn a notification when the agent hits its iteration limit.
pub fn notify_iteration_limit(
    db: PgPool,
    ui_event_tx: Option<&tokio::sync::broadcast::Sender<String>>,
    agent_name: &str,
    max_iterations: usize,
) {
    tracing::warn!(
        agent = %agent_name,
        max_iterations,
        "agent reached iteration limit"
    );
    if let Some(ui_tx) = ui_event_tx {
        let db = db.clone();
        let tx = ui_tx.clone();
        let agent_name = agent_name.to_string();
        tokio::spawn(async move {
            crate::gateway::notify(
                &db,
                &tx,
                "iteration_limit",
                &format!("Iteration limit: {}", agent_name),
                &format!(
                    "Agent {} reached its iteration limit ({} iterations). The task may be incomplete.",
                    agent_name, max_iterations
                ),
                serde_json::json!({"agent": agent_name, "max_iterations": max_iterations}),
            )
            .await
            .ok();
        });
    }
}

/// Spawn a notification when the agent is stopped due to a detected loop.
pub fn notify_loop_detected(
    db: PgPool,
    ui_event_tx: Option<&tokio::sync::broadcast::Sender<String>>,
    agent_name: &str,
    session_id: Uuid,
) {
    if let Some(ui_tx) = ui_event_tx {
        let db = db.clone();
        let tx = ui_tx.clone();
        let agent_name = agent_name.to_string();
        tokio::spawn(async move {
            crate::gateway::notify(
                &db,
                &tx,
                "agent_loop_detected",
                &format!("Agent stuck in loop: {}", agent_name),
                &format!(
                    "Agent {} was stopped after detecting a repeating pattern. Session: {}",
                    agent_name, session_id
                ),
                serde_json::json!({"agent": agent_name, "session_id": session_id.to_string()}),
            )
            .await
            .ok();
        });
    }
}

/// Spawn a notification when auto-continue nudges the LLM.
pub fn notify_auto_continue(
    db: PgPool,
    ui_event_tx: Option<&tokio::sync::broadcast::Sender<String>>,
    agent_name: &str,
    count: u8,
    max: u8,
) {
    if let Some(ui_tx) = ui_event_tx {
        let db = db.clone();
        let tx = ui_tx.clone();
        let agent_name = agent_name.to_string();
        tokio::spawn(async move {
            crate::gateway::notify(
                &db,
                &tx,
                "auto_continue",
                &format!("Auto-continue: {}", agent_name),
                &format!(
                    "Agent continued unfinished task (attempt {}/{})",
                    count, max
                ),
                serde_json::json!({"agent": agent_name}),
            )
            .await
            .ok();
        });
    }
}

/// Spawn a notification when an agent run fails (LLM error).
pub fn notify_agent_error(
    db: PgPool,
    ui_event_tx: Option<&tokio::sync::broadcast::Sender<String>>,
    agent_name: &str,
    reason: &str,
) {
    if let Some(ui_tx) = ui_event_tx {
        let db = db.clone();
        let tx = ui_tx.clone();
        let agent_name = agent_name.to_string();
        let reason = reason.to_string();
        tokio::spawn(async move {
            crate::gateway::notify(
                &db,
                &tx,
                "agent_error",
                "Agent Error",
                &format!("Agent {} run failed: {}", agent_name, reason),
                serde_json::json!({"agent": agent_name, "reason": reason}),
            )
            .await
            .ok();
        });
    }
}
