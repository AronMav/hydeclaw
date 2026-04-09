//! Context building and session compaction.
//! Extracted from engine.rs for readability.

use super::*;

impl AgentEngine {
    /// Build common context: session, messages, system prompt.
    /// Delegates to `self.context_builder.build(...)` — returns a `ContextSnapshot`.
    /// If `resume_session_id` is Some, reuses that session instead of creating/finding one.
    pub(super) async fn build_context(
        &self,
        msg: &IncomingMessage,
        include_tools: bool,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<crate::agent::context_builder::ContextSnapshot> {
        let cb = self.context_builder.get()
            .expect("context_builder not initialized — call set_context_builder after engine Arc creation");
        cb.build(msg, include_tools, resume_session_id, force_new_session).await
    }


    /// Build a SecretsEnvResolver for YAML tool env resolution.
    pub(super) fn make_resolver(&self) -> SecretsEnvResolver {
        SecretsEnvResolver {
            secrets: self.secrets().clone(),
            agent_name: self.agent.name.clone(),
        }
    }

    /// Build OAuthContext for provider-based YAML tool auth (e.g. `oauth_provider: github`).
    pub(super) fn make_oauth_context(&self) -> Option<crate::tools::yaml_tools::OAuthContext> {
        self.oauth().as_ref().map(|mgr| crate::tools::yaml_tools::OAuthContext {
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
