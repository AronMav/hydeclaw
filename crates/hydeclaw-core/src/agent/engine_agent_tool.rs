//! Session-scoped agent tool handler — run/message/status/kill live agents.
//! Included in engine.rs via `#[path = "engine_agent_tool.rs"] mod agent_tool_impl;`.

use super::*;
use crate::agent::session_agent_pool::{self, SessionAgentPool};

/// Extract session_id from enriched `_context` (per-invocation, race-free) with
/// fallback to the shared `processing_session_id` (for host agent SSE path).
fn extract_session_id(args: &serde_json::Value) -> Option<uuid::Uuid> {
    args.get("_context")
        .and_then(|ctx| ctx.get("session_id"))
        .and_then(|s| s.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
}

impl AgentEngine {
    /// Dispatch `agent` tool calls to the appropriate sub-handler based on `action`.
    ///
    /// Actions: `run`, `message`, `status`, `kill`.
    pub(super) async fn handle_agent_tool(&self, args: &serde_json::Value) -> String {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match action {
            "run" => self.handle_agent_run(args).await,
            "message" => self.handle_agent_message(args).await,
            "status" => self.handle_agent_status(args).await,
            "kill" => self.handle_agent_kill(args).await,
            other => format!("Error: unknown agent action '{}'. Expected: run, message, status, kill", other),
        }
    }

    /// `run` — spawn a new live agent in the current session's pool.
    async fn handle_agent_run(&self, args: &serde_json::Value) -> String {
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => return "Error: 'target' parameter is required".to_string(),
        };
        let task = match args.get("task").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return "Error: 'task' parameter is required".to_string(),
        };

        // Resolve session ID from enriched _context (injected by enrich_tool_args, race-free).
        // No fallback to processing_session_id — that shared mutex causes deadlocks and races.
        let session_id = match extract_session_id(args) {
            Some(id) if id != uuid::Uuid::nil() => id,
            _ => return "Error: no active session — agent tool requires session context via _context".to_string(),
        };

        // Resolve target engine from the agent map.
        let agent_map = match &self.agent_map {
            Some(m) => m,
            None => return "Error: agent_map not available (subagent context)".to_string(),
        };
        let target_engine = {
            let map = agent_map.read().await;
            match map.get(target) {
                Some(handle) => handle.engine.clone(),
                None => return format!("Error: agent '{}' not found", target),
            }
        };

        // Register the target agent as a session participant so multi-agent instructions are injected.
        let _ = crate::db::sessions::add_participant(self.db_pool(), session_id, target).await;

        // Get session pools.
        let pools = match &self.session_pools {
            Some(p) => p,
            None => return "Error: session_pools not available".to_string(),
        };

        // Check for duplicate and insert — all under one write lock to prevent TOCTOU race.
        let mut pools_write = pools.write().await;
        let pool = pools_write
            .entry(session_id)
            .or_insert_with(|| SessionAgentPool::new(session_id));
        if pool.contains(target) {
            return format!("Error: {} is already running in this session. Use agent(action: \"message\") to communicate.", target);
        }

        // Spawn the live agent.
        let live_agent = match session_agent_pool::spawn_live_agent(
            target.to_string(), target_engine, task.to_string(), session_id,
        ) {
            Some(la) => la,
            None => return format!("Error: failed to deliver initial task to agent '{}'", target),
        };

        pool.insert(live_agent);

        serde_json::json!({
            "status": "ok",
            "agent": target,
            "session_id": session_id.to_string(),
            "message": format!("Agent '{}' started with initial task", target),
        })
        .to_string()
    }

    /// `message` — send a follow-up message to an already-running live agent.
    async fn handle_agent_message(&self, args: &serde_json::Value) -> String {
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => return "Error: 'target' parameter is required".to_string(),
        };
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return "Error: 'text' parameter is required".to_string(),
        };

        let session_id = match extract_session_id(args) {
            Some(id) if id != uuid::Uuid::nil() => id,
            _ => match self.processing_session_id().lock().await.as_ref().copied() {
                Some(id) => id,
                None => return "Error: no active session — agent tool requires a session context".to_string(),
            },
        };

        let pools = match &self.session_pools {
            Some(p) => p,
            None => return "Error: session_pools not available".to_string(),
        };

        let pools_read = pools.read().await;
        let pool = match pools_read.get(&session_id) {
            Some(p) => p,
            None => return format!("Error: no agent pool for session {}", session_id),
        };
        let agent = match pool.get(target) {
            Some(a) => a,
            None => return format!("Error: agent '{}' not found in session pool", target),
        };

        match agent.message_tx.try_send(session_agent_pool::AgentMessage {
            text: text.to_string(),
        }) {
            Ok(()) => serde_json::json!({
                "status": "ok",
                "agent": target,
                "message": "Message sent",
            })
            .to_string(),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                format!("Error: agent '{}' message queue is full — it may still be processing", target)
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                format!("Error: agent '{}' processing loop has exited", target)
            }
        }
    }

    /// `status` — return status of a single agent (if `agent` given) or all agents in the pool.
    async fn handle_agent_status(&self, args: &serde_json::Value) -> String {
        let session_id = match extract_session_id(args) {
            Some(id) if id != uuid::Uuid::nil() => id,
            _ => match self.processing_session_id().lock().await.as_ref().copied() {
                Some(id) => id,
                None => return "Error: no active session — agent tool requires a session context".to_string(),
            },
        };

        let pools = match &self.session_pools {
            Some(p) => p,
            None => return "Error: session_pools not available".to_string(),
        };

        let pools_read = pools.read().await;
        let pool = match pools_read.get(&session_id) {
            Some(p) => p,
            None => {
                return serde_json::json!({ "agents": [] }).to_string();
            }
        };

        // Single agent query — use "target" (same as other actions for consistency).
        if let Some(target) = args.get("target").and_then(|v| v.as_str()) {
            if let Some(agent) = pool.get(target) {
                let last_result_arc = agent.last_result.clone();
                let status_str = if agent.is_processing() { "processing" } else { "idle" };
                let iterations = agent.iterations();
                let elapsed = agent.elapsed().as_secs_f64();
                // Drop pools_read before awaiting last_result lock.
                drop(pools_read);
                let last_result = last_result_arc.read().await.clone();
                return serde_json::json!({
                    "agent": target,
                    "status": status_str,
                    "iterations": iterations,
                    "elapsed_secs": elapsed,
                    "last_result": last_result,
                })
                .to_string();
            } else {
                return format!("Error: agent '{}' not found in session pool", target);
            }
        }

        // List all agents.
        let entries = pool.list();
        serde_json::json!({ "agents": entries }).to_string()
    }

    /// `kill` — remove (and drop) a live agent from the session pool.
    async fn handle_agent_kill(&self, args: &serde_json::Value) -> String {
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => return "Error: 'target' parameter is required".to_string(),
        };

        let session_id = match extract_session_id(args) {
            Some(id) if id != uuid::Uuid::nil() => id,
            _ => match self.processing_session_id().lock().await.as_ref().copied() {
                Some(id) => id,
                None => return "Error: no active session — agent tool requires a session context".to_string(),
            },
        };

        let pools = match &self.session_pools {
            Some(p) => p,
            None => return "Error: session_pools not available".to_string(),
        };

        let mut pools_write = pools.write().await;
        let pool = match pools_write.get_mut(&session_id) {
            Some(p) => p,
            None => return format!("Error: no agent pool for session {}", session_id),
        };

        match pool.remove(target) {
            Some(_dropped) => {
                // Drop handles cleanup (cancel + abort).
                serde_json::json!({
                    "status": "ok",
                    "agent": target,
                    "message": format!("Agent '{}' killed", target),
                })
                .to_string()
            }
            None => format!("Error: agent '{}' not found in session pool", target),
        }
    }
}
