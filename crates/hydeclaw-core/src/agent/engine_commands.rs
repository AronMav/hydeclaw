//! Slash command routing — extracted from engine.rs for readability.

use super::*;
use crate::agent::localization;

impl AgentEngine {
    /// Handle /slash commands. Returns Some(result) if a command matched, None otherwise.
    pub(super) async fn handle_command(&self, text: &str, msg: &IncomingMessage) -> Option<Result<String>> {
        let cmd = text.trim();
        if !cmd.starts_with('/') {
            return None;
        }
        let (raw_command, args) = cmd.split_once(' ').unwrap_or((cmd, ""));
        // Strip @botname suffix (Telegram sends /status@my_bot)
        let command = raw_command.split('@').next().unwrap_or(raw_command);
        tracing::debug!(command = %command, raw = %raw_command, "slash command received");
        let dm_scope = self.agent.session.as_ref()
            .map(|s| s.dm_scope.as_str())
            .unwrap_or("per-channel-peer");

        let s = localization::get_strings(&self.agent.language);

        match command {
            "/status" => {
                let session_info = match sessions::find_active_session(
                    &self.db, &self.agent.name, &msg.user_id, &msg.channel, dm_scope,
                ).await {
                    Ok(Some(sid)) => {
                        let count = sessions::count_messages(&self.db, sid).await.unwrap_or(0);
                        localization::fmt(s.status_session_active, &[&count.to_string()])
                    }
                    _ => s.status_session_none.to_string(),
                };
                let chunks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_chunks WHERE agent_id = $1")
                    .bind(&self.agent.name)
                    .fetch_one(&self.db).await.unwrap_or(0);
                let provider_name = self.provider.name();
                let current_model = self.provider.current_model();
                Some(Ok(
                    localization::fmt(s.status_format, &[&self.agent.name, provider_name, &current_model, &session_info, &chunks.to_string()])
                ))
            }
            "/new" => {
                match sessions::find_active_session(
                    &self.db, &self.agent.name, &msg.user_id, &msg.channel, dm_scope,
                ).await {
                    Ok(Some(sid)) => {
                        if let Err(e) = sessions::delete_session(&self.db, sid).await {
                            return Some(Err(e));
                        }
                        Some(Ok(s.new_session_started.to_string()))
                    }
                    Ok(None) => Some(Ok(s.new_session_none.to_string())),
                    Err(e) => Some(Err(e)),
                }
            }
            "/reset" => {
                // Delete session
                if let Ok(Some(sid)) = sessions::find_active_session(
                    &self.db, &self.agent.name, &msg.user_id, &msg.channel, dm_scope,
                ).await {
                    let _ = sessions::delete_session(&self.db, sid).await;
                }
                // Delete this agent's unpinned memory (scoped by agent_id)
                let deleted: i64 = sqlx::query_scalar(
                    "WITH d AS (DELETE FROM memory_chunks WHERE pinned = false AND agent_id = $1 RETURNING 1) SELECT COUNT(*) FROM d"
                ).bind(&self.agent.name).fetch_one(&self.db).await.unwrap_or(0);
                Some(Ok(localization::fmt(s.reset_done, &[&deleted.to_string()])))
            }
            "/compact" => {
                let sid = match sessions::find_active_session(
                    &self.db, &self.agent.name, &msg.user_id, &msg.channel, dm_scope,
                ).await {
                    Ok(Some(sid)) => sid,
                    _ => return Some(Ok(s.compact_no_session.to_string())),
                };
                let history_rows = match sessions::load_messages(&self.db, sid, Some(self.agent.max_history_messages.unwrap_or(50) as i64)).await {
                    Ok(h) => h,
                    Err(e) => return Some(Err(e)),
                };
                let mut messages: Vec<Message> = history_rows.into_iter().map(|m| Message {
                    role: match m.role.as_str() {
                        "user" => MessageRole::User,
                        "assistant" => MessageRole::Assistant,
                        "tool" => MessageRole::Tool,
                        _ => MessageRole::System,
                    },
                    content: m.content,
                    tool_calls: m.tool_calls.and_then(|tc| {
                        serde_json::from_value::<Vec<hydeclaw_types::ToolCall>>(tc).ok()
                    }),
                    tool_call_id: m.tool_call_id,
                    thinking_blocks: vec![],
                }).collect();
                let before = messages.len();
                let preserve = self.agent.compaction.as_ref()
                    .map(|c| c.preserve_last_n as usize).unwrap_or(10);
                let messages_snapshot = messages.clone();
                let mut compact_result = None;
                for attempt in 0..2u8 {
                    match history::compact_if_needed(
                        &mut messages, self.provider.as_ref(), self.compaction_provider.as_deref(), 0, preserve, Some(&self.agent.language),
                    ).await {
                        Ok(r) => { compact_result = Some(r); break; }
                        Err(e) if attempt == 0 => {
                            tracing::warn!(error = %e, "compaction failed, retrying...");
                            messages = messages_snapshot.clone();
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        }
                        Err(e) => {
                            return Some(Ok(format!("Compaction failed after retry: {}", e)));
                        }
                    }
                }
                match compact_result {
                    Some(Some(facts)) => {
                        let after = messages.len();
                        self.index_facts_to_memory(&facts).await;

                        // Persist compacted messages to DB (atomic transaction)
                        if let Err(e) = async {
                            let mut tx = self.db.begin().await?;
                            sqlx::query("DELETE FROM messages WHERE session_id = $1")
                                .bind(sid)
                                .execute(&mut *tx)
                                .await?;
                            for m in &messages {
                                let role = match m.role {
                                    hydeclaw_types::MessageRole::User => "user",
                                    hydeclaw_types::MessageRole::Assistant => "assistant",
                                    hydeclaw_types::MessageRole::System => "system",
                                    hydeclaw_types::MessageRole::Tool => "tool",
                                };
                                let tc_json = m.tool_calls.as_ref()
                                    .and_then(|tc| serde_json::to_value(tc).map_err(|e| {
                                        tracing::warn!(error = %e, "failed to serialize tool_calls during compact");
                                        e
                                    }).ok());
                                sqlx::query(
                                    "INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id) \
                                     VALUES ($1, $2, $3, $4, $5)",
                                )
                                .bind(sid)
                                .bind(role)
                                .bind(&m.content)
                                .bind(tc_json.as_ref())
                                .bind(m.tool_call_id.as_deref())
                                .execute(&mut *tx)
                                .await?;
                            }
                            tx.commit().await?;
                            Ok::<(), anyhow::Error>(())
                        }.await {
                            return Some(Ok(format!("Compaction succeeded but DB persist failed: {}", e)));
                        }

                        Some(Ok(
                            localization::fmt(s.compact_done, &[&before.to_string(), &after.to_string(), &facts.len().to_string()])
                        ))
                    }
                    Some(None) => Some(Ok(s.compact_not_needed.to_string())),
                    None => Some(Ok("Compaction failed.".to_string())),
                }
            }
            "/model" => {
                let model_arg = args.trim();
                if model_arg.is_empty() || model_arg == "status" {
                    let current = self.provider.current_model();
                    let base = &self.agent.model;
                    if current == *base {
                        Some(Ok(localization::fmt(s.model_current, &[&current])))
                    } else {
                        Some(Ok(
                            localization::fmt(s.model_override, &[&current, base])
                        ))
                    }
                } else if model_arg == "reset" {
                    self.provider.set_model_override(None);
                    self.invalidate_yaml_tools_cache().await;
                    Some(Ok(localization::fmt(s.model_reset, &[&self.agent.model])))
                } else {
                    self.provider.set_model_override(Some(model_arg.to_string()));
                    self.invalidate_yaml_tools_cache().await;
                    Some(Ok(localization::fmt(s.model_switched, &[model_arg])))
                }
            }
            "/think" => {
                let arg = args.trim();
                let current = self.thinking_level.load(std::sync::atomic::Ordering::Relaxed);
                let new_level: u8 = match arg {
                    "off" | "0" | "false" | "нет" => 0,
                    "on" | "true" | "да" => 3,
                    "minimal" | "min" | "1" => 1,
                    "low" | "2" => 2,
                    "medium" | "med" | "3" => 3,
                    "high" | "4" => 4,
                    "max" | "xhigh" | "5" => 5,
                    _ => if current == 0 { 3 } else { 0 }, // toggle
                };
                self.thinking_level.store(new_level, std::sync::atomic::Ordering::Relaxed);
                let label = match new_level {
                    0 => "OFF",
                    1 => "MINIMAL",
                    2 => "LOW",
                    3 => "MEDIUM",
                    4 => "HIGH",
                    5 => "MAX",
                    _ => "?",
                };
                Some(Ok(
                    localization::fmt(s.think_level, &[label, &new_level.to_string()])
                ))
            }
            "/usage" => {
                let session_id = match sessions::find_active_session(
                    &self.db, &self.agent.name, &msg.user_id, &msg.channel, dm_scope,
                ).await {
                    Ok(Some(sid)) => Some(sid),
                    _ => None,
                };

                // Session usage
                let session_stats = if let Some(sid) = session_id {
                    sqlx::query_as::<_, (i64, i64, i64)>(
                        "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COUNT(*) \
                         FROM usage_log WHERE session_id = $1"
                    )
                    .bind(sid)
                    .fetch_optional(&self.db)
                    .await
                    .ok()
                    .flatten()
                } else {
                    None
                };

                // Today's agent usage
                let today_stats = sqlx::query_as::<_, (i64, i64, i64)>(
                    "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COUNT(*) \
                     FROM usage_log WHERE agent_id = $1 AND created_at > CURRENT_DATE"
                )
                .bind(&self.agent.name)
                .fetch_optional(&self.db)
                .await
                .ok()
                .flatten()
                .unwrap_or((0, 0, 0));

                let mut out = localization::fmt(s.usage_header, &[&self.agent.name, &today_stats.0.to_string(), &today_stats.1.to_string(), &today_stats.2.to_string()]);

                if let Some((s_inp, s_out, s_calls)) = session_stats {
                    out.push('\n');
                    out.push_str(
                        &localization::fmt(s.usage_session, &[&s_inp.to_string(), &s_out.to_string(), &s_calls.to_string()])
                    );
                }

                Some(Ok(out))
            }
            "/export" => {
                let sid = match sessions::find_active_session(
                    &self.db, &self.agent.name, &msg.user_id, &msg.channel, dm_scope,
                ).await {
                    Ok(Some(sid)) => sid,
                    _ => return Some(Ok(s.export_no_session.to_string())),
                };
                let rows = match sessions::load_messages(&self.db, sid, Some(500)).await {
                    Ok(r) => r,
                    Err(e) => return Some(Err(e)),
                };
                if rows.is_empty() {
                    return Some(Ok(s.export_empty.to_string()));
                }
                let mut out = localization::fmt(s.export_header, &[&self.agent.name, &sid.to_string()]);
                for m in &rows {
                    let role = match m.role.as_str() {
                        "user" => "👤 User",
                        "assistant" => "🤖 Assistant",
                        "system" => "⚙️ System",
                        "tool" => "🔧 Tool",
                        _ => &m.role,
                    };
                    let time = m.created_at.format("%H:%M");
                    let content = if m.content.chars().count() > 500 {
                        format!("{}...", m.content.chars().take(500).collect::<String>())
                    } else {
                        m.content.clone()
                    };
                    out.push_str(&format!("\n**{role}** ({time}):\n{content}\n"));
                }
                Some(Ok(out))
            }
            "/help" => {
                Some(Ok(s.help_text.to_string()))
            }
            "/memory" => {
                let query = args.trim();
                let (results, mode) = if query.is_empty() {
                    match self.memory_store.recent(10).await {
                        Ok(r) => (r, "recent".to_string()),
                        Err(e) => return Some(Err(e)),
                    }
                } else {
                    match self.memory_store.search(query, 8, &[], None, None).await {
                        Ok((r, m)) => (r, m),
                        Err(e) => return Some(Err(e)),
                    }
                };
                if results.is_empty() {
                    return Some(Ok(s.memory_empty.to_string()));
                }
                let lines: Vec<String> = results.iter().enumerate().map(|(i, r)| {
                    let pin = if r.pinned { "📌 " } else { "" };
                    format!("{}{}. {}", pin, i + 1,
                        r.content.chars().take(200).collect::<String>())
                }).collect();
                Some(Ok(
                    localization::fmt(s.memory_header, &[&mode, &results.len().to_string(), &lines.join("\n\n")])
                ))
            }
            _ => None, // Unknown command — pass to LLM
        }
    }
}
