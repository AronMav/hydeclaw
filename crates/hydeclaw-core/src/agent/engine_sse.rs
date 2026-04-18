//! SSE streaming execution: handle_sse.
//! Extracted from engine_execution.rs for readability.

use super::*;
use crate::agent::pipeline::execution as exec_helpers;
use crate::agent::pipeline::entry as entry_helpers;
use crate::agent::tool_executor::ToolExecutor;

impl AgentEngine {
    /// Handle message via SSE: emits StreamEvents for AI SDK UI Message Stream Protocol v1.
    /// Supports tool execution, session continuation, and real-time status updates.
    ///
    /// Phase 62 RES-01: `event_tx` is an `EngineEventSender` wrapping a bounded
    /// `mpsc::Sender<StreamEvent>` (capacity 256 in chat.rs). TextDelta uses
    /// `try_send` (droppable per CONTEXT.md); all other variants either use
    /// the synchronous `.send(ev)` entry (which surfaces `FullNonText` on
    /// backpressure so the caller can log/retry) or `.send_async(ev).await`
    /// (which awaits a free slot and only errors on closed channel).
    pub async fn handle_sse(
        &self,
        msg: &IncomingMessage,
        event_tx: crate::agent::engine_event_sender::EngineEventSender,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<Uuid> {
        // Hook: BeforeMessage
        if let crate::agent::hooks::HookAction::Block(reason) = self.hooks().fire(&crate::agent::hooks::HookEvent::BeforeMessage) {
            anyhow::bail!("blocked by hook: {}", reason);
        }

        // Track active request for graceful shutdown/SIGHUP drain
        let cancel_guard = Some(self.state.register_request());

        // Handle slash commands (no LLM needed)
        let user_text = msg.text.clone().unwrap_or_default();
        if let Some(result) = self.handle_command(&user_text, msg).await {
            let text = result?;
            let msg_id_str = format!("msg_{}", Uuid::new_v4());
            // MessageStart is non-text, MUST be delivered (the client needs the
            // message_id to correlate parts). send_async honors the
            // EngineEventSender non-text-never-dropped contract.
            if event_tx
                .send_async(StreamEvent::MessageStart { message_id: msg_id_str })
                .await
                .is_err()
            {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }
            if event_tx.send(StreamEvent::TextDelta(text.clone())).is_err() {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }
            // Finish is a terminal event — must always reach the client.
            if event_tx
                .send_async(StreamEvent::Finish {
                    finish_reason: "command".to_string(),
                    continuation: false,
                })
                .await
                .is_err()
            {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }

            // Save command response to DB for branching consistency
            let sid = if let Some(sid) = resume_session_id {
                sid
            } else {
                crate::db::sessions::get_or_create_session(&self.cfg().db, &self.cfg().agent.name, &msg.user_id, &msg.channel, self.cfg().agent.session.as_ref().map(|s| s.dm_scope.as_str()).unwrap_or("default")).await?
            };
            let sm = SessionManager::new(self.cfg().db.clone());
            let u_msg_id = sm.save_message_ex(sid, "user", &user_text, None, None, None, None, msg.leaf_message_id).await?;
            let a_msg_id = sm.save_message_ex(sid, "assistant", &text, None, None, Some(&self.cfg().agent.name), None, Some(u_msg_id)).await?;
            // Mark slash-command sessions as done (they skip the LLM lifecycle guard)
            let _ = sm.set_run_status(sid, "done").await;
            if let Some((ref id, _)) = cancel_guard {
                self.state.unregister_request(id);
            }
            return Ok(a_msg_id);
        }

        let thinking_level = self.state().thinking_level.load(std::sync::atomic::Ordering::Relaxed);

        // Branch-aware context: when leaf_message_id is set (from frontend),
        // build_context uses load_branch_messages instead of flat load_messages.
        let crate::agent::context_builder::ContextSnapshot { session_id, mut messages, tools: available_tools } =
            self.build_context(msg, true, resume_session_id, force_new_session).await?;

        // Store event_tx so subagent handlers can emit SSE events (e.g., subagent-complete RichCard)
        *self.sse_event_tx().lock().await = Some(event_tx.clone());

        // Lifecycle tracking: mark running, RAII guard marks 'failed' on early exit
        let sm = SessionManager::new(self.cfg().db.clone());
        if let Err(e) = sm.set_run_status(session_id, "running").await {
            tracing::warn!(session_id = %session_id, error = %e, "failed to mark SSE session as running");
        }
        exec_helpers::log_wal_running_with_retry(&sm, session_id).await;
        let mut lifecycle_guard = SessionLifecycleGuard::new(self.cfg().db.clone(), session_id);

        // Emit session ID so the UI can track which session is active
        // SessionId is the identifier the UI uses to track streams — non-text,
        // MUST be delivered. Use send_async per the non-drop contract.
        if event_tx
            .send_async(StreamEvent::SessionId(session_id.to_string()))
            .await
            .is_err()
        {
            tracing::debug!("SSE event channel closed, engine continues for DB save");
        }

        // Broadcast processing start + guard broadcasts end on drop
        let start_event = serde_json::json!({
            "type": "agent_processing",
            "agent": self.cfg().agent.name,
            "session_id": session_id.to_string(),
            "status": "start",
            "channel": msg.channel,
        });
        self.broadcast_ui_event(start_event.clone());
        let _processing_guard = ProcessingGuard::new(
            self.state().ui_event_tx.clone(),
            self.state().processing_tracker.clone(),
            self.cfg().agent.name.clone(),
            &start_event,
        );

        // Add current message, auto-fetch URLs if present
        let enriched_text = {
            let toolgate_url = self.cfg().app_config.toolgate_url.clone()
                .unwrap_or_else(|| "http://localhost:9011".to_string());
            crate::agent::pipeline::subagent::enrich_message_text(
                self.http_client(),
                self.ssrf_http_client(),
                &self.cfg().app_config.gateway.listen,
                &toolgate_url,
                &self.cfg().agent.language,
                &user_text,
                &msg.attachments,
            ).await
        };

        messages.push(Message {
            role: MessageRole::User,
            content: enriched_text,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        });

        let sender_agent_id = exec_helpers::extract_sender_agent_id(&msg.user_id);
        let user_msg_id = sm.save_message_ex(session_id, "user", &user_text, None, None, sender_agent_id, None, msg.leaf_message_id).await?;
        let mut last_msg_id = user_msg_id;

        // Context compaction if needed (model-aware token budget)
        self.compact_messages(&mut messages, None).await;

        // Emit message start — non-text, MUST reach the client so parts can
        // be correlated. send_async honors EngineEventSender non-drop contract.
        let message_id = format!("msg_{}", Uuid::new_v4());
        if event_tx
            .send_async(StreamEvent::MessageStart {
                message_id: message_id.clone(),
            })
            .await
            .is_err()
        {
            tracing::debug!("SSE event channel closed, engine continues for DB save");
        }

        // LLM loop with tool calls
        let mut final_response = String::new();
        let mut final_thinking_blocks: Vec<hydeclaw_types::ThinkingBlock> = vec![];
        let loop_config = self.tool_loop_config();
        let mut detector = LoopDetector::new(&loop_config);
        // WAL warm-up: replay tool events from previous runs into LoopDetector (BUG-026)
        if let Ok(events) = crate::db::session_wal::load_tool_events(&self.cfg().db, session_id).await {
            for ev in &events {
                detector.record_result_from_wal(&ev.tool_name, ev.success);
            }
            if !events.is_empty() {
                tracing::debug!(session_id = %session_id, count = events.len(), "WAL warm-up: replayed tool events into LoopDetector");
            }
        }
        let mut loop_nudge_count: usize = 0;
        let mut did_reset_session = false;
        let mut empty_retry_count: u8 = 0;
        let mut auto_continue_count: u8 = 0;
        let mut context_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        let mut consecutive_failures: usize = 0;
        let mut using_fallback = false;
        let mut fallback_provider: Option<Arc<dyn crate::agent::providers::LlmProvider>> = None;

        for iteration in 0..loop_config.effective_max_iterations() {
            // Check cancellation (graceful shutdown / SIGHUP drain)
            if let Some((_, ref token)) = cancel_guard
                && token.is_cancelled()
            {
                tracing::info!(session = %session_id, "request cancelled — breaking tool loop for graceful shutdown");
                break;
            }

            let step_id = format!("step_{}", iteration);
            if event_tx
                .send(StreamEvent::StepStart {
                    step_id: step_id.clone(),
                })
                .is_err()
            {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }

            self.compact_tool_results(&mut messages, &mut context_chars);
            // Per-iteration streaming channel: forwards LLM token chunks to SSE event stream.
            // chat_stream sends tokens only for text responses (not tool-call responses).
            let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<String>();
            let event_tx_fwd = event_tx.clone();
            tokio::spawn(async move {
                while let Some(chunk) = chunk_rx.recv().await {
                    // TextDelta uses try_send under the hood (droppable per
                    // CONTEXT.md). Err here means either the channel closed
                    // OR the bounded buffer filled faster than the coalescer
                    // drains; in both cases the drop counter is recorded by
                    // the coalescer when it next writes to the outer.
                    if event_tx_fwd.send(StreamEvent::TextDelta(chunk)).is_err() {
                        tracing::debug!("SSE forwarder: event channel closed or full");
                    }
                }
            });
            let llm_result = if let Some(ref fb) = fallback_provider {
                self.chat_stream_with_transient_retry_using(fb, &mut messages, &available_tools, chunk_tx).await
            } else {
                self.chat_stream_with_transient_retry(&mut messages, &available_tools, chunk_tx).await
            };
            let response = match llm_result {
                Ok(r) => {
                    consecutive_failures = 0;
                    r
                }
                Err(e) => {
                    if error_classify::classify(&e) == error_classify::LlmErrorClass::SessionCorruption && !did_reset_session {
                        did_reset_session = true;
                        tracing::warn!(error = %e, "SSE session corrupted, resetting context");
                        messages.retain(|m| m.role == MessageRole::System);
                        messages.push(Message { role: MessageRole::User, content: user_text.clone(), tool_calls: None, tool_call_id: None, thinking_blocks: vec![] });
                        context_chars = messages.iter().map(|m| m.content.chars().count()).sum();
                        continue;
                    }
                    consecutive_failures += 1;
                    if !using_fallback && consecutive_failures >= loop_config.max_consecutive_failures {
                        if fallback_provider.is_none() {
                            fallback_provider = self.create_fallback_provider().await;
                        }
                        if fallback_provider.is_some() {
                            // Issue D: the engine-level fallback-provider switch
                            // is distinct from RoutingProvider failover. Before
                            // switching to the fallback, persist any partial_text
                            // carried by the dying call's typed LlmCallError (and
                            // record an aborted_failover usage row) so work
                            // produced by the primary isn't lost when we re-ask
                            // the fallback.
                            if let Some(partial_id) = persist_partial_if_any(
                                &self.cfg().db,
                                session_id,
                                &self.cfg().agent.name,
                                last_msg_id,
                                &e,
                            )
                            .await
                            {
                                last_msg_id = partial_id;
                            }
                            record_llm_timeout_if_typed(&e);
                            // Engine-level fallback-switch: we ARE failing over to a
                            // sibling provider regardless of `is_failover_worthy()`.
                            // Label explicitly so mid-stream SchemaError / AuthError /
                            // other non-failover-worthy errors don't get mislabeled
                            // as `aborted` when the switch actually occurred.
                            record_aborted_usage(
                                &self.cfg().db,
                                &self.cfg().agent.name,
                                self.cfg().provider.name(),
                                self.cfg().agent.model.as_str(),
                                session_id,
                                &e,
                                UsageAbortStatus::AbortedFailover,
                            )
                            .await;
                            using_fallback = true;
                            consecutive_failures = 0;
                            tracing::warn!(
                                agent = %self.cfg().agent.name,
                                iteration,
                                "switching to fallback provider after consecutive failures (SSE)"
                            );
                            // StepFinish is a tool-cycle marker — non-text, use send_async.
                            if event_tx
                                .send_async(StreamEvent::StepFinish {
                                    step_id,
                                    finish_reason: "fallback".into(),
                                })
                                .await
                                .is_err()
                            {
                                tracing::debug!("SSE event channel closed, engine continues for DB save");
                            }
                            continue;
                        }
                    }
                    // AUDIT:SSE-02 (verified 2026-03-30): LLM errors mid-stream are delivered
                    // as TextDelta (not StreamEvent::Error) via format_user_error(). This is
                    // intentional: the error appears inline in chat history so the user sees it
                    // as a visible message. The engine then sends Finish, ensuring the SSE stream
                    // terminates cleanly. StreamEvent::Error is reserved for top-level handle_sse
                    // failures (caught in chat.rs spawned task).
                    tracing::error!(error = %e, iteration, "SSE LLM call failed, returning fallback");
                    // Task 19: persist partial_text from cancel-class LlmCallError before
                    // surfacing the error, so cancellation never loses work already
                    // produced. Only variants whose `partial_text()` returns a non-empty
                    // string are persisted (ConnectTimeout/RequestTimeout carry none).
                    //
                    // Issue #7: when a partial row is written, chain the subsequent
                    // assistant-error message to it (not to `last_msg_id`) so the
                    // two rows are linear under m012 branching (user → partial →
                    // error) rather than siblings of the user message.
                    if let Some(partial_id) = persist_partial_if_any(
                        &self.cfg().db,
                        session_id,
                        &self.cfg().agent.name,
                        last_msg_id,
                        &e,
                    )
                    .await
                    {
                        last_msg_id = partial_id;
                    }
                    // Always bump `llm_timeout_total` for the four timeout variants,
                    // even on the single-route path that bypasses RoutingProvider.
                    record_llm_timeout_if_typed(&e);
                    // Issue B: record aborted/aborted_failover usage row so the
                    // STATUS_ABORTED* constants have runtime writers (spec §4.6).
                    //
                    // Provider name: if we are currently using the engine-level
                    // fallback provider, use its real name (preserves per-provider
                    // dashboards) rather than the literal "fallback" placeholder.
                    //
                    // Status: the final-error path either bubbles the call up
                    // (no failover) or we already took the fallback-switch branch
                    // above. Here we classify based on whether the error itself
                    // is failover-worthy AND carried partial_text — consistent
                    // with the previous heuristic for the non-switch path.
                    let active_provider_name: &str = if using_fallback {
                        fallback_provider
                            .as_ref()
                            .map(|p| p.name())
                            .unwrap_or("fallback")
                    } else {
                        self.cfg().provider.name()
                    };
                    let abort_status = if e
                        .downcast_ref::<crate::agent::providers::LlmCallError>()
                        .is_some_and(|err| {
                            err.is_failover_worthy()
                                && !err.partial_text().unwrap_or("").is_empty()
                        })
                    {
                        UsageAbortStatus::AbortedFailover
                    } else {
                        UsageAbortStatus::Aborted
                    };
                    record_aborted_usage(
                        &self.cfg().db,
                        &self.cfg().agent.name,
                        active_provider_name,
                        self.cfg().agent.model.as_str(),
                        session_id,
                        &e,
                        abort_status,
                    )
                    .await;
                    let fallback = error_classify::format_user_error(&e);
                    if event_tx.send(StreamEvent::TextDelta(fallback.clone())).is_err() {
                        tracing::debug!("SSE event channel closed, engine continues for DB save");
                    }
                    final_response = fallback;
                    if event_tx
                        .send_async(StreamEvent::StepFinish {
                            step_id,
                            finish_reason: "error".into(),
                        })
                        .await
                        .is_err()
                    {
                        tracing::debug!("SSE event channel closed, engine continues for DB save");
                    }
                    let reason_str = format!("SSE LLM call failed: {e}");
                    lifecycle_guard.fail(&reason_str).await;
                    exec_helpers::notify_agent_error(
                        self.cfg().db.clone(),
                        self.state().ui_event_tx.as_ref(),
                        &self.cfg().agent.name,
                        &reason_str,
                    );
                    break;
                }
            };
            self.record_usage(&response, Some(session_id));

            if response.tool_calls.is_empty() {
                // Final text response — tokens already streamed via chunk_tx forwarder.
                // Only strip thinking for DB save; do NOT re-send as TextDelta.
                final_response = maybe_strip_thinking(&response.content, msg, thinking_level);
                final_thinking_blocks = response.thinking_blocks.clone();

                // Auto-continue: if LLM described remaining work, nudge it to execute.
                // In SSE mode, the "incomplete" text was already streamed — send visible marker.
                if auto_continue_count < loop_config.max_auto_continues && !final_response.is_empty() && looks_incomplete(&final_response) {
                    auto_continue_count += 1;
                    tracing::info!(iteration, count = auto_continue_count, max = loop_config.max_auto_continues, "auto-continue: response looks incomplete, nudging LLM");
                    exec_helpers::notify_auto_continue(
                        self.cfg().db.clone(),
                        self.state().ui_event_tx.as_ref(),
                        &self.cfg().agent.name,
                        auto_continue_count,
                        loop_config.max_auto_continues,
                    );
                    // Auto-continue markers are non-text and MUST be delivered so
                    // the UI can render a continuation separator correctly.
                    let _ = event_tx
                        .send_async(StreamEvent::StepFinish {
                            step_id: step_id.clone(),
                            finish_reason: "continuation".into(),
                        })
                        .await;
                    let _ = event_tx
                        .send_async(StreamEvent::Finish {
                            finish_reason: "continuation".into(),
                            continuation: true,
                        })
                        .await;
                    let _ = event_tx.send(StreamEvent::TextDelta("\n\n...".to_string()));
                    messages.push(Message {
                        role: MessageRole::User,
                        content: AUTO_CONTINUE_NUDGE.to_string(),
                        tool_calls: None,
                        tool_call_id: None,
                        thinking_blocks: vec![],
                    });
                    context_chars += AUTO_CONTINUE_NUDGE.len(); // all ASCII
                    continue;
                }

                if final_response.is_empty() && empty_retry_count < 1 {
                    empty_retry_count += 1;
                    tracing::warn!(iteration, "LLM returned empty response, retrying once");
                    continue;
                }
                if final_response.is_empty() {
                    tracing::warn!(iteration, "LLM returned empty response after retry");
                }
                if event_tx
                    .send(StreamEvent::StepFinish {
                        step_id,
                        finish_reason: "stop".into(),
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }
                break;
            }

            tracing::info!(
                iteration,
                max = loop_config.effective_max_iterations(),
                tools = response.tool_calls.len(),
                "executing tool calls (SSE)"
            );

            // Strip <think> blocks for DB save and LLM context.
            // NOTE: content was already streamed token-by-token via chunk_tx forwarder above;
            // do NOT re-send as TextDelta here — it would duplicate text in the UI.
            let cleaned_content = maybe_strip_thinking(&response.content, msg, thinking_level);

            messages.push(Message {
                role: MessageRole::Assistant,
                content: cleaned_content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
                thinking_blocks: vec![],
            });
            context_chars += cleaned_content.chars().count();

            let tc_json = serde_json::to_value(&response.tool_calls).ok();
            match sm.save_message_ex(
                session_id,
                "assistant",
                &cleaned_content,
                tc_json.as_ref(),
                None,
                Some(&self.cfg().agent.name),
                None,
                Some(last_msg_id),
            )
            .await {
                Ok(id) => { last_msg_id = id; }
                Err(e) => { tracing::warn!(error = %e, session_id = %session_id, "failed to save assistant message to DB"); }
            }

            // Emit ToolCallStart/ToolCallArgs for ALL tools before executing
            for tc in &response.tool_calls {
                if event_tx
                    .send(StreamEvent::ToolCallStart {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }
                let args_text = serde_json::to_string(&tc.arguments).unwrap_or_default();
                if event_tx
                    .send(StreamEvent::ToolCallArgs {
                        id: tc.id.clone(),
                        args_text,
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }
            }

            let loop_broken = match self.tool_executor
                .get()
                .expect("tool_executor not initialized")
                .execute_batch(
                    &response.tool_calls, &msg.context, session_id, &msg.channel,
                    messages.iter().map(|m| m.content.len()).sum(),
                    &mut detector, loop_config.detect_loops,
                ).await {
                Ok(results) => {
                    for (tc_id, tool_result) in &results {
                        // Extract RichCard / File markers for SSE events
                        let entry_helpers::ToolResultParts { display_result, db_result } =
                            entry_helpers::extract_tool_result_events(tool_result, &event_tx);

                        if event_tx
                            .send(StreamEvent::ToolResult {
                                id: tc_id.clone(),
                                result: display_result.clone(),
                            })
                            .is_err()
                        {
                            tracing::debug!("SSE event channel closed, engine continues for DB save");
                        }

                        let display_len = display_result.chars().count();
                        messages.push(Message {
                            role: MessageRole::Tool,
                            content: display_result,
                            tool_calls: None,
                            tool_call_id: Some(tc_id.clone()),
                            thinking_blocks: vec![],
                        });
                        context_chars += display_len;

                        match sm.save_message_ex(
                            session_id, "tool", &db_result, None, Some(tc_id), None, None, Some(last_msg_id),
                        ).await {
                            Ok(id) => { last_msg_id = id; }
                            Err(e) => { tracing::warn!(error = %e, session_id = %session_id, "failed to save tool result to DB"); }
                        }
                    }
                    false
                }
                Err(LoopBreak(reason)) => {
                    if loop_nudge_count < loop_config.max_loop_nudges {
                        messages.push(Message {
                            role: MessageRole::System,
                            content: exec_helpers::build_loop_nudge_message(reason.as_deref()),
                            tool_calls: None,
                            tool_call_id: None,
                            thinking_blocks: vec![],
                        });
                        loop_nudge_count += 1;
                        detector.reset();
                        tracing::warn!(
                            agent = %self.cfg().agent.name,
                            nudge_count = loop_nudge_count,
                            reason = ?reason,
                            "loop nudge injected (SSE path)"
                        );
                        false // continue loop
                    } else {
                        tracing::error!(
                            agent = %self.cfg().agent.name,
                            nudge_count = loop_nudge_count,
                            "max loop nudges reached, force-stopping agent (SSE path)"
                        );
                        true // broken
                    }
                }
            };

            if event_tx
                .send(StreamEvent::StepFinish {
                    step_id,
                    finish_reason: "tool-calls".into(),
                })
                .is_err()
            {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }

            // Forced final call on last iteration or loop break
            if loop_broken || iteration == loop_config.effective_max_iterations() - 1 {
                // Notify if hitting iteration limit (not loop break)
                if !loop_broken && iteration == loop_config.effective_max_iterations() - 1 {
                    exec_helpers::notify_iteration_limit(
                        self.cfg().db.clone(),
                        self.state().ui_event_tx.as_ref(),
                        &self.cfg().agent.name,
                        loop_config.effective_max_iterations(),
                    );
                }
                // Notify if loop was broken after max nudges
                if loop_broken && loop_nudge_count >= loop_config.max_loop_nudges {
                    exec_helpers::notify_loop_detected(
                        self.cfg().db.clone(),
                        self.state().ui_event_tx.as_ref(),
                        &self.cfg().agent.name,
                        session_id,
                    );
                }
                let step_id = format!("step_{}", iteration + 1);
                if event_tx
                    .send(StreamEvent::StepStart {
                        step_id: step_id.clone(),
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }

                match self.cfg().provider.chat(&messages, &[]).await {
                    Ok(forced) => {
                        self.record_usage(&forced, Some(session_id));
                        let text = maybe_strip_thinking(&forced.content, msg, thinking_level);
                        if !text.is_empty()
                            && event_tx.send(StreamEvent::TextDelta(text.clone())).is_err() {
                                tracing::debug!("SSE event channel closed, engine continues for DB save");
                            }
                        final_response = text;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "SSE forced final LLM call failed");
                        let fallback = error_classify::format_user_error(&e);
                        if event_tx.send(StreamEvent::TextDelta(fallback.clone())).is_err() {
                            tracing::debug!("SSE event channel closed, engine continues for DB save");
                        }
                        final_response = fallback;
                        let reason_str = format!("SSE forced final LLM call failed: {e}");
                        lifecycle_guard.fail(&reason_str).await;
                        exec_helpers::notify_agent_error(
                            self.cfg().db.clone(),
                            self.state().ui_event_tx.as_ref(),
                            &self.cfg().agent.name,
                            &reason_str,
                        );
                    }
                }
                if event_tx
                    .send(StreamEvent::StepFinish {
                        step_id,
                        finish_reason: "stop".into(),
                    })
                    .is_err()
                {
                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                }
                break;
            }
        }

        // Save final response with agent_id for multi-agent identity
        let thinking_json = if final_thinking_blocks.is_empty() {
            None
        } else {
            serde_json::to_value(&final_thinking_blocks).ok()
        };
        let assistant_msg_id = sm.save_message_ex(session_id, "assistant", &final_response, None, None, Some(&self.cfg().agent.name), thinking_json.as_ref(), Some(last_msg_id))
            .await?;

        self.maybe_trim_session(session_id).await;

        if event_tx
            .send(StreamEvent::Finish {
                finish_reason: "stop".into(),
                continuation: false,
            })
            .is_err()
        {
            tracing::debug!("SSE event channel closed, engine continues for DB save");
        }

        lifecycle_guard.done().await;

        // Post-session knowledge extraction (background, non-blocking)
        exec_helpers::spawn_knowledge_extraction(
            self.cfg().db.clone(), session_id, self.cfg().agent.name.clone(),
            self.cfg().provider.clone(), self.cfg().memory_store.clone(), messages.len(),
        );

        // Clear SSE event sender
        *self.sse_event_tx().lock().await = None;

        // Unregister active request (cancel/drain tracking)
        if let Some((ref id, _)) = cancel_guard {
            self.state.unregister_request(id);
        }

        Ok(assistant_msg_id)
    }
}

/// Task 19: persist `partial_text` from a cancel-class `LlmCallError` before
/// the error is surfaced, so cancellation never loses work already produced.
///
/// Behavior:
/// * Downcasts `e` to `LlmCallError`. If the downcast fails, returns `None`.
/// * Only persists when `partial_text()` returns `Some` AND the string is
///   non-empty. Variants without partial text (ConnectTimeout, RequestTimeout,
///   Network, Server5xx, SchemaError, AuthError) return `None` — those are
///   failover-worthy and the next route produces the final content.
/// * `abort_reason()` supplies the stable short identifier written to
///   `messages.abort_reason` (see migration 024).
/// * DB insert failures are logged but do not mask the original LLM error —
///   the caller still bubbles `e` up unchanged, and this helper returns
///   `None` in that case.
///
/// Returns `Some(partial_message_id)` when a row was successfully inserted so
/// the caller can thread it into subsequent `save_message_ex` calls as the
/// parent — this prevents the partial + the follow-up error message from
/// becoming siblings of the user message under m012 branching (issue #7).
///
/// Shared by the SSE path (`engine_sse::handle_sse`) and the non-SSE path
/// (`engine_execution::handle_with_status`).
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
            // Persistence failure must never mask the LLM error. Log and drop.
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
/// entirely, so the counter was previously only populated when failover fired
/// — this helper closes that observability gap.
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

/// Explicit status for `record_aborted_usage`. Callers declare intent
/// rather than having the helper infer from the error's failover-worthy
/// flag — the engine-level fallback-switch path is an actual failover
/// for ANY error class (including non-failover-worthy ones like
/// mid-stream SchemaError and AuthError), so the previous inference
/// could mislabel those rows as plain `aborted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UsageAbortStatus {
    /// Call was terminated without attempting a sibling provider.
    /// Examples: user cancel, shutdown drain, MaxDurationExceeded.
    Aborted,
    /// Call was terminated AND a sibling provider (route or engine-level
    /// fallback) was attempted. Examples: routing failover, engine-level
    /// fallback-switch after `consecutive_failures >= max`.
    AbortedFailover,
}

/// Issue B: record an `aborted` / `aborted_failover` row in `usage_log` when
/// an LLM call is terminated before it could complete naturally (spec §4.6).
///
/// Caller declares `status` explicitly to disambiguate the two scenarios
/// that previous heuristic conflated:
///
/// - `Aborted` — single-path final error. The fix to classify as failover
///   or not is the caller's responsibility (e.g. the final-error branch
///   in `handle_sse` uses `llm_err.is_failover_worthy() && !partial.is_empty()`
///   to choose between the two values).
/// - `AbortedFailover` — engine-level fallback-switch branches or routing
///   failover call sites. These always qualify regardless of
///   `is_failover_worthy()`.
///
/// Token count is estimated as `partial_text.len() / 4` (rough bytes-per-token
/// heuristic for pre-tokenizer failures). Input tokens are left as 0 since the
/// call was aborted before usage headers arrived.
///
/// The write is fire-and-forget — DB failures are logged at debug level so
/// they never mask the caller's LLM error handling path.
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
    //! Regression guards for code review 2026-04-17. Terminal/identifier
    //! StreamEvents on the bounded SSE path MUST use `send_async` — the sync
    //! `send()` silently drops on Full, breaking the UI's ability to
    //! correlate message ids, session ids, and stream completion. TextDelta
    //! is the ONLY variant allowed to drop (covered by Phase 62 RES-01).
    use std::path::Path;

    fn source() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/agent/engine_sse.rs");
        std::fs::read_to_string(&path).expect("read engine_sse.rs")
    }

    #[test]
    fn slash_command_message_start_uses_send_async() {
        let src = source();
        // The slash-command early-return path must deliver MessageStart reliably.
        assert!(
            src.contains("send_async(StreamEvent::MessageStart"),
            "slash-command MessageStart must use send_async"
        );
    }

    #[test]
    fn slash_command_finish_uses_send_async() {
        let src = source();
        assert!(
            src.contains(r#"send_async(StreamEvent::Finish {
                    finish_reason: "command".to_string()"#),
            "slash-command Finish must use send_async"
        );
    }

    #[test]
    fn session_id_uses_send_async() {
        let src = source();
        assert!(
            src.contains("send_async(StreamEvent::SessionId"),
            "SessionId must use send_async (UI correlation depends on it)"
        );
    }
}
