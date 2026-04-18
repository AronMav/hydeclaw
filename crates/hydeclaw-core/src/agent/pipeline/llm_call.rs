//! Pipeline step: llm_call — provider call, retry, fallback (migrated from engine_provider.rs).
//!
//! Free functions that encapsulate LLM retry/fallback/budget logic without depending on
//! `&AgentEngine`.  The engine methods in `engine_provider.rs` become thin delegations.

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;

use hydeclaw_types::{Message, ToolDefinition};
use crate::agent::error_classify;
use crate::agent::providers::LlmProvider;

// ── Budget ──────────────────────────────────────────────────────────

/// Check daily token budget before an LLM call.
/// Returns `Ok(())` if budget is unlimited (0) or not yet exhausted.
pub async fn check_budget(db: &sqlx::PgPool, agent_name: &str, daily_budget_tokens: u64) -> Result<()> {
    if daily_budget_tokens == 0 {
        return Ok(());
    }
    let used = crate::db::usage::get_agent_usage_today(db, agent_name)
        .await
        .unwrap_or(0) as u64;
    if used >= daily_budget_tokens {
        anyhow::bail!(
            "Daily token budget exceeded ({}/{} tokens). Resets at midnight.",
            used,
            daily_budget_tokens
        );
    }
    Ok(())
}

// ── Fallback provider ───────────────────────────────────────────────

/// Create a fallback LLM provider by looking up `fallback_provider` in the providers table.
/// Returns `None` if the name is absent, not found, or creation fails.
#[allow(clippy::too_many_arguments)]
pub async fn create_fallback_provider(
    db: &sqlx::PgPool,
    fallback_provider_name: Option<&str>,
    agent_name: &str,
    _temperature: f64,
    _max_tokens: Option<u32>,
    secrets: Arc<crate::secrets::SecretsManager>,
    sandbox: Option<Arc<crate::containers::sandbox::CodeSandbox>>,
    workspace_dir: &str,
    base: bool,
) -> Option<Arc<dyn LlmProvider>> {
    let fb_name = fallback_provider_name?;
    match crate::db::providers::get_provider_by_name(db, fb_name).await {
        Ok(Some(row)) => {
            use crate::agent::providers::{build_provider, build_cli_provider, CliContext};
            let opts: crate::agent::providers::timeouts::ProviderOptions =
                serde_json::from_value(row.options.clone()).unwrap_or_default();
            let timeouts_cfg = opts.timeouts;
            let cancel = tokio_util::sync::CancellationToken::new();

            let provider_box: Box<dyn LlmProvider> = match row.provider_type.as_str() {
                "claude-cli" | "gemini-cli" | "codex-cli" => {
                    let ctx = CliContext {
                        sandbox,
                        agent_name,
                        workspace_dir,
                        base,
                        secrets: secrets.clone(),
                    };
                    match build_cli_provider(&row, None, ctx).await {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(agent = %agent_name, fallback_provider = %fb_name, error = %e,
                                "failed to build fallback CLI provider");
                            return None;
                        }
                    }
                }
                _ => {
                    match build_provider(&row, secrets, &timeouts_cfg, cancel) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(agent = %agent_name, fallback_provider = %fb_name, error = %e,
                                "failed to build fallback provider");
                            return None;
                        }
                    }
                }
            };
            Some(Arc::from(provider_box))
        }
        Ok(None) => {
            tracing::warn!(
                agent = %agent_name,
                fallback_provider = %fb_name,
                "fallback provider not found in providers table"
            );
            None
        }
        Err(e) => {
            tracing::warn!(
                agent = %agent_name,
                fallback_provider = %fb_name,
                error = %e,
                "failed to look up fallback provider"
            );
            None
        }
    }
}

// ── Default context window ──────────────────────────────────────────

/// Default context window size based on model name.
pub fn default_context_for_model(model: &str) -> usize {
    if model.contains("claude") {
        200_000
    } else if model.contains("gpt-4") {
        128_000
    } else if model.contains("MiniMax") || model.contains("M2.5") || model.contains("gemini") {
        1_000_000
    } else {
        128_000
    }
}

// ── Overflow recovery (non-streaming) ───────────────────────────────

/// Call LLM with automatic context overflow recovery.
/// On context overflow (400), invokes `compact` and retries up to 3 times.
pub async fn chat_with_overflow_recovery(
    provider: &dyn LlmProvider,
    messages: &mut Vec<Message>,
    tools: &[ToolDefinition],
    compact: &impl Compactor,
) -> Result<hydeclaw_types::LlmResponse> {
    let max_compact_attempts: u8 = 3;
    let mut last_error = None;

    for compact_attempt in 0..=max_compact_attempts {
        let result = provider.chat(messages, tools).await;
        match result {
            Ok(resp) => return Ok(resp),
            Err(e)
                if crate::agent::tool_loop::is_context_overflow(&e)
                    && compact_attempt < max_compact_attempts =>
            {
                tracing::warn!(
                    attempt = compact_attempt + 1,
                    max = max_compact_attempts,
                    "context overflow — compacting"
                );
                compact.compact(messages).await;
                last_error = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!(
            "context overflow after {} compaction attempts",
            max_compact_attempts
        )
    }))
}

// ── Overflow recovery (streaming) ───────────────────────────────────

/// Streaming variant of [`chat_with_overflow_recovery`].
pub async fn chat_stream_with_overflow_recovery(
    provider: &dyn LlmProvider,
    messages: &mut Vec<Message>,
    tools: &[ToolDefinition],
    chunk_tx: mpsc::UnboundedSender<String>,
    compact: &impl Compactor,
) -> Result<hydeclaw_types::LlmResponse> {
    let max_compact_attempts: u8 = 3;
    let mut last_error = None;

    for compact_attempt in 0..=max_compact_attempts {
        let result = provider
            .chat_stream(messages, tools, chunk_tx.clone())
            .await;
        match result {
            Ok(resp) => return Ok(resp),
            Err(e)
                if crate::agent::tool_loop::is_context_overflow(&e)
                    && compact_attempt < max_compact_attempts =>
            {
                tracing::warn!(
                    attempt = compact_attempt + 1,
                    max = max_compact_attempts,
                    "context overflow — compacting (stream)"
                );
                compact.compact(messages).await;
                last_error = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!(
            "context overflow after {} compaction attempts (stream)",
            max_compact_attempts
        )
    }))
}

// ── Transient retry (non-streaming) ─────────────────────────────────

/// Call LLM with exponential backoff retry (up to 5 attempts, 500ms–32s).
/// Wraps [`chat_with_overflow_recovery`] to add engine-level transient retry.
/// RateLimit (429) uses full 60s cooldown; Retry-After header overrides both.
pub async fn chat_with_transient_retry(
    provider: &dyn LlmProvider,
    messages: &mut Vec<Message>,
    tools: &[ToolDefinition],
    compact: &impl Compactor,
) -> Result<hydeclaw_types::LlmResponse> {
    let config = error_classify::RetryConfig::default();
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..config.max_attempts {
        let result =
            chat_with_overflow_recovery(provider, messages, tools, compact).await;
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

// ── Transient retry (streaming) ─────────────────────────────────────

/// Streaming variant of [`chat_with_transient_retry`].
pub async fn chat_stream_with_transient_retry(
    provider: &dyn LlmProvider,
    messages: &mut Vec<Message>,
    tools: &[ToolDefinition],
    chunk_tx: mpsc::UnboundedSender<String>,
    compact: &impl Compactor,
) -> Result<hydeclaw_types::LlmResponse> {
    let config = error_classify::RetryConfig::default();
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..config.max_attempts {
        let result = chat_stream_with_overflow_recovery(
            provider,
            messages,
            tools,
            chunk_tx.clone(),
            compact,
        )
        .await;
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
    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("LLM stream call failed after retries")
    }))
}

// ── Transient retry with explicit provider (non-streaming) ──────────

/// Variant of [`chat_with_transient_retry`] that uses an explicit provider.
/// Used for fallback provider switching without modifying engine state.
pub async fn chat_with_transient_retry_using(
    provider: &Arc<dyn LlmProvider>,
    messages: &mut Vec<Message>,
    tools: &[ToolDefinition],
    compact: &impl Compactor,
) -> Result<hydeclaw_types::LlmResponse> {
    let config = error_classify::RetryConfig::default();
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..config.max_attempts {
        let result = match provider.chat(messages, tools).await {
            Ok(resp) => Ok(resp),
            Err(e) if crate::agent::tool_loop::is_context_overflow(&e) => {
                tracing::warn!("context overflow on fallback provider, compacting and retrying");
                compact.compact(messages).await;
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
    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("LLM call failed after retries (fallback provider)")
    }))
}

// ── Transient retry with explicit provider (streaming) ──────────────

/// Streaming variant of [`chat_with_transient_retry_using`].
pub async fn chat_stream_with_transient_retry_using(
    provider: &Arc<dyn LlmProvider>,
    messages: &mut Vec<Message>,
    tools: &[ToolDefinition],
    chunk_tx: mpsc::UnboundedSender<String>,
    compact: &impl Compactor,
) -> Result<hydeclaw_types::LlmResponse> {
    let config = error_classify::RetryConfig::default();
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..config.max_attempts {
        let result = match provider
            .chat_stream(messages, tools, chunk_tx.clone())
            .await
        {
            Ok(resp) => Ok(resp),
            Err(e) if crate::agent::tool_loop::is_context_overflow(&e) => {
                tracing::warn!(
                    "context overflow on fallback provider (stream), compacting and retrying"
                );
                compact.compact(messages).await;
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
    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("LLM stream call failed after retries (fallback provider)")
    }))
}

// ── Compactor trait ─────────────────────────────────────────────────

/// Trait abstracting message compaction so free functions don't depend on `AgentEngine`.
/// Implemented by `AgentEngine` (delegates to `compact_messages`).
#[async_trait::async_trait]
pub trait Compactor: Send + Sync {
    /// Compact the message list in-place (e.g. summarize, drop old messages).
    async fn compact(&self, messages: &mut Vec<Message>);
}

// ── Audit ───────────────────────────────────────────────────────────

/// Fire-and-forget audit event recording.
pub fn audit(
    db: sqlx::PgPool,
    agent_name: String,
    event_type: &'static str,
    actor: Option<&str>,
    details: serde_json::Value,
) {
    crate::db::audit::audit_spawn(
        db,
        agent_name,
        event_type,
        actor.map(|s| s.to_string()),
        details,
    );
}
