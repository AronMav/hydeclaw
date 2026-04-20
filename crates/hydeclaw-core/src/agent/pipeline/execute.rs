//! Main LLM+tools loop. Transport-agnostic via EventSink.
//!
//! See docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md §3, §5.
//!
//! Task 6a scope: happy path (one LLM call, batched TextDelta, Finish).
//! Task 6b extends with tool-call iteration, loop detector, error paths.

// Tasks 7-9 wire execute() into the real call-sites; allow dead_code until then.
#![allow(dead_code)]

use crate::agent::engine::AgentEngine;
use crate::agent::pipeline::bootstrap::BootstrapOutcome;
use crate::agent::pipeline::sink::{EventSink, SinkError};
use crate::agent::stream_event::StreamEvent;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ── Outcome types ────────────────────────────────────────────────────────────

pub struct ExecuteOutcome {
    pub status: ExecuteStatus,
    pub session_id: Uuid,
    pub final_text: String,
    /// Thinking blocks from extended thinking (Anthropic only). None for Task 6a.
    /// Task 6b will populate this from LlmResponse::thinking_blocks.
    pub thinking_json: Option<serde_json::Value>,
    pub messages_len_at_end: usize,
}

#[derive(Debug)]
pub enum ExecuteStatus {
    Done,
    Failed(String),
    /// Execution stopped before finishing. Reason is a static label for logging.
    Interrupted(&'static str),
}

// ── execute() ────────────────────────────────────────────────────────────────

/// Run the LLM call and stream the result into `sink`.
///
/// Task 6a implements only the happy path: one LLM call, batched TextDelta, Finish.
/// No tool loop yet — that is Task 6b.
pub async fn execute<S: EventSink>(
    engine: &AgentEngine,
    bootstrap_outcome: BootstrapOutcome,
    sink: &mut S,
    cancel: CancellationToken,
) -> anyhow::Result<ExecuteOutcome> {
    let BootstrapOutcome {
        session_id,
        mut messages,
        tools,
        loop_detector: _loop_detector,
        processing_guard: _processing_guard, // Drop handles cleanup
        lifecycle_guard: _lifecycle_guard,
        // enriched_text and command_output not needed for the LLM call itself
        enriched_text: _,
        command_output: _,
    } = bootstrap_outcome;

    // Bail early if cancel was already signalled before we start.
    if cancel.is_cancelled() {
        return Ok(ExecuteOutcome {
            status: ExecuteStatus::Interrupted("cancel_token"),
            session_id,
            final_text: String::new(),
            thinking_json: None,
            messages_len_at_end: messages.len(),
        });
    }

    // Signal the start of a message to the sink.
    let msg_id = format!("msg_{}", Uuid::new_v4());
    if sink
        .emit(StreamEvent::MessageStart { message_id: msg_id }.into())
        .await
        .is_err()
    {
        return Ok(ExecuteOutcome {
            status: ExecuteStatus::Interrupted("sink_closed"),
            session_id,
            final_text: String::new(),
            thinking_json: None,
            messages_len_at_end: messages.len(),
        });
    }

    // ── Spawn-forwarder pattern ──────────────────────────────────────────────
    // EventSink is not Clone, so we cannot forward each chunk directly to the
    // sink from a spawned task. Instead, a forwarder task accumulates all chunks
    // into a String and returns it via a oneshot. execute() then emits the full
    // accumulated text as a single TextDelta once the LLM call completes.
    //
    // Task 6b will replace this with proper per-chunk streaming.
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (partial_tx, partial_rx) = tokio::sync::oneshot::channel::<String>();

    let forwarder = tokio::spawn(async move {
        let mut buf = String::new();
        while let Some(chunk) = chunk_rx.recv().await {
            buf.push_str(&chunk);
        }
        let _ = partial_tx.send(buf);
    });

    // Call the free function directly — the `pub(super)` method on AgentEngine
    // is not accessible from this module. AgentEngine implements Compactor, so
    // it can serve as both provider source and compaction delegate.
    let provider = engine.cfg().provider.as_ref();
    let llm_result = crate::agent::pipeline::llm_call::chat_stream_with_transient_retry(
        provider,
        &mut messages,
        &tools,
        chunk_tx,
        engine,
    )
    .await;

    // Drain the forwarder (chunk_tx was moved into LLM call and dropped on return).
    let _ = forwarder.await;
    let partial = partial_rx.await.unwrap_or_default();

    match llm_result {
        Ok(_response) => {
            // Emit accumulated text, downgrading to Interrupted if the sink closed.
            if !partial.is_empty() {
                match sink.emit(StreamEvent::TextDelta(partial.clone()).into()).await {
                    Ok(()) => {}
                    Err(SinkError::Closed) => {
                        return Ok(ExecuteOutcome {
                            status: ExecuteStatus::Interrupted("sink_closed"),
                            session_id,
                            final_text: partial,
                            thinking_json: None,
                            messages_len_at_end: messages.len(),
                        });
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            match sink
                .emit(
                    StreamEvent::Finish {
                        finish_reason: "stop".into(),
                        continuation: false,
                    }
                    .into(),
                )
                .await
            {
                Ok(()) => {}
                Err(SinkError::Closed) => {
                    return Ok(ExecuteOutcome {
                        status: ExecuteStatus::Interrupted("sink_closed"),
                        session_id,
                        final_text: partial,
                        thinking_json: None,
                        messages_len_at_end: messages.len(),
                    });
                }
                Err(e) => return Err(e.into()),
            }

            Ok(ExecuteOutcome {
                status: ExecuteStatus::Done,
                session_id,
                final_text: partial,
                thinking_json: None,
                messages_len_at_end: messages.len(),
            })
        }
        Err(e) => Ok(ExecuteOutcome {
            status: ExecuteStatus::Failed(e.to_string()),
            session_id,
            final_text: partial,
            thinking_json: None,
            messages_len_at_end: messages.len(),
        }),
    }
}

// No inline #[cfg(test)] module for Task 6a. Tests require a live
// AgentEngine which is architecturally blocked from inline unit tests
// (see lib.rs 10-module cap, spec §1). Coverage via Task 12 smoke test
// and future CI integration tests once Phase 66 REF-01 exposes a test
// surface for AgentEngine.
