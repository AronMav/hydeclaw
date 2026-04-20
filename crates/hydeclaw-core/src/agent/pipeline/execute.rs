//! Main LLM+tools loop. Transport-agnostic via EventSink.
//!
//! See docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md §3, §5.
//!
//! # Scope
//!
//! This module implements the **safe subset** of the tool loop:
//! - Happy path: N LLM calls with tool-call iterations
//! - Cancellation check at top of each iteration
//! - Sink closed → Interrupted
//! - LLM provider error (after retry exhaustion) → Failed
//! - LoopDetector trip (after max nudges) → Failed
//! - Turn limit reached → Done with finish_reason = "turn_limit"
//!
//! # Explicitly omitted (deferred to Phase 66)
//!
//! - Fallback provider switching on consecutive_failures (`using_fallback` path).
//!   The thin adapters in `engine/run.rs` use a single provider per session entry.
//! - SessionCorruption recovery (messages reset + retry). Pipeline path treats it
//!   as a regular LLM error → `ExecuteStatus::Failed`.
//! - Empty-response auto-retry (`empty_retry_count` path).
//! - Auto-continue detection (`looks_incomplete` / nudge path).
//! - WAL warm-up replay into LoopDetector (bootstrap owns that; execute receives
//!   the already-warmed detector via `BootstrapOutcome::loop_detector`).
//! - Thinking-block stripping from `IncomingMessage` directives. Content is passed
//!   to DB as-is; callers that need stripping should do it in finalize.

// Tasks 7-9 wire execute() into the real call-sites; allow dead_code until then.
#![allow(dead_code)]

use crate::agent::engine::AgentEngine;
use crate::agent::engine::LoopBreak;
use crate::agent::pipeline::bootstrap::BootstrapOutcome;
use crate::agent::pipeline::sink::{EventSink, PipelineEvent, SinkError};
use crate::agent::stream_event::StreamEvent;
use crate::agent::tool_executor::ToolExecutor as _;
use hydeclaw_types::{Message, MessageRole};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ── Outcome types ────────────────────────────────────────────────────────────

pub struct ExecuteOutcome {
    pub status: ExecuteStatus,
    pub session_id: Uuid,
    pub final_text: String,
    /// Thinking blocks from extended thinking (Anthropic only).
    pub thinking_json: Option<serde_json::Value>,
    pub messages_len_at_end: usize,
    /// Parent id for the final assistant message — tracks the end of the
    /// intermediate chain (last tool result or last intermediate assistant)
    /// so finalize can link the final reply correctly.
    pub final_parent_msg_id: Uuid,
}

#[derive(Debug)]
pub enum ExecuteStatus {
    Done,
    Failed(String),
    /// Execution stopped before finishing. Reason is a static label for logging.
    Interrupted(&'static str),
}

// ── Sink helpers ─────────────────────────────────────────────────────────────

/// Emit a `StreamEvent` into the sink, mapping `SinkError::Closed` to the
/// `Interrupted` shortcut via `?` using the sentinel `None` return.
///
/// Returns `Some(())` on success, `None` when the sink is closed (caller should
/// return `Interrupted`). Any other error is propagated with `?`.
macro_rules! emit_or_interrupted {
    ($sink:expr, $ev:expr, $outcome:expr) => {{
        match $sink.emit(PipelineEvent::Stream($ev)).await {
            Ok(()) => {}
            Err(SinkError::Closed) => return Ok($outcome),
            Err(e) => return Err(e.into()),
        }
    }};
}

// ── execute() ────────────────────────────────────────────────────────────────

/// Run the LLM+tools loop and stream results into `sink`.
///
/// Implements the safe subset of the `handle_sse` tool loop (see module doc).
/// Callers that need the full feature set (fallback provider, auto-continue,
/// session corruption recovery) should use `handle_sse` directly until Phase 66.
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
        mut loop_detector,
        processing_guard: _processing_guard, // Drop handles cleanup
        lifecycle_guard: _lifecycle_guard,
        enriched_text: _,
        command_output: _,
        user_message_id,
    } = bootstrap_outcome;

    // last_msg_id threads the DB parent chain through intermediate assistant
    // (with tool_calls) and tool-result messages so reload-from-active-path
    // can reconstruct the full turn, not just user → final assistant.
    let sm = crate::agent::session_manager::SessionManager::new(engine.cfg().db.clone());
    let agent_name = engine.cfg().agent.name.clone();
    let mut last_msg_id: uuid::Uuid = user_message_id;

    // Bail early if cancel was already signalled before we start.
    if cancel.is_cancelled() {
        return Ok(ExecuteOutcome {
            status: ExecuteStatus::Interrupted("cancel_token"),
            session_id,
            final_text: String::new(),
            thinking_json: None,
            messages_len_at_end: messages.len(),
            final_parent_msg_id: last_msg_id,
        });
    }

    // Signal the start of a message to the sink.
    let msg_id = format!("msg_{}", Uuid::new_v4());
    emit_or_interrupted!(
        sink,
        StreamEvent::MessageStart { message_id: msg_id },
        ExecuteOutcome {
            status: ExecuteStatus::Interrupted("sink_closed"),
            session_id,
            final_text: String::new(),
            thinking_json: None,
            messages_len_at_end: messages.len(),
            final_parent_msg_id: last_msg_id,
        }
    );

    // ── Mutable loop state ───────────────────────────────────────────────────
    let loop_config = engine.tool_loop_config();
    let mut final_text = String::new();
    let mut final_thinking_blocks: Vec<hydeclaw_types::ThinkingBlock> = vec![];
    let mut context_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
    let mut loop_nudge_count: usize = 0;

    // ── Turn loop ────────────────────────────────────────────────────────────
    for iteration in 0..loop_config.effective_max_iterations() {
        // 1. Check cancellation (graceful shutdown / SIGHUP drain)
        if cancel.is_cancelled() {
            tracing::info!(session = %session_id, "request cancelled — breaking tool loop");
            return Ok(ExecuteOutcome {
                status: ExecuteStatus::Interrupted("cancel_token"),
                session_id,
                final_text,
                thinking_json: None,
                messages_len_at_end: messages.len(),
                final_parent_msg_id: last_msg_id,
            });
        }

        // 2. Emit StepStart
        let step_id = format!("step_{}", iteration);
        match sink
            .emit(PipelineEvent::Stream(StreamEvent::StepStart {
                step_id: step_id.clone(),
            }))
            .await
        {
            Ok(()) => {}
            Err(SinkError::Closed) => {
                return Ok(ExecuteOutcome {
                    status: ExecuteStatus::Interrupted("sink_closed"),
                    session_id,
                    final_text,
                    thinking_json: None,
                    messages_len_at_end: messages.len(),
                    final_parent_msg_id: last_msg_id,
                });
            }
            Err(e) => return Err(e.into()),
        }

        // 3. Compact tool results to stay within context budget
        crate::agent::pipeline::context::compact_tool_results(
            &engine.cfg().agent.model,
            engine.cfg().agent.compaction.as_ref(),
            &mut messages,
            &mut context_chars,
        );

        // 4. Spawn forwarder — accumulates LLM text chunks and emits a single
        //    TextDelta once the call completes (batched approach from Task 6a).
        //    NOTE: The engine_sse.rs path streams each chunk individually via
        //    the EngineEventSender; we batch here since EventSink is not Clone.
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (partial_tx, partial_rx) = tokio::sync::oneshot::channel::<String>();
        tokio::spawn(async move {
            let mut buf = String::new();
            while let Some(chunk) = chunk_rx.recv().await {
                buf.push_str(&chunk);
            }
            let _ = partial_tx.send(buf);
        });

        // 5. Call LLM
        let provider = engine.cfg().provider.as_ref();
        let llm_result = crate::agent::pipeline::llm_call::chat_stream_with_transient_retry(
            provider,
            &mut messages,
            &tools,
            chunk_tx,
            engine,
        )
        .await;

        // Drain forwarder to get accumulated text
        let partial = partial_rx.await.unwrap_or_default();

        // 6. Handle LLM result
        //
        // Omitted from Task 6b:
        //   - Fallback provider switching (consecutive_failures threshold)
        //   - SessionCorruption recovery (did_reset_session + messages.retain)
        //
        // Both are handled by engine_sse.rs for the SSE call-site.
        let response = match llm_result {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, iteration, "pipeline LLM call failed");
                let reason = format!("LLM call failed: {e}");
                // Emit the error text as TextDelta so the UI shows it
                let user_msg = crate::agent::error_classify::format_user_error(&e);
                match sink
                    .emit(PipelineEvent::Stream(StreamEvent::TextDelta(user_msg.clone())))
                    .await
                {
                    Ok(()) | Err(SinkError::Closed) => {}
                    Err(e2) => return Err(e2.into()),
                }
                let _ = sink
                    .emit(PipelineEvent::Stream(StreamEvent::StepFinish {
                        step_id,
                        finish_reason: "error".into(),
                    }))
                    .await;
                return Ok(ExecuteOutcome {
                    status: ExecuteStatus::Failed(reason),
                    session_id,
                    final_text: user_msg,
                    thinking_json: None,
                    messages_len_at_end: messages.len(),
                    final_parent_msg_id: last_msg_id,
                });
            }
        };

        // Fire-and-forget usage recording (mirrors engine_sse.rs line 405)
        if let Some(ref usage) = response.usage {
            let db = engine.cfg().db.clone();
            let agent = engine.cfg().agent.name.clone();
            let provider_name = response.provider.clone()
                .unwrap_or_else(|| engine.cfg().provider.name().to_string());
            let model = response.model.clone().unwrap_or_default();
            let input = usage.input_tokens;
            let output = usage.output_tokens;
            tokio::spawn(async move {
                if let Err(e) = crate::db::usage::record_usage(
                    &db, &agent, &provider_name, &model, input, output, Some(session_id),
                )
                .await
                {
                    tracing::debug!(error = %e, "failed to record usage");
                }
            });
        }

        // 7. No tool calls → final text response
        if response.tool_calls.is_empty() {
            // Tokens were batched in `partial` — emit as TextDelta
            if !partial.is_empty() {
                match sink
                    .emit(PipelineEvent::Stream(StreamEvent::TextDelta(partial.clone())))
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
                        final_parent_msg_id: last_msg_id,
                        });
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            final_text = partial;
            final_thinking_blocks = response.thinking_blocks.clone();

            let _ = sink
                .emit(PipelineEvent::Stream(StreamEvent::StepFinish {
                    step_id,
                    finish_reason: "stop".into(),
                }))
                .await;

            // Emit Finish — this is the normal done path
            match sink
                .emit(PipelineEvent::Stream(StreamEvent::Finish {
                    finish_reason: "stop".into(),
                    continuation: false,
                }))
                .await
            {
                Ok(()) => {}
                Err(SinkError::Closed) => {
                    return Ok(ExecuteOutcome {
                        status: ExecuteStatus::Interrupted("sink_closed"),
                        session_id,
                        final_text,
                        thinking_json: None,
                        messages_len_at_end: messages.len(),
                        final_parent_msg_id: last_msg_id,
                    });
                }
                Err(e) => return Err(e.into()),
            }

            let thinking_json = if final_thinking_blocks.is_empty() {
                None
            } else {
                serde_json::to_value(&final_thinking_blocks).ok()
            };
            return Ok(ExecuteOutcome {
                status: ExecuteStatus::Done,
                session_id,
                final_text,
                thinking_json,
                messages_len_at_end: messages.len(),
                final_parent_msg_id: last_msg_id,
            });
        }

        // 8. Tool calls present — append assistant message to context
        tracing::info!(
            iteration,
            max = loop_config.effective_max_iterations(),
            tools = response.tool_calls.len(),
            "executing tool calls (pipeline)"
        );

        // Content already streamed via chunk forwarder. Push to context for LLM.
        messages.push(Message {
            role: MessageRole::Assistant,
            content: partial.clone(),
            tool_calls: Some(response.tool_calls.clone()),
            tool_call_id: None,
            thinking_blocks: vec![],
        });
        context_chars += partial.chars().count();

        // Persist the intermediate assistant (with tool_calls) to DB so
        // reload-from-active-path can reconstruct tool-use history.
        // Errors are logged but non-fatal — the in-memory context is already
        // correct, only DB replay degrades.
        let tc_json = serde_json::to_value(&response.tool_calls).ok();
        match sm
            .save_message_ex(
                session_id,
                "assistant",
                &partial,
                tc_json.as_ref(),
                None,
                Some(&agent_name),
                None,
                Some(last_msg_id),
            )
            .await
        {
            Ok(id) => last_msg_id = id,
            Err(e) => tracing::warn!(
                error = %e, session_id = %session_id,
                "failed to save intermediate assistant to DB"
            ),
        }

        // 9. Emit ToolCallStart + ToolCallArgs for each tool (UI feedback)
        for tc in &response.tool_calls {
            let _ = sink
                .emit(PipelineEvent::Stream(StreamEvent::ToolCallStart {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                }))
                .await;
            let args_text = serde_json::to_string(&tc.arguments).unwrap_or_default();
            let _ = sink
                .emit(PipelineEvent::Stream(StreamEvent::ToolCallArgs {
                    id: tc.id.clone(),
                    args_text,
                }))
                .await;
        }

        // 10. Execute tool batch via ToolExecutor (loop detection inside execute_batch)
        let tool_executor = engine
            .tool_executor
            .get()
            .expect("tool_executor not initialized");

        let loop_broken = match tool_executor
            .execute_batch(
                &response.tool_calls,
                &serde_json::Value::Null, // context — callers with msg can pass msg.context; Task 6b uses Null
                session_id,
                "",   // channel — not available here; callers with channel can override in Tasks 7-9
                messages.iter().map(|m| m.content.len()).sum(),
                &mut loop_detector,
                loop_config.detect_loops,
            )
            .await
        {
            Ok(results) => {
                for (tc_id, tool_result) in &results {
                    // Extract rich-card / __file__: markers and emit File/RichCard
                    // stream events; for plain text both halves are the raw string.
                    let ToolResultParts {
                        display_result,
                        db_result,
                    } = extract_tool_result_events(tool_result, sink).await;

                    let _ = sink
                        .emit(PipelineEvent::Stream(StreamEvent::ToolResult {
                            id: tc_id.clone(),
                            result: display_result.clone(),
                        }))
                        .await;

                    let display_len = display_result.chars().count();
                    messages.push(Message {
                        role: MessageRole::Tool,
                        content: display_result,
                        tool_calls: None,
                        tool_call_id: Some(tc_id.clone()),
                        thinking_blocks: vec![],
                    });
                    context_chars += display_len;

                    // Persist tool result to DB with raw markers preserved so
                    // reload-from-active-path can reinstate File/RichCard events.
                    match sm
                        .save_message_ex(
                            session_id,
                            "tool",
                            &db_result,
                            None,
                            Some(tc_id),
                            None,
                            None,
                            Some(last_msg_id),
                        )
                        .await
                    {
                        Ok(id) => last_msg_id = id,
                        Err(e) => tracing::warn!(
                            error = %e, session_id = %session_id,
                            "failed to save tool result to DB"
                        ),
                    }
                }
                false // loop continues
            }
            Err(LoopBreak(reason)) => {
                if loop_nudge_count < loop_config.max_loop_nudges {
                    // Inject nudge message and continue (mirrors engine_sse.rs lines 575-599)
                    messages.push(Message {
                        role: MessageRole::System,
                        content: build_loop_nudge_message(reason.as_deref()),
                        tool_calls: None,
                        tool_call_id: None,
                        thinking_blocks: vec![],
                    });
                    loop_nudge_count += 1;
                    loop_detector.reset();
                    tracing::warn!(
                        agent = %engine.cfg().agent.name,
                        nudge_count = loop_nudge_count,
                        reason = ?reason,
                        "loop nudge injected (pipeline path)"
                    );
                    false // continue — nudge was injected
                } else {
                    // Max nudges exhausted — treat as Failed
                    tracing::error!(
                        agent = %engine.cfg().agent.name,
                        nudge_count = loop_nudge_count,
                        "max loop nudges reached, force-stopping agent (pipeline path)"
                    );
                    true // broken
                }
            }
        };

        let _ = sink
            .emit(PipelineEvent::Stream(StreamEvent::StepFinish {
                step_id: step_id.clone(),
                finish_reason: "tool-calls".into(),
            }))
            .await;

        // Loop break after max nudges → terminate with Failed
        if loop_broken {
            let reason = "loop_detected_max_nudges".to_string();
            let _ = sink
                .emit(PipelineEvent::Stream(StreamEvent::Finish {
                    finish_reason: "loop_detected".into(),
                    continuation: false,
                }))
                .await;
            return Ok(ExecuteOutcome {
                status: ExecuteStatus::Failed(reason),
                session_id,
                final_text,
                thinking_json: None,
                messages_len_at_end: messages.len(),
                final_parent_msg_id: last_msg_id,
            });
        }
    }

    // ── Turn limit reached ────────────────────────────────────────────────────
    // All iterations exhausted without a clean stop. Emit Finish and return Done
    // with finish_reason = "turn_limit" (mirrors engine_sse.rs forced-final-call path,
    // but omits the extra LLM call — that optimization is Task 6b omitted scope).
    tracing::warn!(
        agent = %engine.cfg().agent.name,
        max = loop_config.effective_max_iterations(),
        "pipeline turn limit reached"
    );
    let _ = sink
        .emit(PipelineEvent::Stream(StreamEvent::Finish {
            finish_reason: "turn_limit".into(),
            continuation: false,
        }))
        .await;

    Ok(ExecuteOutcome {
        status: ExecuteStatus::Done,
        session_id,
        final_text,
        thinking_json: None,
        messages_len_at_end: messages.len(),
        final_parent_msg_id: last_msg_id,
    })
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Result of processing a tool result for SSE event emission.
///
/// `display_result` is what goes into the LLM context (markers stripped) and
/// the `ToolResult` event. `db_result` is preserved verbatim for DB storage
/// so reload/active_path rebuild can recover the full marker set.
struct ToolResultParts {
    display_result: String,
    db_result: String,
}

/// Extract inline markers (`__rich_card__:`, `__file__:`) from a tool result,
/// emit corresponding `StreamEvent`s via the sink, and return cleaned display
/// + raw DB text. Ported from the old `pipeline::entry::extract_tool_result_events`
/// so the pipeline path has parity with the deleted engine_sse.rs behaviour.
async fn extract_tool_result_events<S: EventSink>(
    tool_result: &str,
    sink: &mut S,
) -> ToolResultParts {
    use crate::agent::engine::{FILE_PREFIX, RICH_CARD_PREFIX};

    if let Some(json_str) = tool_result.strip_prefix(RICH_CARD_PREFIX) {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(json_str) {
            let card_type = data
                .get("card_type")
                .and_then(|v| v.as_str())
                .unwrap_or("table")
                .to_string();
            let _ = sink
                .emit(PipelineEvent::Stream(StreamEvent::RichCard {
                    card_type,
                    data,
                }))
                .await;
        }
        ToolResultParts {
            display_result: "Rich card displayed".to_string(),
            db_result: tool_result.to_string(),
        }
    } else if tool_result.contains(FILE_PREFIX) {
        let db_result = tool_result.to_string();
        let mut clean_lines: Vec<&str> = Vec::new();
        for line in tool_result.lines() {
            if let Some(json_str) = line.strip_prefix(FILE_PREFIX) {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(json_str) {
                    let url = meta.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let media_type = meta
                        .get("mediaType")
                        .and_then(|v| v.as_str())
                        .unwrap_or("application/octet-stream");
                    if !url.is_empty() {
                        let _ = sink
                            .emit(PipelineEvent::Stream(StreamEvent::File {
                                url: url.to_string(),
                                media_type: media_type.to_string(),
                            }))
                            .await;
                    }
                }
            } else {
                clean_lines.push(line);
            }
        }
        let text = clean_lines.join("\n");
        let display_result = if text.is_empty() {
            "Image displayed inline in the chat. Do NOT use canvas or other tools to show it again.".to_string()
        } else {
            text
        };
        ToolResultParts {
            display_result,
            db_result,
        }
    } else {
        ToolResultParts {
            display_result: tool_result.to_string(),
            db_result: tool_result.to_string(),
        }
    }
}

/// Build the system nudge message injected when a tool-call loop is detected.
fn build_loop_nudge_message(reason: Option<&str>) -> String {
    let nudge_desc = reason.unwrap_or("repeating pattern");
    format!(
        "LOOP DETECTED: You have repeated the same sequence of actions ({desc}). \
         Change your approach entirely. If the task is too large for a single session, \
         tell the user and suggest breaking it into smaller steps. Do NOT retry the same approach.",
        desc = nudge_desc
    )
}

// No inline #[cfg(test)] module for Task 6a/6b. Tests require a live
// AgentEngine which is architecturally blocked from inline unit tests
// (see lib.rs 10-module cap, spec §1). Coverage via Task 12 smoke test
// and future CI integration tests once Phase 66 REF-01 exposes a test
// surface for AgentEngine.
