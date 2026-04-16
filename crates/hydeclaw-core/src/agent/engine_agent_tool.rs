//! Session-scoped agent tool handler — thin delegations to `pipeline::agent_tool`.
//! Included in engine.rs via `#[path = "engine_agent_tool.rs"] mod agent_tool_impl;`.

use super::*;

impl AgentEngine {
    /// Dispatch `agent` tool calls to the appropriate sub-handler based on `action`.
    pub(super) async fn handle_agent_tool(&self, args: &serde_json::Value) -> String {
        crate::agent::pipeline::agent_tool::handle_agent_tool(
            self.session_pools.as_ref(),
            self.agent_map.as_ref(),
            &self.db,
            &self.agent.name,
            args,
        )
        .await
    }
}
