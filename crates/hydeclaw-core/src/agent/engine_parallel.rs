use crate::agent::tool_loop::{LoopDetector, LoopStatus};
use crate::tools::yaml_tools;
use crate::tools::semantic_cache::SemanticCache;
use hydeclaw_types::ToolCall;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

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
            | "agent"
    )
}

fn is_tool_cacheable(name: &str) -> bool {
    matches!(
        name,
        "searxng_search" | "brave_search" | "browser_render" | "web_search"
    )
}

pub struct LoopBreak(pub Option<String>);

impl super::AgentEngine {
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
        let mut results: Vec<Option<String>> = vec![None; n];

        // 1. Loop detection PHASE 1 (Check limits BEFORE execution)
        if detect_loops {
            for tc in tool_calls {
                if let LoopStatus::Break(reason) = detector.check_limits(&tc.name, &tc.arguments) {
                    tracing::error!(tool = %tc.name, reason = %reason, "tool loop broken (pre-check)");
                    return Err(LoopBreak(Some(reason)));
                }
            }
        }

        // 2. Enrich args
        let enriched: Vec<Value> = tool_calls
            .iter()
            .map(|tc| Self::enrich_tool_args(&tc.arguments, context, session_id, channel))
            .collect();

        // 3. Semantic Cache Check
        for (i, tc) in tool_calls.iter().enumerate() {
            if is_tool_cacheable(&tc.name) && self.memory_store.is_available() {
                let query_text = tc.arguments.get("query").or_else(|| tc.arguments.get("url")).and_then(|v| v.as_str()).unwrap_or("");
                if !query_text.is_empty()
                    && let Ok(Some(cached_res)) = SemanticCache::check(&self.db, &self.memory_store, &tc.name, query_text, 0.95).await {
                    tracing::info!(tool = %tc.name, query = %query_text, "semantic cache hit");
                    results[i] = Some(cached_res);
                }
            }
        }

        // 4. Load YAML tools
        let yaml_tools = {
            let cache = self.tex().yaml_tools_cache.read().await;
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
                *self.tex().yaml_tools_cache.write().await = (std::time::Instant::now(), tools.clone());
                tools
            }
        };

        // 4. Partition (only those NOT found in cache)
        let mut parallel_indices = Vec::new();
        let mut sequential_indices = Vec::new();
        for (i, tc) in tool_calls.iter().enumerate() {
            if results[i].is_some() { continue; }
            
            let is_parallel = if is_system_tool_parallel_safe(&tc.name) {
                true
            } else if self.needs_approval(&tc.name) {
                false
            } else if let Some(tool) = yaml_tools.get(&tc.name) {
                tool.parallel && tool.channel_action.is_none()
            } else {
                false
            };
            if is_parallel { parallel_indices.push(i); } else { sequential_indices.push(i); }
        }

        // 5. Execute
        let subagent_timeout = super::subagent_impl::parse_subagent_timeout(&self.app_config.subagents.in_process_timeout) + std::time::Duration::from_secs(10);
        let default_timeout = std::time::Duration::from_secs(120);
        // Helpers for WAL logging
        let start_payload = |tc: &ToolCall| -> serde_json::Value { 
            serde_json::json!({ 
                "tool_call_id": tc.id, 
                "tool_name": tc.name,
                "args_hash": format!("{:x}", LoopDetector::hash_call_raw(&tc.name, &tc.arguments))
            }) 
        };
        let end_payload = |tc: &ToolCall, res: &str| -> serde_json::Value {
            let success = !res.to_lowercase().contains("error") && !res.to_lowercase().contains("failed");
            serde_json::json!({ 
                "tool_call_id": tc.id, 
                "tool_name": tc.name,
                "success": success
            })
        };

        // 5a. Parallel batch
        if !parallel_indices.is_empty() {
            for &i in &parallel_indices {
                let _ = crate::db::session_wal::log_event(&self.db, session_id, "tool_start", Some(&start_payload(&tool_calls[i]))).await;
            }

            let futs: Vec<_> = parallel_indices.iter().map(|&i| {
                let name = tool_calls[i].name.clone();
                let args = enriched[i].clone();
                let timeout = if name == "agent" { subagent_timeout } else { default_timeout };
                async move {
                    match tokio::time::timeout(timeout, self.execute_tool_call(&name, &args)).await {
                        Ok(r) => (i, self.truncate_tool_result(&r, current_context_chars)),
                        Err(_) => (i, format!("Tool '{}' timed out after {}s", name, timeout.as_secs())),
                    }
                }
            }).collect();

            for (i, result) in futures_util::future::join_all(futs).await {
                if detect_loops {
                    let success = !result.starts_with("Error:") && !result.starts_with("tool error:") && !result.contains("timed out");
                    detector.record_execution(&tool_calls[i].name, &tool_calls[i].arguments, success);
                }
                
                // Store in semantic cache if successful
                if is_tool_cacheable(&tool_calls[i].name) && !result.starts_with("Error:") && !result.starts_with("tool error:") {
                    let query_text = tool_calls[i].arguments.get("query").or_else(|| tool_calls[i].arguments.get("url")).and_then(|v| v.as_str()).unwrap_or("");
                    if !query_text.is_empty() {
                        let _ = SemanticCache::store(&self.db, &self.memory_store, &tool_calls[i].name, query_text, &result, 3600).await;
                    }
                }

                results[i] = Some(result.clone());
                let _ = crate::db::session_wal::log_event(&self.db, session_id, "tool_end", Some(&end_payload(&tool_calls[i], &result))).await;
            }
        }

        // 5b. Sequential
        for &i in &sequential_indices {
            let _ = crate::db::session_wal::log_event(&self.db, session_id, "tool_start", Some(&start_payload(&tool_calls[i]))).await;
            let res = match tokio::time::timeout(std::time::Duration::from_secs(120), self.execute_tool_call(&tool_calls[i].name, &enriched[i])).await {
                Ok(r) => self.truncate_tool_result(&r, current_context_chars),
                Err(_) => format!("Tool '{}' timed out after 120s", tool_calls[i].name),
            };
            if detect_loops {
                let success = !res.starts_with("Error:") && !res.starts_with("tool error:") && !res.contains("timed out");
                detector.record_execution(&tool_calls[i].name, &tool_calls[i].arguments, success);
            }

            // Store in semantic cache if successful
            if is_tool_cacheable(&tool_calls[i].name) && !res.starts_with("Error:") && !res.starts_with("tool error:") {
                let query_text = tool_calls[i].arguments.get("query").or_else(|| tool_calls[i].arguments.get("url")).and_then(|v| v.as_str()).unwrap_or("");
                if !query_text.is_empty() {
                    let _ = SemanticCache::store(&self.db, &self.memory_store, &tool_calls[i].name, query_text, &res, 3600).await;
                }
            }

            results[i] = Some(res.clone());
            let _ = crate::db::session_wal::log_event(&self.db, session_id, "tool_end", Some(&end_payload(&tool_calls[i], &res))).await;
        }

        // 6. Final reassemble
        Ok(tool_calls.iter().enumerate().map(|(i, tc)| (tc.id.clone(), results[i].take().unwrap_or_default())).collect())
    }
}
