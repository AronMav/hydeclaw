//! Handoff tool handler -- spawn target agent as async subagent and return immediately.

use super::*;
use crate::agent::subagent_state;

// ── handoff handler ─────────────────────────────────────────────────────────

impl AgentEngine {
    /// Internal tool: hand off a task to another agent via async subagent delegation.
    /// Spawns the target agent as an isolated subagent and returns immediately.
    /// The turn loop in chat.rs drains `pending_handoffs` after handle_sse and
    /// injects completed results as user messages.
    pub(super) async fn handle_handoff(&self, args: &serde_json::Value) -> String {
        // 1. Validate required parameters (agent, task, context all required)
        let target = match args.get("agent").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => return "Error: 'agent' parameter is required and must be non-empty".to_string(),
        };
        let task = match args.get("task").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return "Error: 'task' parameter is required and must be non-empty".to_string(),
        };
        let context = match args.get("context").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c,
            _ => return "Error: 'context' parameter is required and must be non-empty".to_string(),
        };

        // 2. Cannot handoff to self
        if target == self.agent.name {
            return "Error: cannot handoff to yourself".to_string();
        }

        // 3. Verify target agent exists and resolve its engine
        let agent_map = match &self.agent_map {
            Some(m) => m,
            None => return "Error: agent registry not available".to_string(),
        };
        let target_engine = {
            let map = agent_map.read().await;
            match map.get(target) {
                Some(h) => h.engine.clone(),
                None => {
                    return format!(
                        "Error: agent '{}' not found. Use agents_list to see available agents.",
                        target
                    );
                }
            }
        };

        // 4. Add participant to session
        let session_id = match *self.processing_session_id().lock().await {
            Some(id) => id,
            None => return "Error: no active session".to_string(),
        };
        let _ = SessionManager::new(self.db.clone()).add_participant(session_id, target).await;

        // 5. Register in target's subagent registry
        let (id, handle, cancel, completion_rx) = target_engine.subagent_registry().register(task).await;

        // 6. Build enriched task with context
        let full_task = format!(
            "[Handoff from {}]\nTask: {}\nContext: {}",
            self.agent.name, task, context
        );

        // 7. Spawn async subagent on TARGET engine
        let target_clone = target_engine.clone();
        let loop_max = target_engine.tool_loop_config().effective_max_iterations();
        // Parse timeout from config (e.g. "2m", "30s"), default 120s
        let timeout = super::subagent_impl::parse_subagent_timeout(&self.app_config.subagents.in_process_timeout);
        let deadline = Some(std::time::Instant::now() + timeout);
        let handle_clone = handle.clone();

        tokio::spawn(async move {
            let result = tokio::time::timeout(
                timeout,
                target_clone.run_subagent(&full_task, loop_max, deadline, Some(cancel), Some(handle_clone.clone()), None),
            ).await;
            let mut h = handle_clone.write().await;
            h.finished_at = Some(chrono::Utc::now());
            match result {
                Err(_elapsed) => {
                    h.status = subagent_state::SubagentStatus::Failed;
                    h.error = Some("timeout".to_string());
                }
                Ok(Ok(text)) => {
                    h.status = subagent_state::SubagentStatus::Completed;
                    h.result = Some(text);
                }
                Ok(Err(e)) => {
                    h.status = subagent_state::SubagentStatus::Failed;
                    h.error = Some(e.to_string());
                }
            }
            let sub_result = subagent_state::SubagentResult {
                status: h.status,
                result: h.result.clone(),
                error: h.error.clone(),
            };
            let maybe_tx = h.completion_tx.take();
            drop(h);
            if let Some(tx) = maybe_tx {
                let _ = tx.send(sub_result);
            }
        });

        // 8. Store pending handoff on INITIATOR (self)
        self.pending_handoffs.lock().await.push(PendingHandoff {
            subagent_id: id,
            target_name: target.to_string(),
            completion_rx,
        });

        tracing::info!(
            from = %self.agent.name,
            to = %target,
            task = %task,
            "handoff tool: spawned async subagent"
        );

        // 9. Return immediately
        format!("Handoff to {} accepted. Agent is working on the task. Result will be provided when complete.", target)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn handoff_accepted_format() {
        let target = "Researcher";
        let msg = format!(
            "Handoff to {} accepted. Agent is working on the task. Result will be provided when complete.",
            target
        );
        assert!(msg.starts_with("Handoff to Researcher accepted"));
        assert!(msg.contains("Result will be provided when complete"));
    }

    #[test]
    fn handoff_accepted_format_special_chars() {
        let target = "Agent-With-Dashes";
        let msg = format!(
            "Handoff to {} accepted. Agent is working on the task. Result will be provided when complete.",
            target
        );
        assert!(msg.contains("Agent-With-Dashes"));
    }
}
