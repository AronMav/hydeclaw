//! Handoff tool handler -- transfer control to another agent with task + context.

use super::*;

// ── handoff handler ─────────────────────────────────────────────────────────

impl AgentEngine {
    /// Internal tool: hand off the current turn to another agent.
    /// Sets `handoff_target` which the turn loop in chat.rs reads after handle_sse returns.
    /// Adds participant to session AND transfers the active turn to the target agent.
    pub(super) async fn handle_handoff(&self, args: &serde_json::Value) -> String {
        // 1. Validate required parameters
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

        // 3. Resolve target agent engine
        let agent_map = match &self.agent_map {
            Some(m) => m,
            None => return "Error: agent registry not available".to_string(),
        };
        let target_engine = {
            let map = agent_map.read().await;
            match map.get(target) {
                Some(h) => h.engine.clone(),
                None => return format!(
                    "Error: agent '{}' not found. Use agents_list to see available agents.",
                    target
                ),
            }
        };

        // 4. Add participant to session
        if let Some(sid) = *self.processing_session_id().lock().await {
            let _ = crate::agent::session_manager::SessionManager::new(self.db.clone())
                .add_participant(sid, target).await;
        }

        tracing::info!(
            from = %self.agent.name,
            to = %target,
            task = %task,
            "handoff tool: running target agent in isolated context"
        );

        // 5. Run target agent as isolated subagent — own system prompt, own context.
        // Target sees ONLY the task+context, not the initiator's full conversation.
        let full_task = format!(
            "{}\n\nContext from {}:\n{}",
            task, self.agent.name, context
        );

        let timeout = std::time::Duration::from_secs(120);
        match tokio::time::timeout(
            timeout,
            target_engine.run_subagent(&full_task, 30, Some(std::time::Instant::now() + timeout), None, None, None),
        ).await {
            Ok(Ok(response)) => {
                tracing::info!(from = %target, to = %self.agent.name, response_len = response.len(), "handoff complete");
                format!("[Response from {}]\n{}", target, response)
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, agent = %target, "handoff agent failed");
                format!("[Error from {}] {}", target, e)
            }
            Err(_) => {
                tracing::warn!(agent = %target, "handoff agent timed out after 120s");
                format!("[Timeout from {}] Agent did not respond within 120 seconds.", target)
            }
        }
    }
}
