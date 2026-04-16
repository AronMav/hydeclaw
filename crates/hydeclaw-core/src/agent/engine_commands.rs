//! Slash command routing — thin delegation to `pipeline::commands`.

use super::*;
use crate::agent::pipeline::commands::{CommandContext, handle_command};

impl AgentEngine {
    /// Handle /slash commands. Returns Some(result) if a command matched, None otherwise.
    pub(super) async fn handle_command(&self, text: &str, msg: &IncomingMessage) -> Option<Result<String>> {
        let dm_scope = self.agent.session.as_ref()
            .map(|s| s.dm_scope.as_str())
            .unwrap_or("per-channel-peer");

        let ctx = CommandContext {
            agent_name: &self.agent.name,
            agent_language: &self.agent.language,
            agent_model: &self.agent.model,
            dm_scope,
            max_history_messages: self.agent.max_history_messages,
            compaction_config: self.agent.compaction.as_ref(),
            db: &self.db,
            provider: self.provider.as_ref(),
            compaction_provider: self.compaction_provider.as_deref(),
            thinking_level: &self.thinking_level,
            memory_store: self.memory_store.as_ref(),
        };

        handle_command(
            &ctx,
            text,
            msg,
            || async { self.invalidate_yaml_tools_cache().await },
        ).await
    }
}
