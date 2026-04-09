//! Handoff tool handler -- transfer control to another agent with task + context.

use super::*;

// ── handoff handler ─────────────────────────────────────────────────────────

impl AgentEngine {
    /// Internal tool: hand off the current turn to another agent.
    /// Sets `handoff_target` which the turn loop in chat.rs reads after handle_sse returns.
    /// Adds participant to session AND transfers the active turn to the target agent.
    pub(super) async fn handle_handoff(&self, args: &serde_json::Value) -> String {
        // 1. Validate required parameters (D-02: agent, task, context all required)
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

        // 3. Verify target agent exists
        let agent_map = match &self.agent_map {
            Some(m) => m,
            None => return "Error: agent registry not available".to_string(),
        };
        {
            let map = agent_map.read().await;
            if !map.contains_key(target) {
                return format!(
                    "Error: agent '{}' not found. Use agents_list to see available agents.",
                    target
                );
            }
        }

        // 4. Prevent multiple handoffs in same turn (parallel tool execution guard)
        {
            let existing = self.handoff_target.lock().await;
            if existing.is_some() {
                return "Error: handoff already requested this turn. Only one handoff per turn is allowed.".to_string();
            }
        }

        // 5. Add participant to session (reuse invite_agent pattern, D-03)
        let session_id = match *self.processing_session_id().lock().await {
            Some(id) => id,
            None => return "Error: no active session".to_string(),
        };
        let _ = SessionManager::new(self.db.clone()).add_participant(session_id, target).await;

        // 6. Set handoff target (consumed by turn loop in chat.rs)
        *self.handoff_target.lock().await = Some(HandoffRequest {
            target_agent: target.to_string(),
            task: task.to_string(),
            context: context.to_string(),
        });

        tracing::info!(
            from = %self.agent.name,
            to = %target,
            task = %task,
            "handoff tool: transferring turn"
        );

        // 7. Return confirmation to LLM (D-04)
        format!("Handoff to {} accepted. They will respond next.", target)
    }
}
