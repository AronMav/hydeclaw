//! Provider retry/fallback helpers, budget checks, and audit logging.
//! Extracted from engine.rs for readability.

use super::*;

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
    /// Returns None if fallback_provider is not configured or if provider creation fails.
    /// Looks up the connection by name in the providers table and creates a provider from it.
    pub(super) async fn create_fallback_provider(&self) -> Option<Arc<dyn crate::agent::providers::LlmProvider>> {
        let fb_name = self.agent.fallback_provider.as_deref()?;
        match crate::db::providers::get_provider_by_name(&self.db, fb_name).await {
            Ok(Some(row)) => {
                let p = crate::agent::providers::create_provider_from_connection(
                    &row,
                    None,
                    self.agent.temperature,
                    self.agent.max_tokens,
                    self.secrets().clone(),
                    self.sandbox().clone(),
                    &self.agent.name,
                    &self.workspace_dir,
                    self.agent.base,
                ).await;
                Some(p)
            }
            Ok(None) => {
                tracing::warn!(
                    agent = %self.agent.name,
                    fallback_provider = %fb_name,
                    "fallback provider not found in providers table"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    agent = %self.agent.name,
                    fallback_provider = %fb_name,
                    error = %e,
                    "failed to look up fallback provider"
                );
                None
            }
        }
    }

    /// Check daily token budget before LLM call.
    pub(super) async fn check_budget(&self) -> Result<()> {
        let budget = self.agent.daily_budget_tokens;
        if budget == 0 { return Ok(()); }
        let used = crate::db::usage::get_agent_usage_today(&self.db, &self.agent.name)
            .await.unwrap_or(0) as u64;
        if used >= budget {
            anyhow::bail!("Daily token budget exceeded ({}/{} tokens). Resets at midnight.", used, budget);
        }
        Ok(())
    }

    /// Call LLM with automatic context overflow recovery.
    /// On context overflow (400), compacts messages and retries up to 3 times.
    pub(super) async fn chat_with_overflow_recovery(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        let max_compact_attempts: u8 = 3;
        let mut last_error = None;

        for compact_attempt in 0..=max_compact_attempts {
            let result = self.provider.chat(messages, tools).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) if crate::agent::tool_loop::is_context_overflow(&e) && compact_attempt < max_compact_attempts => {
                    tracing::warn!(attempt = compact_attempt + 1, max = max_compact_attempts, "context overflow — compacting");
                    self.compact_messages(messages, None).await;
                    last_error = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("context overflow after {} compaction attempts", max_compact_attempts)))
    }

    /// Call LLM with exponential backoff retry (up to 5 attempts, 500ms–32s).
    /// Wraps chat_with_overflow_recovery to add engine-level transient retry
    /// when ALL providers (including fallbacks) returned a retryable error.
    /// RateLimit (429) uses full 60s cooldown; Retry-After header overrides both.
    pub(super) async fn chat_with_transient_retry(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        let config = error_classify::RetryConfig::default();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..config.max_attempts {
            let result = self.chat_with_overflow_recovery(messages, tools).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let class = error_classify::classify(&e);
                    if !error_classify::is_retryable(&class) {
                        return Err(e);
                    }
                    let delay = error_classify::extract_retry_after(&e.to_string())
                        .unwrap_or_else(|| config.retry_delay_for_error(&class, attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = config.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error_class = ?class,
                        error = %e,
                        "retrying LLM call"
                    );
                    last_error = Some(e);
                    if attempt < config.max_attempts - 1 {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM call failed after retries")))
    }

    /// Streaming variant of chat_with_overflow_recovery.
    /// On context overflow (400), compacts messages and retries up to 3 times.
    pub(super) async fn chat_stream_with_overflow_recovery(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        let max_compact_attempts: u8 = 3;
        let mut last_error = None;

        for compact_attempt in 0..=max_compact_attempts {
            let result = self.provider.chat_stream(messages, tools, chunk_tx.clone()).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) if crate::agent::tool_loop::is_context_overflow(&e) && compact_attempt < max_compact_attempts => {
                    tracing::warn!(attempt = compact_attempt + 1, max = max_compact_attempts, "context overflow — compacting (stream)");
                    self.compact_messages(messages, None).await;
                    last_error = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("context overflow after {} compaction attempts (stream)", max_compact_attempts)))
    }

    /// Streaming variant of chat_with_transient_retry.
    /// Uses identical exponential backoff logic; passes a fresh clone of chunk_tx on each retry.
    pub(super) async fn chat_stream_with_transient_retry(
        &self,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        let config = error_classify::RetryConfig::default();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..config.max_attempts {
            let result = self.chat_stream_with_overflow_recovery(messages, tools, chunk_tx.clone()).await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let class = error_classify::classify(&e);
                    if !error_classify::is_retryable(&class) {
                        return Err(e);
                    }
                    let delay = error_classify::extract_retry_after(&e.to_string())
                        .unwrap_or_else(|| config.retry_delay_for_error(&class, attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = config.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error_class = ?class,
                        error = %e,
                        "retrying LLM call (stream)"
                    );
                    last_error = Some(e);
                    if attempt < config.max_attempts - 1 {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM stream call failed after retries")))
    }

    /// Variant of chat_with_transient_retry that uses an explicit provider instead of self.provider.
    /// Used for fallback provider switching without modifying engine state.
    pub(super) async fn chat_with_transient_retry_using(
        &self,
        provider: &Arc<dyn crate::agent::providers::LlmProvider>,
        messages: &mut Vec<Message>,
        tools: &[ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        self.check_budget().await?;
        let config = error_classify::RetryConfig::default();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..config.max_attempts {
            let result = match provider.chat(messages, tools).await {
                Ok(resp) => Ok(resp),
                Err(e) if crate::agent::tool_loop::is_context_overflow(&e) => {
                    tracing::warn!("context overflow on fallback provider, compacting and retrying");
                    self.compact_messages(messages, None).await;
                    provider.chat(messages, tools).await
                }
                Err(e) => Err(e),
            };
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let class = error_classify::classify(&e);
                    if !error_classify::is_retryable(&class) {
                        return Err(e);
                    }
                    let delay = error_classify::extract_retry_after(&e.to_string())
                        .unwrap_or_else(|| config.retry_delay_for_error(&class, attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = config.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error_class = ?class,
                        error = %e,
                        "retrying LLM call (fallback provider)"
                    );
                    last_error = Some(e);
                    if attempt < config.max_attempts - 1 {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM call failed after retries (fallback provider)")))
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
        let config = error_classify::RetryConfig::default();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..config.max_attempts {
            let result = match provider.chat_stream(messages, tools, chunk_tx.clone()).await {
                Ok(resp) => Ok(resp),
                Err(e) if crate::agent::tool_loop::is_context_overflow(&e) => {
                    tracing::warn!("context overflow on fallback provider (stream), compacting and retrying");
                    self.compact_messages(messages, None).await;
                    provider.chat_stream(messages, tools, chunk_tx.clone()).await
                }
                Err(e) => Err(e),
            };
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let class = error_classify::classify(&e);
                    if !error_classify::is_retryable(&class) {
                        return Err(e);
                    }
                    let delay = error_classify::extract_retry_after(&e.to_string())
                        .unwrap_or_else(|| config.retry_delay_for_error(&class, attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = config.max_attempts,
                        delay_ms = delay.as_millis() as u64,
                        error_class = ?class,
                        error = %e,
                        "retrying LLM call (fallback provider, stream)"
                    );
                    last_error = Some(e);
                    if attempt < config.max_attempts - 1 {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM stream call failed after retries (fallback provider)")))
    }

    /// Default context window size based on model name.
    pub(crate) fn default_context_for_model(model: &str) -> usize {
        if model.contains("claude") { 200_000 }
        else if model.contains("gpt-4") { 128_000 }
        else if model.contains("MiniMax") || model.contains("M2.5") || model.contains("gemini") { 1_000_000 }
        else { 128_000 }
    }

    /// Fire-and-forget audit event recording.
    pub(super) fn audit(&self, event_type: &'static str, actor: Option<&str>, details: serde_json::Value) {
        crate::db::audit::audit_spawn(
            self.db.clone(),
            self.agent.name.clone(),
            event_type,
            actor.map(|s| s.to_string()),
            details,
        );
    }
}
