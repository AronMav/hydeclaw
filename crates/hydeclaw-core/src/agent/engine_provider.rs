//! Provider retry/fallback helpers, budget checks, and audit logging.
//! Thin delegation layer — real logic lives in `pipeline::llm_call`.

use super::*;
use crate::agent::pipeline::llm_call::Compactor;

/// `AgentEngine` acts as its own compactor — delegates to `compact_messages`.
#[async_trait::async_trait]
impl Compactor for AgentEngine {
    async fn compact(&self, messages: &mut Vec<Message>) {
        self.compact_messages(messages, None).await;
    }
}

impl AgentEngine {
    /// Build tool loop config from agent TOML settings (or defaults).
    pub(crate) fn tool_loop_config(&self) -> crate::agent::tool_loop::ToolLoopConfig {
        self.agent
            .tool_loop
            .as_ref()
            .map(crate::agent::tool_loop::ToolLoopConfig::from)
            .unwrap_or_default()
    }

    /// Create fallback LLM provider from agent config.
    pub(super) async fn create_fallback_provider(&self) -> Option<Arc<dyn crate::agent::providers::LlmProvider>> {
        crate::agent::pipeline::llm_call::create_fallback_provider(
            &self.db,
            self.agent.fallback_provider.as_deref(),
            &self.agent.name,
            self.agent.temperature,
            self.agent.max_tokens,
            self.secrets().clone(),
            self.sandbox().clone(),
            &self.workspace_dir,
            self.agent.base,
        )
        .await
    }

    /// Check daily token budget before LLM call.
    pub(super) async fn check_budget(&self) -> Result<()> {
        crate::agent::pipeline::llm_call::check_budget(
            &self.db,
            &self.agent.name,
            self.agent.daily_budget_tokens,
        )
        .await
    }

    /// Call LLM with automatic context overflow recovery.
    pub(super) async fn chat_with_overflow_recovery(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_with_overflow_recovery(
            self.provider.as_ref(),
            messages,
            tools,
            self,
        )
        .await
    }

    /// Call LLM with exponential backoff retry.
    pub(super) async fn chat_with_transient_retry(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_with_transient_retry(
            self.provider.as_ref(),
            messages,
            tools,
            self,
        )
        .await
    }

    /// Streaming variant of chat_with_overflow_recovery.
    #[allow(dead_code)]
    pub(super) async fn chat_stream_with_overflow_recovery(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_stream_with_overflow_recovery(
            self.provider.as_ref(),
            messages,
            tools,
            chunk_tx,
            self,
        )
        .await
    }

    /// Streaming variant of chat_with_transient_retry.
    pub(super) async fn chat_stream_with_transient_retry(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_stream_with_transient_retry(
            self.provider.as_ref(),
            messages,
            tools,
            chunk_tx,
            self,
        )
        .await
    }

    /// Variant that uses an explicit provider (for fallback switching).
    pub(super) async fn chat_with_transient_retry_using(
        &self,
        provider: &Arc<dyn crate::agent::providers::LlmProvider>,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_with_transient_retry_using(
            provider,
            messages,
            tools,
            self,
        )
        .await
    }

    /// Streaming variant of chat_with_transient_retry_using.
    pub(super) async fn chat_stream_with_transient_retry_using(
        &self,
        provider: &Arc<dyn crate::agent::providers::LlmProvider>,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        crate::agent::pipeline::llm_call::chat_stream_with_transient_retry_using(
            provider,
            messages,
            tools,
            chunk_tx,
            self,
        )
        .await
    }

    /// Default context window size based on model name.
    pub(crate) fn default_context_for_model(model: &str) -> usize {
        crate::agent::pipeline::llm_call::default_context_for_model(model)
    }

    /// Fire-and-forget audit event recording.
    pub(super) fn audit(&self, event_type: &'static str, actor: Option<&str>, details: serde_json::Value) {
        crate::agent::pipeline::llm_call::audit(
            self.db.clone(),
            self.agent.name.clone(),
            event_type,
            actor,
            details,
        );
    }
}
