//! Context building and session compaction.
//! Extracted from engine.rs for readability.

use super::*;

impl AgentEngine {
    /// Build common context: session, messages, system prompt.
    /// Returns (session_id, messages, available_tools).
    /// If `resume_session_id` is Some, reuses that session instead of creating/finding one.
    pub(super) async fn build_context(
        &self,
        msg: &IncomingMessage,
        include_tools: bool,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<(Uuid, Vec<Message>, Vec<ToolDefinition>)> {
        // 1. Get or create session (or resume existing)
        let sm = SessionManager::new(self.db.clone());
        let session_id = if let Some(sid) = resume_session_id {
            sm.resume(sid).await?
        } else if force_new_session {
            sm.create_new(&self.agent.name, &msg.user_id, &msg.channel).await?
        } else {
            let dm_scope = self.agent.session.as_ref()
                .map(|s| s.dm_scope.as_str())
                .unwrap_or("per-channel-peer");
            sm.get_or_create(&self.agent.name, &msg.user_id, &msg.channel, dm_scope).await?
        };

        // 2. Load conversation history
        let limit = self.agent.max_history_messages.unwrap_or(50) as i64;
        let history = sm.load_messages(session_id, Some(limit)).await?;

        // 3. Build system prompt with MCP tool schemas
        let ws_prompt =
            workspace::load_workspace_prompt(&self.workspace_dir, &self.agent.name).await?;

        // MCP tool schemas in system prompt: name + description only.
        // Full parameter schemas are provided via native tool definitions (section 6).
        let mcp_schemas: Vec<String> = if let Some(ref mcp) = self.mcp {
            let defs = mcp.all_tool_definitions().await;
            defs.iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect()
        } else {
            vec![]
        };

        // 4. Capabilities + system prompt
        let user_text = msg.text.clone().unwrap_or_default();

        let capabilities = workspace::CapabilityFlags {
            has_search: self.has_tool("search_web").await || self.has_tool("search_web_fresh").await,
            has_memory: self.memory_store.is_available(),
            has_message_actions: self.channel_router.is_some(),
            has_cron: self.scheduler.is_some(),
            has_yaml_tools: true,
            has_browser: Self::browser_renderer_url() != "disabled",
            has_host_exec: self.agent.base && self.sandbox.is_none(),
            is_base: self.agent.base,
        };

        let mut runtime = self.runtime_context(msg);
        runtime.channels = self.get_channel_info().await;
        let mut system_prompt = workspace::build_system_prompt(
            &ws_prompt,
            &mcp_schemas,
            &capabilities,
            &self.agent.language,
            &runtime,
        );

        // 4b. Skill matching removed — skills are now loaded on-demand via skill_use tool.

        // 4c. Skill capture prompt — if user requests saving approach as a skill
        {
            let msg_lower = user_text.to_lowercase();
            let is_capture_request =
                (msg_lower.contains("save") && msg_lower.contains("skill"))
                || (msg_lower.contains("сохрани") && (msg_lower.contains("навык") || msg_lower.contains("скилл")));
            if is_capture_request {
                system_prompt.push_str(
                    "\n\n## Skill Capture\n\
                     The user wants to save the approach from the previous task as a reusable skill.\n\
                     Use workspace_write to create a file in workspace/skills/ with YAML frontmatter \
                     (name, description, triggers, tools_required) and markdown body.\n\
                     Extract the strategy, not specific data.\n"
                );
            }
        }

        // 4d. Multi-agent session context (Phase 19: CTXA-02, CTXA-03)
        // When session has multiple participants, inform agent about collaboration context
        if let Ok(participants) = sessions::get_participants(&self.db, session_id).await
            && participants.len() > 1
        {
            system_prompt.push_str("\n\n## Multi-Agent Session\n");
            system_prompt.push_str("You are in a collaborative multi-agent session.\n\n");
            system_prompt.push_str("**Participants:** ");
            system_prompt.push_str(&participants.join(", "));
            system_prompt.push_str("\n\n");
            system_prompt.push_str("**CRITICAL RULE:** When another agent hands off to you or mentions you, ");
            system_prompt.push_str("you MUST respond to the question or task directly. ");
            system_prompt.push_str("Do NOT redirect back to the agent who called you. ");
            system_prompt.push_str("Do NOT say 'ask them directly'. Answer the question yourself.\n\n");
            system_prompt.push_str("**Forward handoff:** If the task requires ANOTHER agent's expertise ");
            system_prompt.push_str("(not the one who called you), use the `handoff` tool to delegate forward. ");
            system_prompt.push_str("Example: Agent A asks you to get info from Agent C — use handoff to Agent C.\n");
            system_prompt.push_str("Provide: `agent` (target name), `task` (what they should do), ");
            system_prompt.push_str("`context` (relevant facts — keep concise).\n");
        }

        // L0: Always-on pinned memory chunks — injected on every build_context call (CTX-01, CTX-02)
        let pinned_budget = self.app_config.memory.pinned_budget_tokens;
        let memory_ctx = self.build_memory_context(pinned_budget).await;
        if !memory_ctx.pinned_text.is_empty() {
            system_prompt.push_str(&memory_ctx.pinned_text);
        }
        // Store pinned IDs for L2 dedup (CTX-04)
        *self.pinned_chunk_ids.lock().await = memory_ctx.pinned_ids;

        tracing::info!(
            agent = %self.agent.name,
            prompt_bytes = system_prompt.len(),
            prompt_approx_tokens = system_prompt.len() / 4,
            "system_prompt_size"
        );

        // 5. Assemble messages
        let mut messages: Vec<Message> = vec![Message {
            role: MessageRole::System,
            content: system_prompt,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        }];

        for row in &history {
            // Filter out heartbeat-related messages from multi-agent context
            // These pollute the conversation history and confuse agents
            let content_lower = row.content.to_lowercase();
            if content_lower.contains("heartbeat_ok")
                || content_lower.contains("heartbeat ok")
                || (content_lower.contains("nothing to announce") && content_lower.len() < 100)
            {
                continue;
            }

            messages.push(row_to_message(row));
        }

        // Transcript repair — differential append scoped to last dangling assistant (ENG-01):
        // Instead of clearing messages + reloading from DB, extract missing call_ids from
        // the already-parsed messages and append synthetic results directly.
        if let Some(last_idx) = messages.iter().rposition(|m| {
            m.role == MessageRole::Assistant && m.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty())
        }) {
            let has_results = messages[last_idx + 1..].iter().any(|m| m.role == MessageRole::Tool);
            if !has_results {
                // Extract tool_call_ids from the last dangling assistant (already parsed by row_to_message / ENG-02)
                let all_call_ids: Vec<String> = messages[last_idx]
                    .tool_calls
                    .as_ref()
                    .map(|tcs| tcs.iter().map(|tc| tc.id.clone()).collect())
                    .unwrap_or_default();

                // Filter out any that already have a matching Tool message after last_idx
                let existing_ids: std::collections::HashSet<&str> = messages[last_idx + 1..]
                    .iter()
                    .filter(|m| m.role == MessageRole::Tool)
                    .filter_map(|m| m.tool_call_id.as_deref())
                    .collect();
                let missing_ids: Vec<String> = all_call_ids
                    .into_iter()
                    .filter(|id| !existing_ids.contains(id.as_str()))
                    .collect();

                if !missing_ids.is_empty() {
                    tracing::warn!(
                        session_id = %session_id,
                        count = missing_ids.len(),
                        "dangling tool calls detected — inserting synthetic results"
                    );

                    // Persist synthetic rows to DB (narrowed — no session-wide scan)
                    if let Err(e) = SessionManager::new(self.db.clone())
                        .insert_missing_tool_results(session_id, &missing_ids)
                        .await
                    {
                        tracing::warn!(error = %e, "failed to insert synthetic tool results");
                    }

                    // Append synthetic tool results directly — no DB reload, no re-parse (ENG-01)
                    for call_id in missing_ids {
                        messages.push(Message {
                            role: MessageRole::Tool,
                            content: "[interrupted] Tool execution was interrupted (process restart). Result unavailable.".to_string(),
                            tool_calls: None,
                            tool_call_id: Some(call_id),
                            thinking_blocks: vec![],
                        });
                    }
                }
            }
        }

        // Sanitize any MiniMax XML tool calls that leaked into stored tool results.
        // Prevents old sessions with corrupt context from causing cascading XML loops.
        if messages.iter().any(|m| m.role == MessageRole::Tool && m.content.contains("<minimax:tool_call>")) {
            messages = messages
                .into_iter()
                .map(|mut m| {
                    if m.role == MessageRole::Tool {
                        m.content = strip_minimax_xml(&m.content);
                    }
                    m
                })
                .collect();
            tracing::warn!("sanitized MiniMax XML tool calls from session context");
        }

        // Proactive tool output pruning (turn-based) — before first LLM call.
        // Complements compact_tool_results (reactive, fires at 70% threshold in the tool loop).
        if let Some(keep_turns) = self.agent.session.as_ref().and_then(|s| s.prune_tool_output_after_turns)
            && keep_turns > 0 {
                messages = prune_old_tool_outputs(&messages, keep_turns);
                tracing::debug!(keep_turns, "proactive tool output pruning applied");
            }

        // 6. Available tools (if requested)
        let available_tools = if include_tools {
            let mut tools = self.internal_tool_definitions();
            // Shared YAML tools (workspace/tools/*.yaml) — use 30s cache to avoid per-request disk reads.
            let yaml_tools: Vec<crate::tools::yaml_tools::YamlToolDef> = {
                let cache = self.yaml_tools_cache.read().await;
                if cache.0.elapsed() < std::time::Duration::from_secs(30) && !cache.1.is_empty() {
                    cache.1.values().cloned().collect()
                } else {
                    drop(cache);
                    let loaded = crate::tools::yaml_tools::load_yaml_tools(&self.workspace_dir, false).await;
                    let map: std::collections::HashMap<String, crate::tools::yaml_tools::YamlToolDef> =
                        loaded.iter().cloned().map(|t| (t.name.clone(), t)).collect();
                    *self.yaml_tools_cache.write().await = (std::time::Instant::now(), map);
                    loaded
                }
            };
            let is_base = self.agent.base;
            let penalties = self.penalty_cache.get_penalties().await;
            let mut yaml_filtered: Vec<_> = yaml_tools
                .into_iter()
                .filter(|t| !t.required_base || is_base)
                .collect();
            yaml_filtered.sort_by(|a, b| {
                let pa = penalties.get(&a.name).copied().unwrap_or(1.0);
                let pb = penalties.get(&b.name).copied().unwrap_or(1.0);
                pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal)
            });
            tools.extend(yaml_filtered.into_iter().map(|t| t.to_tool_definition()));
            if let Some(ref mcp) = self.mcp {
                tools.extend(mcp.all_tool_definitions().await);
            }
            let mut all_tools = self.filter_tools_by_policy(tools);

            // Dynamic top-K: if configured and tool count exceeds the limit, select most relevant
            if let Some(max_k) = self.agent.max_tools_in_context
                && all_tools.len() > max_k && !user_text.is_empty() {
                    all_tools = self.select_top_k_tools_semantic(all_tools, &user_text, max_k).await;
                }

            all_tools
        } else {
            vec![]
        };

        Ok((session_id, messages, available_tools))
    }


    /// Build a SecretsEnvResolver for YAML tool env resolution.
    pub(super) fn make_resolver(&self) -> SecretsEnvResolver {
        SecretsEnvResolver {
            secrets: self.secrets.clone(),
            agent_name: self.agent.name.clone(),
        }
    }

    /// Build OAuthContext for provider-based YAML tool auth (e.g. `oauth_provider: github`).
    pub(super) fn make_oauth_context(&self) -> Option<crate::tools::yaml_tools::OAuthContext> {
        self.oauth.as_ref().map(|mgr| crate::tools::yaml_tools::OAuthContext {
            manager: mgr.clone(),
            agent_id: self.agent.name.clone(),
        })
    }

    /// Truncate a string to `max` chars with "..." suffix, preserving char boundaries.
    /// Format a tool error as structured JSON for better LLM parsing.
    pub(super) fn format_tool_error(tool_name: &str, error: &str) -> String {
        serde_json::json!({"status": "error", "tool": tool_name, "error": error}).to_string()
    }

    pub(super) fn truncate_preview(s: &str, max: usize) -> String {
        if s.len() > max {
            format!("{}...", &s[..s.floor_char_boundary(max)])
        } else {
            s.to_string()
        }
    }

    /// Truncate a tool result to fit within remaining context budget.
    /// Preserves head + tail (tail may contain errors/JSON closing).
    /// Budget: 50% of remaining context, floor 2000 chars.
    pub(super) fn truncate_tool_result(&self, result: &str, current_context_chars: usize) -> String {
        let model_max_chars = Self::default_context_for_model(&self.agent.model) * 4;
        let remaining = model_max_chars.saturating_sub(current_context_chars);
        let limit = (remaining * 50 / 100).max(2000);
        if result.len() <= limit {
            return result.to_string();
        }
        let tail_region = &result[result.len().saturating_sub(1500)..];
        let tail_has_error = tail_region.contains("error") || tail_region.contains("Error")
            || tail_region.contains("failed") || tail_region.contains("exception");
        let tail_size = if tail_has_error { 1500 } else { 500 };
        let marker = format!("\n\n[... truncated {} → {} chars ...]\n\n", result.len(), limit);
        let head_size = limit.saturating_sub(tail_size).saturating_sub(marker.len());
        let head = &result[..result.floor_char_boundary(head_size)];
        let tail = &result[result.floor_char_boundary(result.len().saturating_sub(tail_size))..];
        tracing::debug!(original = result.len(), truncated = limit, tail_has_error, "tool result truncated");
        format!("{}{}{}", head, marker, tail)
    }

    /// Replace old tool results with "[compacted]" when context exceeds 70% of model window.
    /// Preserves the last `preserve_n` tool results and the system message.
    /// `context_chars` is a running total of character counts across all messages,
    /// maintained incrementally by the caller. Updated in place after compaction.
    pub(super) fn compact_tool_results(&self, messages: &mut [Message], context_chars: &mut usize) {
        let context_window = Self::default_context_for_model(&self.agent.model) * 4;
        let threshold = context_window * 70 / 100;
        if *context_chars <= threshold {
            return;
        }
        let preserve_n = self.agent.compaction.as_ref()
            .map(|c| c.preserve_last_n as usize)
            .unwrap_or(10);

        // Count tool messages, compact oldest ones
        let tool_indices: Vec<usize> = messages.iter().enumerate()
            .filter(|(_, m)| m.role == MessageRole::Tool)
            .map(|(i, _)| i)
            .collect();
        let to_compact = tool_indices.len().saturating_sub(preserve_n);
        if to_compact == 0 { return; }

        let mut compacted = 0usize;
        let mut chars_removed = 0usize;
        for &idx in tool_indices.iter().take(to_compact) {
            let old_len = messages[idx].content.chars().count();
            if old_len > 100 {
                let replacement = "[tool result compacted]";
                let new_len = replacement.len(); // 23 chars, all ASCII
                chars_removed += old_len - new_len;
                messages[idx].content = replacement.to_string();
                compacted += 1;
            }
        }
        if compacted > 0 {
            let old_total = *context_chars;
            *context_chars = context_chars.saturating_sub(chars_removed);
            tracing::info!(compacted, old_chars = old_total, new_chars = *context_chars, "compacted old tool results");
        }
    }

    /// Get compaction parameters from agent config.
    pub(super) fn compaction_params(&self) -> (usize, usize) {
        let max_tokens = self.agent.compaction.as_ref()
            .and_then(|c| c.max_context_tokens)
            .map(|t| t as usize)
            .unwrap_or_else(|| Self::default_context_for_model(&self.agent.model));
        let preserve_last_n = self.agent.compaction.as_ref()
            .map(|c| c.preserve_last_n as usize)
            .unwrap_or(10);
        (max_tokens, preserve_last_n)
    }

    /// Run compaction on messages if token budget exceeded, indexing extracted facts to memory.
    /// Pass `Some(detector)` when inside the LLM loop to inject a progress header after compaction.
    pub(super) async fn compact_messages(&self, messages: &mut Vec<Message>, detector: Option<&LoopDetector>) {
        let (max_tokens, preserve_last_n) = self.compaction_params();
        if let Ok(Some(facts)) = history::compact_if_needed(
            messages,
            self.provider.as_ref(),
            self.compaction_provider.as_deref(),
            max_tokens,
            preserve_last_n,
            Some(&self.agent.language),
        )
        .await
        {
            tracing::info!(facts = facts.len(), "extracted facts during compaction");
            self.audit(crate::db::audit::event_types::COMPACTION, None, serde_json::json!({"facts": facts.len(), "max_tokens": max_tokens}));
            self.index_facts_to_memory(&facts).await;

            // Inject / replace progress header after compaction
            if let Some(det) = detector {
                history::remove_progress_header(messages);
                let header = history::generate_progress_header(messages, det);
                // Insert at position 1 (after the system prompt at index 0), if present
                let insert_pos = if messages.first().map(|m| m.role == MessageRole::System).unwrap_or(false) { 1 } else { 0 };
                messages.insert(insert_pos, Message {
                    role: MessageRole::System,
                    content: header,
                    tool_calls: None,
                    tool_call_id: None,
                    thinking_blocks: vec![],
                });
            }

            // Notify user about compaction
            if let Some(ref ui_tx) = self.ui_event_tx {
                let db = self.db.clone();
                let tx = ui_tx.clone();
                let agent_name = self.agent.name.clone();
                tokio::spawn(async move {
                    crate::gateway::notify(
                        &db, &tx, "context_compaction",
                        &format!("Context compacted: {}", agent_name),
                        &format!("Agent {} session was compacted to stay within token budget", agent_name),
                        serde_json::json!({"agent": agent_name}),
                    ).await.ok();
                });
            }
        }
    }

    /// Compact a specific session's messages via API.
    /// Returns `(facts_extracted, new_message_count)`.
    pub async fn compact_session(&self, session_id: uuid::Uuid) -> Result<(usize, usize)> {
        let rows = SessionManager::new(self.db.clone()).load_messages(session_id, Some(2000)).await?;
        if rows.len() < 4 {
            anyhow::bail!("session too short to compact ({} messages)", rows.len());
        }

        let mut messages: Vec<Message> = rows.iter().map(row_to_message).collect();

        // Force compaction by using max_tokens=1 (threshold=0, always exceeds)
        let facts = history::compact_if_needed(
            &mut messages,
            self.provider.as_ref(),
            self.compaction_provider.as_deref(),
            1, // force: any token count > 0 triggers compaction
            2,
            Some(&self.agent.language),
        )
        .await?;

        let facts_count = facts.as_ref().map(|f| f.len()).unwrap_or(0);

        if let Some(ref facts) = facts {
            self.index_facts_to_memory(facts).await;
        }

        // Replace messages in DB (atomic transaction)
        let mut tx = self.db.begin().await?;
        sqlx::query("DELETE FROM messages WHERE session_id = $1")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        for msg in &messages {
            let role = match msg.role {
                hydeclaw_types::MessageRole::User => "user",
                hydeclaw_types::MessageRole::Assistant => "assistant",
                hydeclaw_types::MessageRole::System => "system",
                hydeclaw_types::MessageRole::Tool => "tool",
            };
            let tc_json = msg
                .tool_calls
                .as_ref()
                .and_then(|tc| serde_json::to_value(tc).ok());
            sqlx::query(
                "INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id, agent_id) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(session_id)
            .bind(role)
            .bind(&msg.content)
            .bind(tc_json.as_ref())
            .bind(msg.tool_call_id.as_deref())
            .bind(if role == "assistant" { Some(&self.agent.name) } else { None::<&String> })
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        let new_count = messages.len();
        self.audit(
            crate::db::audit::event_types::COMPACTION,
            Some("api"),
            serde_json::json!({
                "session_id": session_id.to_string(),
                "facts": facts_count,
                "new_messages": new_count,
                "original_messages": rows.len(),
            }),
        );

        tracing::info!(
            session_id = %session_id, facts = facts_count,
            old = rows.len(), new = new_count, "session compacted via API"
        );

        Ok((facts_count, new_count))
    }
}
