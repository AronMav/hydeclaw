//! Channel/session execution entry points: handle_with_status, handle_streaming.
//! Thin adapters over pipeline::{bootstrap, execute, finalize}.
//!
//! TODO(phase-66): The old handle_with_status / handle_streaming implementations supported:
//! - Fallback provider switching on consecutive_failures
//! - SessionCorruption recovery (reset + retry)
//! - Empty-response auto-retry
//! - Auto-continue detection (looks_incomplete / length-limit nudge)
//! - Forced final LLM call at turn_limit
//! - Skill capture hints (multi-step tool iteration footer)
//! - Token usage footer for non-streaming responses
//! Those features are not yet ported into pipeline::execute.
//! Task 12 smoke test and CI will reveal which of them users depend on.

use super::*;
use crate::agent::pipeline::bootstrap::{self, BootstrapContext, BootstrapOutcome};
use crate::agent::pipeline::sink::{self, EventSink, PipelineEvent};
use crate::agent::pipeline::{execute, finalize};
use crate::agent::stream_event::StreamEvent;

impl AgentEngine {
    /// Handle with optional status callback for real-time phase updates.
    /// `chunk_tx` — optional channel for streaming response chunks to the caller.
    ///
    /// Thin adapter over pipeline::{bootstrap, execute, finalize} using `ChannelStatusSink`.
    pub async fn handle_with_status(
        &self,
        msg: &IncomingMessage,
        status_tx: Option<tokio::sync::mpsc::UnboundedSender<ProcessingPhase>>,
        chunk_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<String> {
        self.cfg().approval_manager.prune_stale().await;

        if let crate::agent::hooks::HookAction::Block(reason) =
            self.hooks().fire(&crate::agent::hooks::HookEvent::BeforeMessage)
        {
            anyhow::bail!("blocked by hook: {}", reason);
        }
        let _cancel_guard = self.state.register_request();

        let mut s = sink::ChannelStatusSink::new(status_tx, chunk_tx);

        let boot = bootstrap::bootstrap(
            self,
            BootstrapContext {
                msg,
                resume_session_id: None,
                force_new_session: false,
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
        let mut lifecycle_guard = lifecycle_guard.expect("set by bootstrap");
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

        // Channel adapters render slash commands as plain TextDelta
        if let Some(text) = command_output.take() {
            let _ = s
                .emit(PipelineEvent::Stream(StreamEvent::TextDelta(text.clone())))
                .await;
            let fin_ctx = finalize::finalize_context_from_engine(
                self,
                session_id,
                boot_for_execute.messages.len(),
                msg,
            );
            return finalize::finalize(
                fin_ctx,
                finalize::FinalizeOutcome::Done {
                    assistant_text: text,
                    thinking_json: None,
                },
                &mut s,
                &mut lifecycle_guard,
            )
            .await;
        }

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
        finalize::finalize(fin_ctx, fin_outcome, &mut s, &mut lifecycle_guard).await
    }

    /// Handle with streaming: sends content chunks via mpsc channel for progressive display.
    ///
    /// Thin adapter over pipeline::{bootstrap, execute, finalize} using `ChunkSink`.
    /// Uses `use_history: false` (matches old behaviour — streaming callers get no prior context).
    pub async fn handle_streaming(
        &self,
        msg: &IncomingMessage,
        chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let mut s = sink::ChunkSink::new(chunk_tx);

        let boot = bootstrap::bootstrap(
            self,
            BootstrapContext {
                msg,
                resume_session_id: None,
                force_new_session: false,
                use_history: false,
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
        let mut lifecycle_guard = lifecycle_guard.expect("set by bootstrap");
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

        if let Some(text) = command_output.take() {
            let _ = s
                .emit(PipelineEvent::Stream(StreamEvent::TextDelta(text.clone())))
                .await;
            let fin_ctx = finalize::finalize_context_from_engine(
                self,
                session_id,
                boot_for_execute.messages.len(),
                msg,
            );
            return finalize::finalize(
                fin_ctx,
                finalize::FinalizeOutcome::Done {
                    assistant_text: text,
                    thinking_json: None,
                },
                &mut s,
                &mut lifecycle_guard,
            )
            .await;
        }

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
        finalize::finalize(fin_ctx, fin_outcome, &mut s, &mut lifecycle_guard).await
    }
}
