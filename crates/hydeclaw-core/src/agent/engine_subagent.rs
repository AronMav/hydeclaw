//! Subagent, inter-agent, web-fetch, code-exec, tool-selection, and OpenAI handlers —
//! extracted from engine.rs for readability.
//! Easy helpers moved to `pipeline::subagent`; this file retains `run_subagent*` (heavy engine deps).

use super::*;
use crate::agent::subagent_state;

/// Sentinel prefix for subagent cancellation errors — matched in spawned task.
const SUBAGENT_CANCELLED: &str = "subagent cancelled";

// Re-export pipeline free functions for use within the engine module.
pub(crate) use crate::agent::pipeline::subagent::parse_subagent_timeout;
pub(super) use crate::agent::pipeline::subagent::SUBAGENT_DENIED_TOOLS;

impl AgentEngine {
    /// Enrich user text: auto-fetch URLs (max 2), add attachment descriptions.
    /// Delegates to the pipeline free function.
    pub(super) async fn enrich_message_text(
        &self,
        user_text: &str,
        attachments: &[hydeclaw_types::MediaAttachment],
    ) -> String {
        let toolgate_url = self.cfg().app_config.toolgate_url.clone()
            .unwrap_or_else(|| "http://localhost:9011".to_string());
        crate::agent::pipeline::subagent::enrich_message_text(
            self.http_client(),
            self.ssrf_http_client(),
            &self.cfg().app_config.gateway.listen,
            &toolgate_url,
            &self.cfg().agent.language,
            user_text,
            attachments,
        ).await
    }

    /// Fetch a URL and return text content.
    /// Delegates to the pipeline free function.
    pub(super) async fn handle_web_fetch(&self, args: &serde_json::Value) -> String {
        crate::agent::pipeline::subagent::handle_web_fetch(
            self.http_client(),
            self.ssrf_http_client(),
            &self.cfg().app_config.gateway.listen,
            args,
        ).await
    }

    /// Run an in-process subagent with isolated LLM context.
    pub async fn run_subagent(
        &self,
        task: &str,
        max_iterations: usize,
        deadline: Option<std::time::Instant>,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        handle: Option<Arc<tokio::sync::RwLock<subagent_state::SubagentHandle>>>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<String> {
        self.run_subagent_with_session(task, max_iterations, deadline, cancel, handle, allowed_tools, None).await
    }

    /// Like `run_subagent` but with an explicit session_id for tool context enrichment.
    /// When `session_id` is Some, it is passed to `execute_tool_calls_partitioned` so tools
    /// like `agent` can find the correct SessionAgentPool via enriched `_context`.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_subagent_with_session(
        &self,
        task: &str,
        max_iterations: usize,
        deadline: Option<std::time::Instant>,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
        handle: Option<Arc<tokio::sync::RwLock<subagent_state::SubagentHandle>>>,
        allowed_tools: Option<Vec<String>>,
        session_id: Option<uuid::Uuid>,
    ) -> Result<String> {
        let ws_prompt =
            workspace::load_workspace_prompt(&self.cfg().workspace_dir, &self.cfg().agent.name).await?;
        let capabilities = workspace::CapabilityFlags {
            has_search: self.has_tool("search_web").await || self.has_tool("search_web_fresh").await,
            has_memory: self.cfg().memory_store.is_available(),
            has_message_actions: self.state().channel_router.is_some(),
            has_cron: self.cfg().scheduler.is_some(),
            has_yaml_tools: true,
            has_browser: Self::browser_renderer_url() != "disabled",
            has_host_exec: self.cfg().agent.base && self.sandbox().is_none(),
            is_base: self.cfg().agent.base,
        };
        let runtime = workspace::RuntimeContext {
            agent_name: self.cfg().agent.name.clone(),
            owner_id: self.cfg().agent.access.as_ref().and_then(|a| a.owner_id.clone()),
            channel: "agent".to_string(),
            model: self.cfg().provider.current_model(),
            datetime_display: workspace::format_local_datetime(&self.cfg().default_timezone),
            formatting_prompt: None,
            channels: vec![],
        };
        let system_prompt =
            workspace::build_system_prompt(&ws_prompt, &[], &capabilities, &self.cfg().agent.language, &runtime);

        let mut messages = vec![
            Message {
                role: MessageRole::System,
                content: format!(
                    "{}\n\nYou are a subagent. Complete the task and return the result concisely.",
                    system_prompt
                ),
                tool_calls: None,
                tool_call_id: None,
                thinking_blocks: vec![],
            },
            Message {
                role: MessageRole::User,
                content: task.to_string(),
                tool_calls: None,
                tool_call_id: None,
                thinking_blocks: vec![],
            },
        ];

        let mut available_tools = self.internal_tool_definitions_for_subagent(allowed_tools.as_deref());
        let yaml_tools: Vec<crate::tools::yaml_tools::YamlToolDef> = {
            let cache = self.tex().yaml_tools_cache.read().await;
            if cache.0.elapsed() < std::time::Duration::from_secs(30) && !cache.1.is_empty() {
                cache.1.values().cloned().collect()
            } else {
                drop(cache);
                let loaded = crate::tools::yaml_tools::load_yaml_tools(&self.cfg().workspace_dir, false).await;
                let map: std::collections::HashMap<String, crate::tools::yaml_tools::YamlToolDef> =
                    loaded.iter().cloned().map(|t| (t.name.clone(), t)).collect();
                *self.tex().yaml_tools_cache.write().await = (std::time::Instant::now(), std::sync::Arc::new(map));
                loaded
            }
        };
        available_tools.extend(yaml_tools.into_iter().map(|t| t.to_tool_definition()));
        if let Some(mcp) = self.mcp() {
            available_tools.extend(mcp.all_tool_definitions().await);
        }
        available_tools = self.filter_tools_by_policy(available_tools);

        let loop_config = self.tool_loop_config();
        let effective_max = max_iterations.min(loop_config.effective_max_iterations());
        let mut detector = LoopDetector::new(&loop_config);
        let mut loop_nudge_count: usize = 0;

        for iteration in 0..effective_max {
            // Cancel check
            if let Some(ref c) = cancel
                && c.load(std::sync::atomic::Ordering::Relaxed) {
                    tracing::info!(iteration, "subagent cancelled by parent");
                    anyhow::bail!("{} at iteration {}", SUBAGENT_CANCELLED, iteration);
                }
            // Deadline check (only if set)
            if let Some(dl) = deadline
                && std::time::Instant::now() > dl {
                    tracing::warn!(iteration, "subagent deadline reached, returning partial result");
                    let forced = self.cfg().provider.chat(&messages, &[]).await?;
                    return Ok(strip_thinking(&forced.content));
                }

            let response = if loop_config.compact_on_overflow {
                self.chat_with_overflow_recovery(&mut messages, &available_tools).await?
            } else {
                self.cfg().provider.chat(&messages, &available_tools).await?
            };

            if response.tool_calls.is_empty() {
                return Ok(strip_thinking(&response.content));
            }

            tracing::info!(
                iteration,
                max = effective_max,
                tools = response.tool_calls.len(),
                "subagent executing tool calls"
            );

            messages.push(Message {
                role: MessageRole::Assistant,
                content: response.content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
                thinking_blocks: vec![],
            });

            // Use an empty object (not Null) so enrich_tool_args can inject session_id into _context.
            let effective_session_id = session_id.unwrap_or_else(uuid::Uuid::nil);
            let subagent_context = serde_json::json!({});
            let loop_broken = match self.execute_tool_calls_partitioned(
                &response.tool_calls, &subagent_context, effective_session_id, crate::agent::channel_kind::channel::INTER_AGENT,
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
                    }
                    false
                }
                Err(LoopBreak(reason)) => {
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
                            nudge_count = loop_nudge_count,
                            reason = ?reason,
                            "subagent loop nudge injected"
                        );
                        false // continue loop
                    } else {
                        tracing::error!(
                            nudge_count = loop_nudge_count,
                            "subagent max loop nudges reached, force-stopping"
                        );
                        true // broken
                    }
                }
            };

            // Log iteration to handle (if managed)
            if let Some(ref h) = handle {
                let tool_names: Vec<String> = response.tool_calls.iter().map(|tc| tc.name.clone()).collect();
                let preview: String = response.content.chars().take(200).collect();
                let mut hh = h.write().await;
                hh.log.push(subagent_state::SubagentLogEntry {
                    iteration,
                    timestamp: chrono::Utc::now(),
                    tool_calls: tool_names,
                    content_preview: preview,
                });
            }

            if loop_broken || iteration == effective_max - 1 {
                let forced = self.cfg().provider.chat(&messages, &[]).await?;
                return Ok(strip_thinking(&forced.content));
            }
        }

        anyhow::bail!("subagent exceeded max iterations")
    }
}

// -- tool selection --

impl AgentEngine {
    /// Select top-K tools using embedding-based cosine similarity.
    /// Delegates to the pipeline free function.
    pub(super) async fn select_top_k_tools_semantic(
        &self,
        tools: Vec<hydeclaw_types::ToolDefinition>,
        query: &str,
        k: usize,
    ) -> Vec<hydeclaw_types::ToolDefinition> {
        crate::agent::pipeline::subagent::select_top_k_tools_semantic(
            self.cfg().embedder.as_ref(),
            self.tool_embed_cache().as_ref(),
            self.cfg().memory_store.is_available(),
            tools,
            query,
            k,
        ).await
    }

}
