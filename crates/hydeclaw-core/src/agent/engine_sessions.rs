//! Session-related internal tools — thin delegations to `pipeline::sessions`.

use super::*;

impl AgentEngine {
    /// Internal tool: list recent sessions for this agent.
    pub(super) async fn handle_sessions_list(&self, args: &serde_json::Value) -> String {
        crate::agent::pipeline::sessions::handle_sessions_list(
            &self.db,
            &self.agent.name,
            args,
        )
        .await
    }

    /// Internal tool: retrieve message history from a specific session.
    pub(super) async fn handle_sessions_history(&self, args: &serde_json::Value) -> String {
        crate::agent::pipeline::sessions::handle_sessions_history(
            &self.db,
            &self.agent.name,
            args,
        )
        .await
    }

    /// Internal tool: list all running agents with their provider and model info.
    pub(super) async fn handle_agents_list(&self, args: &serde_json::Value) -> String {
        crate::agent::pipeline::sessions::handle_agents_list(
            self.agent_map.as_ref(),
            self.session_pools.as_ref(),
            &self.agent.name,
            args,
        )
        .await
    }

    /// Internal tool: search messages across all sessions by content.
    pub(super) async fn handle_session_search(&self, args: &serde_json::Value) -> String {
        crate::agent::pipeline::sessions::handle_session_search(
            &self.db,
            &self.agent.name,
            args,
        )
        .await
    }

    /// Internal tool: get metadata about the current session.
    pub(super) async fn handle_session_context(&self, args: &serde_json::Value) -> String {
        crate::agent::pipeline::sessions::handle_session_context(&self.db, args).await
    }

    /// Internal tool: send a message to a specific user/channel via channel adapter.
    pub(super) async fn handle_session_send(&self, args: &serde_json::Value) -> String {
        crate::agent::pipeline::sessions::handle_session_send(
            self.channel_router.as_ref(),
            args,
        )
        .await
    }

    /// Internal tool: export a session's full conversation as text.
    pub(super) async fn handle_session_export(&self, args: &serde_json::Value) -> String {
        crate::agent::pipeline::sessions::handle_session_export(&self.db, args).await
    }
}
