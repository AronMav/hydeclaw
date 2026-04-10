//! SSE streaming execution: handle_sse.
//! Extracted from engine_execution.rs for readability.

use super::*;
use crate::agent::tool_executor::ToolExecutor;

impl AgentEngine {
    /// Handle message via SSE: emits StreamEvents for AI SDK UI Message Stream Protocol v1.
    /// Supports tool execution, session continuation, and real-time status updates.
    pub async fn handle_sse(
        &self,
        msg: &IncomingMessage,
        event_tx: mpsc::UnboundedSender<StreamEvent>,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<()> {
        let _chat_guard = crate::graph_worker::ChatActiveGuard::new();

        // Hook: BeforeMessage
        if let crate::agent::hooks::HookAction::Block(reason) = self.hooks().fire(&crate::agent::hooks::HookEvent::BeforeMessage) {
            anyhow::bail!("blocked by hook: {}", reason);
        }

        // Handle slash commands (no LLM needed)
        let user_text = msg.text.clone().unwrap_or_default();
        if let Some(result) = self.handle_command(&user_text, msg).await {
            let text = result?;
            let msg_id = format!("msg_{}", Uuid::new_v4());
            if event_tx.send(StreamEvent::MessageStart { message_id: msg_id }).is_err() {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }
            if event_tx.send(StreamEvent::TextDelta(text)).is_err() {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }
            if event_tx.send(StreamEvent::Finish { finish_reason: "command".to_string(), continuation: false }).is_err() {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }
            return Ok(());
        }

        let thinking_level = self.thinking_level.load(std::sync::atomic::Ordering::Relaxed);

        // TODO(branching): When frontend sends leaf_message_id, use load_branch_messages
        // instead of load_messages in build_context to provide branch-aware LLM context.
        let crate::agent::context_builder::ContextSnapshot { session_id, mut messages, tools: available_tools } =
            self.build_context(msg, true, resume_session_id, force_new_session).await?;

        // Store session_id for tool handlers that need session context (e.g., handoff)
        *self.processing_session_id().lock().await = Some(session_id);
        // Store event_tx so subagent handlers can emit SSE events (e.g., subagent-complete RichCard)
        *self.sse_event_tx().lock().await = Some(event_tx.clone());

        // Lifecycle tracking: mark running, RAII guard marks 'failed' on early exit
        let sm = SessionManager::new(self.db.clone());
        if let Err(e) = sm.set_run_status(session_id, "running").await {
            tracing::warn!(session_id = %session_id, error = %e, "failed to mark SSE session as running");
        }
        let mut lifecycle_guard = SessionLifecycleGuard::new(self.db.clone(), session_id);

        // Emit session ID so the UI can track which session is active
        if event_tx.send(StreamEvent::SessionId(session_id.to_string())).is_err() {
            tracing::debug!("SSE event channel closed, engine continues for DB save");
        }

        // Broadcast processing start + guard broadcasts end on drop
        let start_event = serde_json::json!({
            "type": "agent_processing",
            "agent": self.agent.name,
            "session_id": session_id.to_string(),
            "status": "start",
            "channel": msg.channel,
        });
        self.broadcast_ui_event(start_event.clone());
        let _processing_guard = ProcessingGuard::new(
            self.ui_event_tx.clone(),
            self.processing_tracker.clone(),
            self.agent.name.clone(),
            &start_event,
        );

        // Add current message, auto-fetch URLs if present
        let enriched_text = self.enrich_message_text(&user_text, &msg.attachments).await;

        messages.push(Message {
            role: MessageRole::User,
            content: enriched_text,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        });

        // For inter-agent messages (user_id starts with "agent:"), save the sender agent_id
        let sender_agent_id = if msg.user_id.starts_with("agent:") { Some(msg.user_id.trim_start_matches("agent:")) } else { None };
        sm.save_message_ex(session_id, "user", &user_text, None, None, sender_agent_id, None).await?;

        // Context compaction if needed (model-aware token budget)
        self.compact_messages(&mut messages, None).await;

        // Emit message start
        let message_id = format!("msg_{}", Uuid::new_v4());
        if event_tx
            .send(StreamEvent::MessageStart {
                message_id: message_id.clone(),
            })
            .is_err()
        {
            tracing::debug!("SSE event channel closed, engine continues for DB save");
        }

        // LLM loop with tool calls
        let mut final_response = String::new();
        let mut final_thinking_blocks: Vec<hydeclaw_types::ThinkingBlock> = vec![];
        let loop_config = self.tool_loop_config();
        let mut detector = LoopDetector::new(&loop_config);
        let mut loop_nudge_count: usize = 0;
        let mut did_reset_session = false;
        let mut empty_retry_count: u8 = 0;
        let mut auto_continue_count: u8 = 0;
        let mut context_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        let mut consecutive_failures: usize = 0;
        let mut using_fallback = false;
        let mut fallback_provider: Option<Arc<dyn crate::agent::providers::LlmProvider>> = None;

        for iteration in 0..loop_config.effective_max_iterations() {
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
                    if event_tx_fwd.send(StreamEvent::TextDelta(chunk)).is_err() {
                        tracing::debug!("SSE forwarder: event channel closed");
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
                            using_fallback = true;
                            consecutive_failures = 0;
                            tracing::warn!(
                                agent = %self.agent.name,
                                iteration,
                                "switching to fallback provider after consecutive failures (SSE)"
                            );
                            if event_tx.send(StreamEvent::StepFinish { step_id, finish_reason: "fallback".into() }).is_err() {
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
                    let fallback = error_classify::format_user_error(&e);
                    if event_tx.send(StreamEvent::TextDelta(fallback.clone())).is_err() {
                        tracing::debug!("SSE event channel closed, engine continues for DB save");
                    }
                    final_response = fallback;
                    if event_tx.send(StreamEvent::StepFinish { step_id, finish_reason: "error".into() }).is_err() {
                        tracing::debug!("SSE event channel closed, engine continues for DB save");
                    }
                    let reason_str = format!("SSE LLM call failed: {e}");
                    lifecycle_guard.fail(&reason_str).await;
                    {
                        let db = self.db.clone();
                        let agent_name = self.agent.name.clone();
                        let rs = reason_str.clone();
                        if let Some(ref ui_tx) = self.ui_event_tx {
                            let tx = ui_tx.clone();
                            tokio::spawn(async move {
                                crate::gateway::notify(
                                    &db, &tx, "agent_error",
                                    "Agent Error",
                                    &format!("Agent {} run failed: {}", agent_name, rs),
                                    serde_json::json!({"agent": agent_name, "reason": rs}),
                                ).await.ok();
                            });
                        }
                    }
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
                    {
                        let db = self.db.clone();
                        let agent_name = self.agent.name.clone();
                        let cnt = auto_continue_count;
                        let max = loop_config.max_auto_continues;
                        if let Some(ref ui_tx) = self.ui_event_tx {
                            let tx = ui_tx.clone();
                            tokio::spawn(async move {
                                crate::gateway::notify(
                                    &db, &tx, "auto_continue",
                                    &format!("Auto-continue: {}", agent_name),
                                    &format!("Agent continued unfinished task (attempt {}/{})", cnt, max),
                                    serde_json::json!({"agent": agent_name}),
                                ).await.ok();
                            });
                        }
                    }
                    let _ = event_tx.send(StreamEvent::StepFinish {
                        step_id: step_id.clone(),
                        finish_reason: "continuation".into(),
                    });
                    let _ = event_tx.send(StreamEvent::Finish {
                        finish_reason: "continuation".into(),
                        continuation: true,
                    });
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
            if let Err(e) = sm.save_message(
                session_id,
                "assistant",
                &cleaned_content,
                tc_json.as_ref(),
                None,
            )
            .await {
                tracing::warn!(error = %e, session_id = %session_id, "failed to save assistant message to DB");
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
                        let (display_result, db_result) = if let Some(json_str) = tool_result.strip_prefix(RICH_CARD_PREFIX) {
                            if let Ok(data) = serde_json::from_str::<serde_json::Value>(json_str) {
                                let card_type = data.get("card_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("table")
                                    .to_string();
                                if event_tx.send(StreamEvent::RichCard { card_type, data }).is_err() {
                                    tracing::debug!("SSE event channel closed, engine continues for DB save");
                                }
                            }
                            ("Rich card displayed".to_string(), "Rich card displayed".to_string())
                        } else if tool_result.contains(FILE_PREFIX) {
                            let db_result = tool_result.clone();
                            let mut clean_lines = Vec::new();
                            for line in tool_result.lines() {
                                if let Some(json_str) = line.strip_prefix(FILE_PREFIX) {
                                    if let Ok(meta) = serde_json::from_str::<serde_json::Value>(json_str) {
                                        let url = meta.get("url").and_then(|v| v.as_str()).unwrap_or("");
                                        let media_type = meta.get("mediaType").and_then(|v| v.as_str()).unwrap_or("application/octet-stream");
                                        if !url.is_empty()
                                            && event_tx.send(StreamEvent::File { url: url.to_string(), media_type: media_type.to_string() }).is_err() {
                                                tracing::debug!("SSE event channel closed, engine continues for DB save");
                                            }
                                    }
                                } else {
                                    clean_lines.push(line.as_ref());
                                }
                            }
                            let text = clean_lines.join("\n");
                            let display_result = if text.is_empty() { "Image displayed inline in the chat. Do NOT use canvas or other tools to show it again.".to_string() } else { text };
                            (display_result, db_result)
                        } else {
                            (tool_result.clone(), tool_result.clone())
                        };

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

                        if let Err(e) = sm.save_message(
                            session_id, "tool", &db_result, None, Some(tc_id),
                        ).await {
                            tracing::warn!(error = %e, session_id = %session_id, "failed to save tool result to DB");
                        }
                    }
                    false
                }
                Err(parallel_impl::LoopBreak(reason)) => {
                    if loop_nudge_count < loop_config.max_loop_nudges {
                        let nudge_desc = reason.as_deref().unwrap_or("repeating pattern");
                        let nudge_msg = format!(
                            "LOOP DETECTED: You have repeated the same sequence of actions ({desc}). \
                             Change your approach entirely. If the task is too large for a single session, \
                             tell the user and suggest breaking it into smaller steps. Do NOT retry the same approach.",
                            desc = nudge_desc
                        );
                        messages.push(Message {
                            role: MessageRole::System,
                            content: nudge_msg,
                            tool_calls: None,
                            tool_call_id: None,
                            thinking_blocks: vec![],
                        });
                        loop_nudge_count += 1;
                        detector.reset();
                        tracing::warn!(
                            agent = %self.agent.name,
                            nudge_count = loop_nudge_count,
                            reason = ?reason,
                            "loop nudge injected (SSE path)"
                        );
                        false // continue loop
                    } else {
                        tracing::error!(
                            agent = %self.agent.name,
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
                    tracing::warn!(
                        agent = %self.agent.name,
                        max_iterations = loop_config.effective_max_iterations(),
                        "agent reached iteration limit"
                    );
                    if let Some(ref ui_tx) = self.ui_event_tx {
                        let db = self.db.clone();
                        let tx = ui_tx.clone();
                        let agent_name = self.agent.name.clone();
                        let max_iter = loop_config.effective_max_iterations();
                        tokio::spawn(async move {
                            crate::gateway::notify(
                                &db, &tx, "iteration_limit",
                                &format!("Iteration limit: {}", agent_name),
                                &format!("Agent {} reached its iteration limit ({} iterations). The task may be incomplete.", agent_name, max_iter),
                                serde_json::json!({"agent": agent_name, "max_iterations": max_iter}),
                            ).await.ok();
                        });
                    }
                }
                // Notify if loop was broken after max nudges
                if loop_broken && loop_nudge_count >= loop_config.max_loop_nudges {
                    if let Some(ref ui_tx) = self.ui_event_tx {
                        let db = self.db.clone();
                        let tx = ui_tx.clone();
                        let agent_name = self.agent.name.clone();
                        let sid = session_id;
                        tokio::spawn(async move {
                            crate::gateway::notify(
                                &db, &tx, "agent_loop_detected",
                                &format!("Agent stuck in loop: {}", agent_name),
                                &format!("Agent {} was stopped after detecting a repeating pattern. Session: {}", agent_name, sid),
                                serde_json::json!({"agent": agent_name, "session_id": sid.to_string()}),
                            ).await.ok();
                        });
                    }
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

                match self.provider.chat(&messages, &[]).await {
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
                        {
                            let db = self.db.clone();
                            let agent_name = self.agent.name.clone();
                            let rs = reason_str.clone();
                            if let Some(ref ui_tx) = self.ui_event_tx {
                                let tx = ui_tx.clone();
                                tokio::spawn(async move {
                                    crate::gateway::notify(
                                        &db, &tx, "agent_error",
                                        "Agent Error",
                                        &format!("Agent {} run failed: {}", agent_name, rs),
                                        serde_json::json!({"agent": agent_name, "reason": rs}),
                                    ).await.ok();
                                });
                            }
                        }
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
        sm.save_message_ex(session_id, "assistant", &final_response, None, None, Some(&self.agent.name), thinking_json.as_ref())
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

        // Clear processing session context
        *self.processing_session_id().lock().await = None;
        *self.sse_event_tx().lock().await = None;

        // Post-session graph extraction (background)
        if self.memory_store.is_available() && messages.len() >= 5 {
            let db = self.db.clone();
            let provider = self.provider.clone();
            let sid = session_id;
            let msgs = std::sync::Arc::new(messages);
            tokio::spawn(async move {
                if let Err(e) = crate::memory_graph::extract_session_to_graph(&db, &provider, sid, msgs).await {
                    tracing::debug!(session = %sid, error = %e, "post-session graph extraction skipped");
                }
            });
        }

        Ok(())
    }
}