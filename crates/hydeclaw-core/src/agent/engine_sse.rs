//! SSE entry point: thin adapter over pipeline::{bootstrap, execute, finalize}.
//!
//! TODO(phase-66): The old handle_sse implementation supported:
//! - Fallback provider switching on consecutive_failures
//! - SessionCorruption recovery (reset + retry)
//! - Empty-response auto-retry
//! - Auto-continue detection (looks_incomplete nudge)
//! - Forced final LLM call at turn_limit
//! - Rich-card/file marker extraction from tool results via entry_helpers
//! Those features are not yet ported into pipeline::execute.
//! Task 12 smoke test and CI will reveal which of them users depend on.

use super::*;
use crate::agent::pipeline::bootstrap::{self, BootstrapContext, BootstrapOutcome};
use crate::agent::pipeline::sink::{self, EventSink, PipelineEvent};
use crate::agent::pipeline::{execute, finalize};
use crate::agent::stream_event::StreamEvent;

impl AgentEngine {
    /// Handle message via SSE: thin adapter over pipeline::{bootstrap, execute, finalize}.
    ///
    /// Phase 62 RES-01: `event_tx` is an `EngineEventSender` wrapping a bounded
    /// `mpsc::Sender<StreamEvent>` (capacity 256 in chat.rs).
    pub async fn handle_sse(
        &self,
        msg: &IncomingMessage,
        event_tx: crate::agent::engine_event_sender::EngineEventSender,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<Uuid> {
        if let crate::agent::hooks::HookAction::Block(reason) =
            self.hooks().fire(&crate::agent::hooks::HookEvent::BeforeMessage)
        {
            anyhow::bail!("blocked by hook: {}", reason);
        }
        let _cancel_guard = self.state.register_request();

        let mut s = sink::SseSink::new(event_tx);

        let boot = bootstrap::bootstrap(
            self,
            BootstrapContext {
                msg,
                resume_session_id,
                force_new_session,
                use_history: true,
            },
            &mut s,
        )
        .await?;

        let BootstrapOutcome {
            session_id,
            messages,
            tools,
            loop_detector,
            processing_guard,
            lifecycle_guard,
            mut command_output,
            enriched_text,
        } = boot;
        let mut lifecycle_guard = lifecycle_guard.expect("bootstrap always sets lifecycle_guard");

        // Emit SessionId so the UI can track which session is active.
        let _ = s
            .emit(PipelineEvent::Stream(StreamEvent::SessionId(
                session_id.to_string(),
            )))
            .await;

        let boot_for_execute = BootstrapOutcome {
            lifecycle_guard: None,
            command_output: None,
            session_id,
            messages,
            tools,
            loop_detector,
            processing_guard,
            enriched_text,
        };

        // Slash-command early exit
        if let Some(text) = command_output.take() {
            let msg_id = format!("msg_{}", Uuid::new_v4());
            let _ = s
                .emit(PipelineEvent::Stream(StreamEvent::MessageStart {
                    message_id: msg_id,
                }))
                .await;
            let _ = s
                .emit(PipelineEvent::Stream(StreamEvent::TextDelta(text.clone())))
                .await;
            let _ = s
                .emit(PipelineEvent::Stream(StreamEvent::Finish {
                    finish_reason: "command".to_string(),
                    continuation: false,
                }))
                .await;

            let fin_ctx = finalize::finalize_context_from_engine(
                self,
                session_id,
                boot_for_execute.messages.len(),
                msg,
            );
            finalize::finalize(
                fin_ctx,
                finalize::FinalizeOutcome::Done {
                    assistant_text: text,
                    thinking_json: None,
                },
                &mut s,
                &mut lifecycle_guard,
            )
            .await?;
            return Ok(session_id);
        }

        // Full pipeline
        let cancel = tokio_util::sync::CancellationToken::new();
        let outcome = execute::execute(self, boot_for_execute, &mut s, cancel).await?;

        let fin_ctx = finalize::finalize_context_from_engine(
            self,
            session_id,
            outcome.messages_len_at_end,
            msg,
        );
        let fin_outcome = finalize::execute_status_to_finalize(
            outcome.status,
            outcome.final_text,
            outcome.thinking_json,
        );
        finalize::finalize(fin_ctx, fin_outcome, &mut s, &mut lifecycle_guard).await?;

        Ok(session_id)
    }
}

// ── helpers (kept for symmetry — used by engine_execution.rs via super::sse_impl) ──

/// Task 19: persist `partial_text` from a cancel-class `LlmCallError` before
/// the error is surfaced, so cancellation never loses work already produced.
///
/// Shared by engine_execution.rs (non-SSE path) via `super::sse_impl::persist_partial_if_any`.
pub(super) async fn persist_partial_if_any(
    db: &sqlx::PgPool,
    session_id: uuid::Uuid,
    agent_name: &str,
    parent_message_id: uuid::Uuid,
    e: &anyhow::Error,
) -> Option<uuid::Uuid> {
    let llm_err = e.downcast_ref::<crate::agent::providers::LlmCallError>()?;
    let (Some(partial), Some(reason)) = (llm_err.partial_text(), llm_err.abort_reason()) else {
        return None;
    };
    if partial.is_empty() {
        return None;
    }
    match crate::db::sessions::insert_assistant_partial(
        db,
        session_id,
        Some(agent_name),
        partial,
        Some(reason),
        Some(parent_message_id),
    )
    .await
    {
        Ok(partial_id) => {
            tracing::info!(
                session_id = %session_id,
                agent = %agent_name,
                abort_reason = reason,
                bytes = partial.len().min(crate::db::sessions::MAX_PARTIAL_BYTES),
                partial_message_id = %partial_id,
                "persisted partial assistant message before surfacing cancel-class LLM error"
            );
            Some(partial_id)
        }
        Err(persist_err) => {
            tracing::warn!(
                session_id = %session_id,
                agent = %agent_name,
                abort_reason = reason,
                error = %persist_err,
                "failed to persist partial assistant message on cancel; original LLM error still propagates"
            );
            None
        }
    }
}

/// Bump `llm_timeout_total{provider, kind}` when the engine catches a
/// timeout-class `LlmCallError`. Single-route agents bypass `RoutingProvider`
/// entirely, so the counter was previously only populated when failover fired.
pub(super) fn record_llm_timeout_if_typed(e: &anyhow::Error) {
    let Some(llm_err) = e.downcast_ref::<crate::agent::providers::LlmCallError>() else {
        return;
    };
    let Some(metrics) = crate::metrics::global() else {
        return;
    };
    use crate::agent::providers::LlmCallError::*;
    match llm_err {
        ConnectTimeout { provider, .. } => metrics.record_llm_timeout(provider, "connect"),
        RequestTimeout { provider, .. } => metrics.record_llm_timeout(provider, "request"),
        InactivityTimeout { provider, .. } => metrics.record_llm_timeout(provider, "inactivity"),
        MaxDurationExceeded { provider, .. } => metrics.record_llm_timeout(provider, "max_duration"),
        _ => {}
    }
}

/// Explicit status for `record_aborted_usage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UsageAbortStatus {
    /// Call was terminated without attempting a sibling provider.
    Aborted,
    /// Call was terminated AND a sibling provider was attempted.
    AbortedFailover,
}

/// Record an `aborted` / `aborted_failover` row in `usage_log` when an LLM call
/// is terminated before it could complete naturally.
pub(super) async fn record_aborted_usage(
    db: &sqlx::PgPool,
    agent_name: &str,
    provider_name: &str,
    model: &str,
    session_id: uuid::Uuid,
    e: &anyhow::Error,
    status: UsageAbortStatus,
) {
    use crate::db::usage::{insert_aborted_row, STATUS_ABORTED, STATUS_ABORTED_FAILOVER};
    let Some(llm_err) = e.downcast_ref::<crate::agent::providers::LlmCallError>() else {
        return;
    };
    let partial = llm_err.partial_text().unwrap_or("");
    let status = match status {
        UsageAbortStatus::Aborted => STATUS_ABORTED,
        UsageAbortStatus::AbortedFailover => STATUS_ABORTED_FAILOVER,
    };
    let est_output_tokens = (partial.len() / 4).min(u32::MAX as usize) as u32;
    match insert_aborted_row(
        db,
        agent_name,
        provider_name,
        model,
        session_id,
        est_output_tokens,
        status,
    )
    .await
    {
        Ok(()) => tracing::debug!(
            session_id = %session_id,
            agent = %agent_name,
            provider = %provider_name,
            status = %status,
            est_output_tokens,
            "recorded aborted usage row"
        ),
        Err(err) => tracing::debug!(
            session_id = %session_id,
            agent = %agent_name,
            provider = %provider_name,
            status = %status,
            error = %err,
            "failed to record aborted usage row (non-fatal)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn source() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/agent/engine_sse.rs");
        std::fs::read_to_string(&path).expect("read engine_sse.rs")
    }

    #[test]
    fn handle_sse_uses_bootstrap_and_execute() {
        let src = source();
        assert!(
            src.contains("bootstrap::bootstrap"),
            "handle_sse must call pipeline::bootstrap"
        );
        assert!(
            src.contains("execute::execute"),
            "handle_sse must call pipeline::execute"
        );
        assert!(
            src.contains("finalize::finalize"),
            "handle_sse must call pipeline::finalize"
        );
    }

    #[test]
    fn handle_sse_emits_session_id() {
        let src = source();
        assert!(
            src.contains("StreamEvent::SessionId"),
            "handle_sse must emit SessionId so UI can track the session"
        );
    }

    #[test]
    fn slash_command_path_emits_finish() {
        let src = source();
        assert!(
            src.contains(r#"finish_reason: "command""#),
            "slash-command path must emit Finish with finish_reason=command"
        );
    }
}
