//! Context building and session compaction.
//! Thin delegation layer — logic lives in `pipeline::context`.

use super::*;

impl AgentEngine {
    /// Build common context: session, messages, system prompt.
    /// Delegates to `self.context_builder.build(...)` — returns a `ContextSnapshot`.
    /// If `resume_session_id` is Some, reuses that session instead of creating/finding one.
    pub(super) async fn build_context(
        &self,
        msg: &IncomingMessage,
        include_tools: bool,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<crate::agent::context_builder::ContextSnapshot> {
        let cb = self.context_builder.get()
            .expect("context_builder not initialized — call set_context_builder after engine Arc creation");
        crate::agent::pipeline::context::build_context(cb.as_ref(), msg, include_tools, resume_session_id, force_new_session).await
    }

    /// Build a SecretsEnvResolver for YAML tool env resolution.
    pub(super) fn make_resolver(&self) -> SecretsEnvResolver {
        crate::agent::pipeline::context::make_resolver(self.secrets(), &self.agent.name)
    }

    /// Build OAuthContext for provider-based YAML tool auth (e.g. `oauth_provider: github`).
    pub(super) fn make_oauth_context(&self) -> Option<crate::tools::yaml_tools::OAuthContext> {
        crate::agent::pipeline::context::make_oauth_context(self.oauth().as_ref(), &self.agent.name)
    }

    /// Format a tool error as structured JSON for better LLM parsing.
    pub(super) fn format_tool_error(tool_name: &str, error: &str) -> String {
        crate::agent::pipeline::context::format_tool_error(tool_name, error)
    }

    /// Truncate a string to `max` chars with "..." suffix, preserving char boundaries.
    pub(super) fn truncate_preview(s: &str, max: usize) -> String {
        crate::agent::pipeline::context::truncate_preview(s, max)
    }

    /// Truncate a tool result to fit within remaining context budget.
    pub(super) fn truncate_tool_result(&self, result: &str, current_context_chars: usize) -> String {
        crate::agent::pipeline::context::truncate_tool_result(&self.agent.model, result, current_context_chars)
    }

    /// Replace old tool results with "[compacted]" when context exceeds 70% of model window.
    pub(super) fn compact_tool_results(&self, messages: &mut [Message], context_chars: &mut usize) {
        crate::agent::pipeline::context::compact_tool_results(
            &self.agent.model,
            self.agent.compaction.as_ref(),
            messages,
            context_chars,
        )
    }

    /// Get compaction parameters from agent config.
    #[allow(dead_code)]
    pub(super) fn compaction_params(&self) -> (usize, usize) {
        crate::agent::pipeline::context::compaction_params(&self.agent.model, self.agent.compaction.as_ref())
    }

    /// Run compaction on messages if token budget exceeded, indexing extracted facts to memory.
    pub(super) async fn compact_messages(&self, messages: &mut Vec<Message>, detector: Option<&LoopDetector>) {
        let engine = self;
        crate::agent::pipeline::context::compact_messages(
            &engine.agent.model,
            engine.agent.compaction.as_ref(),
            &engine.agent.language,
            engine.provider.as_ref(),
            engine.compaction_provider.as_deref(),
            &engine.db,
            engine.ui_event_tx.as_ref(),
            &engine.agent.name,
            &engine.audit_queue,
            messages,
            detector,
            |facts| async move { engine.index_facts_to_memory(&facts).await },
        )
        .await
    }

    /// Compact a specific session's messages via API.
    /// Returns `(facts_extracted, new_message_count)`.
    pub async fn compact_session(&self, session_id: uuid::Uuid) -> Result<(usize, usize)> {
        let engine = self;
        crate::agent::pipeline::context::compact_session(
            &engine.db,
            engine.provider.as_ref(),
            engine.compaction_provider.as_deref(),
            &engine.agent.language,
            &engine.agent.name,
            session_id,
            &engine.audit_queue,
            |facts| async move { engine.index_facts_to_memory(&facts).await },
        )
        .await
    }
}
