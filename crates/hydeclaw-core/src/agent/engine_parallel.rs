use crate::agent::tool_loop::{LoopDetector, LoopStatus};
use crate::tools::yaml_tools;
use hydeclaw_types::ToolCall;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

/// Tools that are safe for concurrent execution (read-only or independently stateful).
fn is_system_tool_parallel_safe(name: &str) -> bool {
    matches!(
        name,
        "web_fetch"
            | "memory"
            | "workspace_read"
            | "workspace_list"
            | "tool_list"
            | "skill"
            | "session"
            | "canvas"
            | "rich_card"
            // Inter-agent / subagent calls are I/O-bound and target independent agents —
            // safe to run concurrently (each agent has its own DB session + semaphore).
            | "subagent"
    )
}

pub(super) struct LoopBreak;

impl super::AgentEngine {
    /// Execute tool calls with parallel/sequential partitioning.
    /// Returns Vec<(tool_call_id, tool_result)> in original order.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_tool_calls_partitioned(
        &self,
        tool_calls: &[ToolCall],
        context: &Value,
        session_id: Uuid,
        channel: &str,
        current_context_chars: usize,
        detector: &mut LoopDetector,
        detect_loops: bool,
    ) -> Result<Vec<(String, String)>, LoopBreak> {
        let n = tool_calls.len();

        // 1. Loop detection on ALL tools first
        if detect_loops {
            for tc in tool_calls {
                match detector.record(&tc.name, &tc.arguments) {
                    LoopStatus::Ok => {}
                    LoopStatus::Warning(count) => {
                        tracing::warn!(tool = %tc.name, count, "possible tool loop detected");
                    }
                    LoopStatus::Break(reason) => {
                        tracing::error!(tool = %tc.name, reason = %reason, "tool loop broken");
                        return Err(LoopBreak);
                    }
                }
            }
        }

        // 2. Enrich args for all
        let enriched: Vec<Value> = tool_calls
            .iter()
            .map(|tc| Self::enrich_tool_args(&tc.arguments, context, session_id, channel))
            .collect();

        // 3. Load YAML tools with 30s cache (avoid per-batch disk reads)
        let yaml_tools = {
            let cache = self.yaml_tools_cache.read().await;
            if cache.0.elapsed() < std::time::Duration::from_secs(30) && !cache.1.is_empty() {
                cache.1.clone()
            } else {
                drop(cache);
                let tools: HashMap<String, yaml_tools::YamlToolDef> =
                    yaml_tools::load_yaml_tools(&self.workspace_dir, false)
                        .await
                        .into_iter()
                        .map(|t| (t.name.clone(), t))
                        .collect();
                *self.yaml_tools_cache.write().await =
                    (std::time::Instant::now(), tools.clone());
                tools
            }
        };

        // 4. Partition into parallel-safe and sequential
        let mut parallel_indices = Vec::new();
        let mut sequential_indices = Vec::new();
        for (i, tc) in tool_calls.iter().enumerate() {
            let is_parallel = if is_system_tool_parallel_safe(&tc.name) {
                true
            } else if self.needs_approval(&tc.name) {
                false
            } else if let Some(tool) = yaml_tools.get(&tc.name) {
                tool.parallel && tool.channel_action.is_none()
            } else {
                false
            };
            if is_parallel {
                parallel_indices.push(i);
            } else {
                sequential_indices.push(i);
            }
        }

        // 5. Execute
        let mut results: Vec<Option<String>> = vec![None; n];

        // 5a. Parallel batch (only if >1 parallel tool)
        if parallel_indices.len() > 1 {
            let futs: Vec<_> = parallel_indices
                .iter()
                .map(|&i| {
                    let name = tool_calls[i].name.clone();
                    let args = enriched[i].clone();
                    // Subagent tools get their configured timeout + 10s buffer so the outer cap
                    // does not kill a long-running subagent before its inner deadline fires.
                    let tool_timeout = if name == "subagent" {
                        let base = super::subagent_impl::parse_subagent_timeout(
                            &self.app_config.subagents.in_process_timeout,
                        );
                        base + std::time::Duration::from_secs(10)
                    } else {
                        std::time::Duration::from_secs(120)
                    };
                    async move {
                        match tokio::time::timeout(
                            tool_timeout,
                            self.execute_tool_call(&name, &args),
                        )
                        .await
                        {
                            Ok(result) => (i, self.truncate_tool_result(&result, current_context_chars)),
                            Err(_) => {
                                let secs = tool_timeout.as_secs();
                                tracing::warn!(tool = %name, timeout_secs = secs, "tool call timed out");
                                (i, format!("Tool '{}' timed out after {}s", name, secs))
                            }
                        }
                    }
                })
                .collect();

            let parallel_results = futures_util::future::join_all(futs).await;
            for (i, result) in parallel_results {
                results[i] = Some(result);
            }

            tracing::info!(
                count = parallel_indices.len(),
                "parallel tool batch completed"
            );
        } else if parallel_indices.len() == 1 {
            let i = parallel_indices[0];
            let name = &tool_calls[i].name;
            // Subagent tools get their configured timeout + 10s buffer.
            let tool_timeout = if name == "subagent" {
                let base = super::subagent_impl::parse_subagent_timeout(
                    &self.app_config.subagents.in_process_timeout,
                );
                base + std::time::Duration::from_secs(10)
            } else {
                std::time::Duration::from_secs(120)
            };
            let result = match tokio::time::timeout(
                tool_timeout,
                self.execute_tool_call(name, &enriched[i]),
            )
            .await
            {
                Ok(r) => self.truncate_tool_result(&r, current_context_chars),
                Err(_) => {
                    let secs = tool_timeout.as_secs();
                    tracing::warn!(tool = %name, timeout_secs = secs, "tool call timed out");
                    format!("Tool '{}' timed out after {}s", name, secs)
                }
            };
            results[i] = Some(result);
        }

        // 5b. Sequential
        for &i in &sequential_indices {
            let result = match tokio::time::timeout(
                std::time::Duration::from_secs(120),
                self.execute_tool_call(&tool_calls[i].name, &enriched[i]),
            )
            .await
            {
                Ok(r) => self.truncate_tool_result(&r, current_context_chars),
                Err(_) => {
                    tracing::warn!(tool = %tool_calls[i].name, "sequential tool call timed out after 120s");
                    format!("Tool '{}' timed out after 120s", tool_calls[i].name)
                }
            };
            results[i] = Some(result);
        }

        // 6. Reassemble in original order
        Ok(tool_calls
            .iter()
            .enumerate()
            .map(|(i, tc)| (tc.id.clone(), results[i].take().unwrap_or_default()))
            .collect())
    }
}
