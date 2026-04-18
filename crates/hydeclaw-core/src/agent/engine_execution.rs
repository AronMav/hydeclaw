//! Main execution loop: handle_with_status, handle_streaming, handle_sse.
//! Extracted from engine.rs for readability.

use super::*;
use crate::agent::pipeline::execution as exec_helpers;
use crate::agent::tool_executor::ToolExecutor;

impl AgentEngine {
    /// Handle with optional status callback for real-time phase updates.
    /// `chunk_tx` — optional channel for streaming response chunks to the caller (e.g. progressive display).
    pub async fn handle_with_status(
        &self,
        msg: &IncomingMessage,
        status_tx: Option<mpsc::UnboundedSender<ProcessingPhase>>,
        chunk_tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<String> {
        // Sweep stale approval waiters (older than 10 minutes)
        self.cfg().approval_manager.prune_stale().await;

        // Track active request for graceful shutdown/SIGHUP drain
        let cancel_guard = Some(self.state.register_request());

        // Hook: BeforeMessage
        if let crate::agent::hooks::HookAction::Block(reason) = self.hooks().fire(&crate::agent::hooks::HookEvent::BeforeMessage) {
            if let Some((ref id, _)) = cancel_guard {
                self.state.unregister_request(id);
            }
            anyhow::bail!("blocked by hook: {}", reason);
        }

        let crate::agent::context_builder::ContextSnapshot { session_id, mut messages, tools: available_tools } =
            self.build_context(msg, true, None, false).await?;

        // Mark session as running — watchdog and startup cleanup use this
        let sm = SessionManager::new(self.cfg().db.clone());
        if let Err(e) = sm.set_run_status(session_id, "running").await {
            tracing::warn!(session_id = %session_id, error = %e, "failed to mark session as running");
        }
        exec_helpers::log_wal_running_with_retry(&sm, session_id).await;
        // RAII guard: if we exit early via `?` (error path), mark session as 'failed'.
        let mut lifecycle_guard = SessionLifecycleGuard::new(self.cfg().db.clone(), session_id);

        // Broadcast processing start to UI (typing indicator) + guard broadcasts end on drop
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

        // invite_agent removed (v3.0) — agent tool is the inter-agent mechanism

        // Add current message, auto-fetch URLs if present
        let user_text = msg.text.clone().unwrap_or_default();

        // Slash commands — handle without LLM
        if let Some(result) = self.handle_command(&user_text, msg).await {
            lifecycle_guard.done().await;
            if let Some((ref id, _)) = cancel_guard {
                self.state.unregister_request(id);
            }
            return result;
        }

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

        // Prompt injection detection (logging-only)
        let injections = crate::tools::content_security::detect_prompt_injection(&enriched_text);
        if !injections.is_empty() {
            tracing::warn!(patterns = ?injections, "potential prompt injection detected");
            let preview = Self::truncate_preview(&enriched_text, 200);
            self.audit(crate::db::audit::event_types::PROMPT_INJECTION, msg.context.get("user_id").and_then(|v| v.as_str()), serde_json::json!({
                "patterns": injections, "text_preview": preview
            }));
        }

        messages.push(Message {
            role: MessageRole::User,
            content: enriched_text,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        });

        // Save user message (original, not enriched)
        let sender_agent_id = exec_helpers::extract_sender_agent_id(&msg.user_id);
        let user_msg_id = sm.save_message_ex(session_id, "user", &user_text, None, None, sender_agent_id, None, msg.leaf_message_id).await?;
        let mut last_msg_id = user_msg_id;

        // Context compaction if needed (model-aware token budget)
        self.compact_messages(&mut messages, None).await;

        // LLM loop (with tool calls)
        let mut final_response = String::new();
        let mut final_thinking_blocks: Vec<hydeclaw_types::ThinkingBlock> = vec![];
        let mut streamed_via_chunk_tx = false;
        let mut total_input_tokens: u32 = 0;
        let mut total_output_tokens: u32 = 0;
        let mut tool_iterations: u32 = 0;
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

            if let Some(ref tx) = status_tx {
                tx.send(ProcessingPhase::Thinking).ok();
            }

            // Compact old tool results if context is getting full
            self.compact_tool_results(&mut messages, &mut context_chars);

            // Use streaming if chunk_tx available (enables progressive display)
            let llm_result = if let Some(tx) = &chunk_tx {
                if let Some(ref fb) = fallback_provider {
                    self.chat_stream_with_transient_retry_using(fb, &mut messages, &available_tools, tx.clone()).await
                } else {
                    self.chat_stream_with_transient_retry(&mut messages, &available_tools, tx.clone()).await
                }
            } else if let Some(ref fb) = fallback_provider {
                self.chat_with_transient_retry_using(fb, &mut messages, &available_tools).await
            } else {
                self.chat_with_transient_retry(&mut messages, &available_tools).await
            };
            let response = match llm_result {
                Ok(r) => {
                    consecutive_failures = 0;
                    r
                }
                Err(e) => {
                    let class = error_classify::classify(&e);
                    // Auto-reset on session corruption (once)
                    if class == error_classify::LlmErrorClass::SessionCorruption && !did_reset_session {
                        did_reset_session = true;
                        tracing::warn!(error = %e, "session corrupted, resetting context and retrying");
                        messages.retain(|m| m.role == MessageRole::System);
                        messages.push(Message {
                            role: MessageRole::User,
                            content: user_text.clone(),
                            tool_calls: None,
                            tool_call_id: None,
                            thinking_blocks: vec![],
                        });
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
                                agent = %self.cfg().agent.name,
                                iteration,
                                "switching to fallback provider after consecutive failures"
                            );
                            continue;
                        }
                    }
                    tracing::error!(error = %e, iteration, "LLM call failed, returning fallback");
                    // Task 19: persist partial_text from cancel-class LlmCallError before
                    // surfacing. See engine_sse.rs for the matching hook in the streaming
                    // path; the two share `persist_partial_if_any` mounted in the
                    // `engine::sse_impl` submodule (path-included leaf).
                    //
                    // Issue #7: when a partial row is written, advance `last_msg_id`
                    // to its id so the final assistant-error row (saved below after
                    // the loop) hangs off the partial, keeping the thread linear
                    // under m012 branching.
                    if let Some(partial_id) = super::sse_impl::persist_partial_if_any(
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
                    super::sse_impl::record_llm_timeout_if_typed(&e);
                    final_response = error_classify::format_user_error(&e);
                    break;
                }
            };
            self.record_usage(&response, Some(session_id));
            if let Some(ref u) = response.usage {
                total_input_tokens += u.input_tokens;
                total_output_tokens += u.output_tokens;
            }

            if response.tool_calls.is_empty() {
                final_response = if let Some(notice) = &response.fallback_notice {
                    format!("{}\n\n{}", notice, response.content)
                } else {
                    response.content.clone()
                };
                final_thinking_blocks = response.thinking_blocks.clone();
                if final_response.is_empty() && empty_retry_count < 1 {
                    empty_retry_count += 1;
                    tracing::warn!(iteration, "LLM returned empty response, retrying once");
                    continue;
                }
                if final_response.is_empty() {
                    tracing::warn!(iteration, "LLM returned empty response after retry");
                }

                // Auto-continue: if LLM described remaining work or stopped due to length limit, nudge it to continue
                let is_length_limit = response.finish_reason.as_deref() == Some("length");
                if auto_continue_count < loop_config.max_auto_continues && !final_response.is_empty() && (is_length_limit || looks_incomplete(&final_response)) {
                    auto_continue_count += 1;
                    let reason = if is_length_limit { "response truncated by length limit" } else { "response looks incomplete" };
                    tracing::info!(iteration, count = auto_continue_count, max = loop_config.max_auto_continues, reason, "auto-continue: nudging LLM");
                    exec_helpers::notify_auto_continue(
                        self.cfg().db.clone(),
                        self.state().ui_event_tx.as_ref(),
                        &self.cfg().agent.name,
                        auto_continue_count,
                        loop_config.max_auto_continues,
                    );
                    if let Some(ref tx) = chunk_tx {
                        tx.send("\n\n...".to_string()).ok();
                    }
                    messages.push(Message {
                        role: MessageRole::User,
                        content: AUTO_CONTINUE_NUDGE.to_string(),
                        tool_calls: None,
                        tool_call_id: None,
                        thinking_blocks: vec![],
                    });
                    context_chars += AUTO_CONTINUE_NUDGE.len();
                    continue;
                }

                if chunk_tx.is_some() {
                    streamed_via_chunk_tx = true;
                }
                break;
            }

            tracing::info!(
                iteration,
                max = loop_config.effective_max_iterations(),
                tools = response.tool_calls.len(),
                "executing tool calls"
            );

            let cleaned_content = strip_thinking(&response.content);

            // Send intermediate text to channel (so Telegram shows progress)
            if let Some(ref tx) = chunk_tx
                && !cleaned_content.is_empty() {
                    tx.send(cleaned_content.clone()).ok();
                }

            messages.push(Message {
                role: MessageRole::Assistant,
                content: cleaned_content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
                thinking_blocks: vec![],
            });
            context_chars += cleaned_content.chars().count();

            // Save assistant message with tool_calls to DB (thinking stripped)
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

            if let Some(ref tx) = status_tx
                && let Some(tc) = response.tool_calls.first() {
                    tx.send(ProcessingPhase::CallingTool(tc.name.clone())).ok();
                }
            tool_iterations += 1;
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
                        messages.push(Message {
                            role: MessageRole::Tool,
                            content: tool_result.clone(),
                            tool_calls: None,
                            tool_call_id: Some(tc_id.clone()),
                            thinking_blocks: vec![],
                        });
                        context_chars += tool_result.chars().count();
                        match sm.save_message_ex(
                            session_id, "tool", tool_result, None, Some(tc_id), None, None, Some(last_msg_id),
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
                            "loop nudge injected (channel/session path)"
                        );
                        false // continue loop
                    } else {
                        tracing::error!(
                            agent = %self.cfg().agent.name,
                            nudge_count = loop_nudge_count,
                            "max loop nudges reached, force-stopping agent (channel/session path)"
                        );
                        true // broken
                    }
                }
            };

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
                // Forced final call — use streaming if chunk_tx is available
                let forced_result = if let Some(ref tx) = chunk_tx {
                    self.cfg().provider.chat_stream(&messages, &[], tx.clone()).await
                } else {
                    self.cfg().provider.chat(&messages, &[]).await
                };
                match forced_result {
                    Ok(forced) => {
                        final_response = forced.content;
                        if chunk_tx.is_some() { streamed_via_chunk_tx = true; }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "forced final LLM call failed");
                        final_response = error_classify::format_user_error(&e);
                    }
                }
                break;
            }
        }

        if let Some(ref tx) = status_tx {
            tx.send(ProcessingPhase::Composing).ok();
        }

        // Skill capture hint: suggest saving multi-step approach as a skill
        if tool_iterations >= 3
            && !crate::agent::channel_kind::channel::is_automated(&msg.channel)
            && !final_response.is_empty()
        {
            final_response.push_str(
                "\n\n---\n_This task used a multi-step approach not covered by any skill. \
                To save it for reuse, say: \"save as skill\" / \"сохрани как навык\"._"
            );
        }

        let thinking_level = self.state().thinking_level.load(std::sync::atomic::Ordering::Relaxed);
        let final_response = maybe_strip_thinking(&final_response, msg, thinking_level);

        // Send final response to chunk consumer (if not already streamed)
        if let Some(ref tx) = chunk_tx
            && !streamed_via_chunk_tx && !final_response.is_empty() {
                tx.send(final_response.clone()).ok();
            }

        let thinking_json = if final_thinking_blocks.is_empty() {
            None
        } else {
            serde_json::to_value(&final_thinking_blocks).ok()
        };
        sm.save_message_ex(session_id, "assistant", &final_response, None, None, Some(&self.cfg().agent.name), thinking_json.as_ref(), Some(last_msg_id))
            .await?;

        self.maybe_trim_session(session_id).await;

        // Append usage footer (only for non-streaming, not saved to DB)
        let with_footer = if total_input_tokens > 0 && chunk_tx.is_none() {
            format!("{}\n\n---\n📊 {}→{} tokens", final_response, total_input_tokens, total_output_tokens)
        } else {
            final_response
        };

        lifecycle_guard.done().await;

        // Post-session knowledge extraction (background, non-blocking)
        exec_helpers::spawn_knowledge_extraction(
            self.cfg().db.clone(), session_id, self.cfg().agent.name.clone(),
            self.cfg().provider.clone(), self.cfg().memory_store.clone(), messages.len(),
        );

        // Unregister active request (cancel/drain tracking)
        if let Some((ref id, _)) = cancel_guard {
            self.state.unregister_request(id);
        }

        Ok(with_footer)
    }

    /// Handle with streaming: sends content chunks via mpsc channel for SSE or progressive display.
    pub async fn handle_streaming(
        &self,
        msg: &IncomingMessage,
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let thinking_level = self.state().thinking_level.load(std::sync::atomic::Ordering::Relaxed);
        let crate::agent::context_builder::ContextSnapshot { session_id, mut messages, tools: _ } =
            self.build_context(msg, false, None, false).await?;

        // Lifecycle tracking
        let sm = SessionManager::new(self.cfg().db.clone());
        if let Err(e) = sm.set_run_status(session_id, "running").await {
            tracing::warn!(session_id = %session_id, error = %e, "failed to mark streaming session as running");
        }
        let mut lifecycle_guard = SessionLifecycleGuard::new(self.cfg().db.clone(), session_id);

        let user_text = msg.text.clone().unwrap_or_default();
        messages.push(Message {
            role: MessageRole::User,
            content: user_text.clone(),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        });

        let sender_agent_id = exec_helpers::extract_sender_agent_id(&msg.user_id);
        sm.save_message_ex(session_id, "user", &user_text, None, None, sender_agent_id, None, None).await?;

        // Stream LLM response (no tools for streaming — simple text response)
        let (final_response, stream_thinking_json) = match self.cfg().provider.chat_stream(&messages, &[], chunk_tx).await {
            Ok(response) => {
                let tb_json = if response.thinking_blocks.is_empty() {
                    None
                } else {
                    serde_json::to_value(&response.thinking_blocks).ok()
                };
                (maybe_strip_thinking(&response.content, msg, thinking_level), tb_json)
            }
            Err(e) => {
                tracing::error!(error = %e, "streaming LLM call failed, returning fallback");
                let reason_str = format!("streaming LLM call failed: {e}");
                lifecycle_guard.fail(&reason_str).await;
                exec_helpers::notify_agent_error(
                    self.cfg().db.clone(),
                    self.state().ui_event_tx.as_ref(),
                    &self.cfg().agent.name,
                    &reason_str,
                );
                (error_classify::format_user_error(&e), None)
            }
        };

        sm.save_message_ex(session_id, "assistant", &final_response, None, None, Some(&self.cfg().agent.name), stream_thinking_json.as_ref(), None)
            .await?;
        self.maybe_trim_session(session_id).await;

        lifecycle_guard.done().await;

        // Post-session knowledge extraction (background, non-blocking)
        exec_helpers::spawn_knowledge_extraction(
            self.cfg().db.clone(), session_id, self.cfg().agent.name.clone(),
            self.cfg().provider.clone(), self.cfg().memory_store.clone(), messages.len(),
        );

        Ok(final_response)
    }
}
