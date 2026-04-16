//! Subagent, inter-agent, web-fetch, code-exec, tool-selection, and OpenAI handlers —
//! extracted from engine.rs for readability.

use super::*;
use crate::agent::subagent_state;

/// Sentinel prefix for subagent cancellation errors — matched in spawned task.
const SUBAGENT_CANCELLED: &str = "subagent cancelled";

/// Parse a duration string like "2m", "30s" for subagent timeout.
/// Defaults to 2m (120s) on invalid input — matches the config default.
pub(crate) fn parse_subagent_timeout(s: &str) -> std::time::Duration {
    let s = s.trim();
    if let Some(mins) = s.strip_suffix('m')
        && let Ok(n) = mins.parse::<u64>() {
        return std::time::Duration::from_secs(n * 60);
    }
    if let Some(secs) = s.strip_suffix('s')
        && let Ok(n) = secs.parse::<u64>() {
        return std::time::Duration::from_secs(n);
    }
    std::time::Duration::from_secs(120) // default 2m
}

/// Tools denied to subagents by default (prevent recursive spawning, destructive operations, and dangerous ops).
/// workspace_write and workspace_edit are allowed so subagents can write shared state files (SUB-01).
pub(super) const SUBAGENT_DENIED_TOOLS: &[&str] = &[
    "workspace_delete",
    "workspace_rename", "cron", "secret_set", "process",
];

impl AgentEngine {
    /// Fetch URL content, extract readable text, truncate for LLM context.
    /// Uses 10s timeout to avoid blocking message processing on slow URLs.
    pub(super) async fn fetch_url_content(&self, url: &str) -> Result<String> {
        // Only allow localhost on Core API port — block access to internal services.
        // Parse port from gateway listen address (e.g. "0.0.0.0:18789" → 18789)
        let core_port = self.app_config.gateway.listen.rsplit(':').next()
            .and_then(|p| p.parse::<u16>().ok()).unwrap_or(18789);
        let is_core_api = url.starts_with(&format!("http://localhost:{}", core_port))
            || url.starts_with(&format!("http://127.0.0.1:{}", core_port));

        if !is_core_api {
            // SSRF protection: scheme + internal blocklist check (sync),
            // private IP blocking is handled by ssrf_http_client's DNS resolver.
            crate::tools::ssrf::validate_url_scheme(url)?;
        }

        let client = if is_core_api { self.http_client() } else { self.ssrf_http_client() };
        let resp = client
            .get(url)
            .header("User-Agent", "HydeClaw/0.1 (link-preview)")
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("HTTP {}", resp.status());
        }

        // Skip non-HTML content (PDFs, images, etc.)
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.contains("text/html") && !ct.contains("text/plain") {
            anyhow::bail!("non-HTML content: {}", ct);
        }

        // OOM guard: reject responses larger than 512KB before reading into memory
        const BODY_LIMIT: u64 = 512 * 1024;
        if let Some(cl) = resp.content_length()
            && cl > BODY_LIMIT {
                anyhow::bail!("response too large ({} bytes, limit {})", cl, BODY_LIMIT);
            }

        let body = resp.text().await?;
        // Truncate body if Content-Length was missing/inaccurate
        let body = if body.len() > BODY_LIMIT as usize {
            let boundary = body.floor_char_boundary(BODY_LIMIT as usize);
            body[..boundary].to_string()
        } else {
            body
        };

        // Extract readable content (removes nav, header, footer, ads, etc.)
        let text = extract_readable_text(&body);

        // Truncate to ~4000 bytes for LLM context (safe UTF-8 boundary)
        let truncated = if text.len() > 4000 {
            let boundary = text.floor_char_boundary(4000);
            format!("{}...\n[truncated, {} characters total]", &text[..boundary], text.chars().count())
        } else {
            text
        };

        Ok(crate::tools::content_security::wrap_external_content(&truncated, &format!("web_fetch:{}", url)))
    }

    /// Enrich user text: auto-fetch URLs (max 2), add attachment descriptions.
    pub(super) async fn enrich_message_text(
        &self,
        user_text: &str,
        attachments: &[hydeclaw_types::MediaAttachment],
    ) -> String {
        let mut enriched = user_text.to_string();

        // PII redaction before sending to external LLM
        let (redacted, pii_count) = crate::agent::pii::redact(&enriched);
        if pii_count > 0 {
            tracing::info!(count = pii_count, "redacted PII from user message");
            enriched = redacted;
        }

        let urls: Vec<String> = extract_urls(user_text);
        for url in urls.iter().take(2) {
            match self.fetch_url_content(url).await {
                Ok(content) => {
                    tracing::info!(url = %url, len = content.len(), "fetched URL content");
                    enriched.push_str(&format!("\n\n[Content of URL {}]:\n{}", url, content));
                }
                Err(e) => {
                    tracing::warn!(url = %url, error = %e, "failed to fetch URL");
                }
            }
        }
        enrich_with_attachments(&mut enriched, attachments);

        // Auto-transcribe voice messages via toolgate STT
        let toolgate_url = self.app_config.toolgate_url.clone()
            .unwrap_or_else(|| "http://localhost:9011".to_string());
        crate::agent::url_tools::auto_transcribe_audio(
            &mut enriched, attachments, &toolgate_url, &self.agent.language, self.http_client(),
        ).await;

        enriched
    }

    // handle_spawn_subagent, handle_subagent_status, handle_subagent_logs, handle_subagent_kill
    // were removed — the old `subagent` tool dispatch no longer calls them.
    // run_subagent() below is still used by spawn_live_agent() in session_agent_pool.rs.

    /// Fetch a URL and return text content.
    pub(super) async fn handle_web_fetch(&self, args: &serde_json::Value) -> String {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return "Error: 'url' parameter is required.".to_string(),
        };
        let max_length = args
            .get("max_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(50000) as usize;

        tracing::info!(url = %url, "web_fetch: fetching URL");

        // Determine if this is a local Core API call (e.g., /api/doctor).
        // Only allow localhost on Core API port (18789) — block access to internal services
        // like toolgate (9011), postgres, redis, etc.
        // Parse port from gateway listen address (e.g. "0.0.0.0:18789" → 18789)
        let core_port = self.app_config.gateway.listen.rsplit(':').next()
            .and_then(|p| p.parse::<u16>().ok()).unwrap_or(18789);
        let is_core_api = url.starts_with(&format!("http://localhost:{}", core_port))
            || url.starts_with(&format!("http://127.0.0.1:{}", core_port));

        if !is_core_api {
            // SSRF protection: scheme + internal blocklist (sync);
            // private IP blocking via ssrf_http_client's DNS resolver.
            if let Err(e) = crate::tools::ssrf::validate_url_scheme(url) {
                return format!("Error: {}", e);
            }
        }

        let client = if is_core_api { self.http_client() } else { self.ssrf_http_client() };
        let resp = match client.get(url)
            .header("User-Agent", "HydeClaw/1.0")
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return format!("Error fetching URL: {}", e),
        };

        if !resp.status().is_success() {
            return format!("HTTP error {}", resp.status());
        }

        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Guard against unbounded response bodies (OOM protection).
        // Allow 2x max_length to account for HTML tags stripped during extraction.
        let body_limit = max_length * 2;
        if let Some(cl) = resp.content_length()
            && cl as usize > body_limit {
                return format!("Error: response too large ({} bytes, limit {})", cl, body_limit);
            }

        let body = match resp.text().await {
            Ok(t) if t.len() > body_limit => {
                let boundary = t.floor_char_boundary(body_limit);
                t[..boundary].to_string()
            }
            Ok(t) => t,
            Err(e) => return format!("Error reading response: {}", e),
        };

        // Extract readable text from HTML; pass through JSON/plain text as-is
        let text = if ct.contains("text/html") {
            extract_readable_text(&body)
        } else {
            body
        };

        // Truncate if too long (safe UTF-8 boundary)
        let trimmed = if text.len() > max_length {
            let boundary = text.floor_char_boundary(max_length);
            format!("{}...\n\n[Truncated at {} chars, total {}]", &text[..boundary], max_length, text.len())
        } else {
            text
        };

        // Wrap in content-security boundary to mitigate prompt injection
        crate::tools::content_security::wrap_external_content(&trimmed, &format!("web_fetch:{}", url))
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
            workspace::load_workspace_prompt(&self.workspace_dir, &self.agent.name).await?;
        let capabilities = workspace::CapabilityFlags {
            has_search: self.has_tool("search_web").await || self.has_tool("search_web_fresh").await,
            has_memory: self.memory_store.is_available(),
            has_message_actions: self.channel_router.is_some(),
            has_cron: self.scheduler.is_some(),
            has_yaml_tools: true,
            has_browser: Self::browser_renderer_url() != "disabled",
            has_host_exec: self.agent.base && self.sandbox().is_none(),
            is_base: self.agent.base,
        };
        let runtime = workspace::RuntimeContext {
            agent_name: self.agent.name.clone(),
            owner_id: self.agent.access.as_ref().and_then(|a| a.owner_id.clone()),
            channel: "agent".to_string(),
            model: self.provider.current_model(),
            datetime_display: workspace::format_local_datetime(&self.default_timezone),
            formatting_prompt: None,
            channels: vec![],
        };
        let system_prompt =
            workspace::build_system_prompt(&ws_prompt, &[], &capabilities, &self.agent.language, &runtime);

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
                let loaded = crate::tools::yaml_tools::load_yaml_tools(&self.workspace_dir, false).await;
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
                    let forced = self.provider.chat(&messages, &[]).await?;
                    return Ok(strip_thinking(&forced.content));
                }

            let response = if loop_config.compact_on_overflow {
                self.chat_with_overflow_recovery(&mut messages, &available_tools).await?
            } else {
                self.provider.chat(&messages, &available_tools).await?
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
                Err(super::parallel_impl::LoopBreak(reason)) => {
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
                let forced = self.provider.chat(&messages, &[]).await?;
                return Ok(strip_thinking(&forced.content));
            }
        }

        anyhow::bail!("subagent exceeded max iterations")
    }
}

// -- tool selection and OpenAI handler --

impl AgentEngine {
    // ── git handlers ──────────────────────────────────────────────────────────

    /// Select top-K tools using embedding-based cosine similarity.
    /// Falls back to keyword scoring when the embedding service is unavailable.
    pub(super) async fn select_top_k_tools_semantic(
        &self,
        tools: Vec<hydeclaw_types::ToolDefinition>,
        query: &str,
        k: usize,
    ) -> Vec<hydeclaw_types::ToolDefinition> {
        // Always include core tools
        const ALWAYS_INCLUDE: &[&str] = &[
            "workspace_read", "workspace_write", "workspace_edit", "workspace_list", "workspace_delete", "workspace_rename",
            "memory", "code_exec", "agent",
            "tool_create", "tool_list", "tool_test", "tool_verify", "tool_disable",
            "skill", "git",
            // UI-critical tools: must never be filtered out by top-K selection
            "canvas", "rich_card", "web_fetch",
            // Agent interaction & system tools
            "message", "cron", "session", "agents_list", "browser_action",
            "process", "secret_set", "skill_use", "tool_discover",
        ];

        let mut always = Vec::new();
        let mut candidates: Vec<hydeclaw_types::ToolDefinition> = Vec::new();
        for tool in tools {
            if ALWAYS_INCLUDE.contains(&tool.name.as_str()) {
                always.push(tool);
            } else {
                candidates.push(tool);
            }
        }

        let remaining_slots = k.saturating_sub(always.len());
        if remaining_slots == 0 || candidates.is_empty() {
            return always;
        }

        // Try embedding-based selection if memory store is available
        if self.memory_store.is_available() {
            match self.select_by_embedding(&candidates, query, remaining_slots).await {
                Ok(selected) => {
                    tracing::debug!(
                        total = always.len() + selected.len(),
                        k,
                        method = "embedding",
                        "semantic top-K tool selection applied"
                    );
                    let mut result = always;
                    result.extend(selected);
                    return result;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "embedding unavailable, falling back to keyword scoring");
                }
            }
        }

        // Fallback: keyword scoring
        let selected = select_top_k_by_keywords(candidates, query, remaining_slots);
        tracing::debug!(
            total = always.len() + selected.len(),
            k,
            method = "keyword",
            "keyword top-K tool selection applied"
        );
        let mut result = always;
        result.extend(selected);
        result
    }

    /// Score tools by cosine similarity against the query embedding.
    pub(super) async fn select_by_embedding(
        &self,
        tools: &[hydeclaw_types::ToolDefinition],
        query: &str,
        k: usize,
    ) -> anyhow::Result<Vec<hydeclaw_types::ToolDefinition>> {
        let query_vec = self.embedder.embed(query).await?;

        let mut scored: Vec<(f32, usize)> = Vec::with_capacity(tools.len());
        for (idx, tool) in tools.iter().enumerate() {
            let tool_text = format!("{} {}", tool.name, tool.description);
            let cache_key = format!("tool::{}", tool.name);
            let tool_vec = self
                .tool_embed_cache()
                .get_or_embed(&cache_key, &tool_text, self.embedder.as_ref())
                .await?;
            let sim = crate::tools::embedding::cosine_similarity(&query_vec, &tool_vec);
            scored.push((sim, idx));
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);

        let result = scored
            .into_iter()
            .map(|(_, idx)| tools[idx].clone())
            .collect();
        Ok(result)
    }

}

/// Keyword-based top-K fallback (original algorithm).
fn select_top_k_by_keywords(
    tools: Vec<hydeclaw_types::ToolDefinition>,
    query: &str,
    k: usize,
) -> Vec<hydeclaw_types::ToolDefinition> {
    let query_words: Vec<String> = query
        .split_whitespace()
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_lowercase())
        .collect();

    let mut scored: Vec<(usize, hydeclaw_types::ToolDefinition)> = tools
        .into_iter()
        .map(|t| {
            let haystack = format!("{} {}", t.name, t.description).to_lowercase();
            let score = query_words.iter().filter(|w| haystack.contains(w.as_str())).count();
            (score, t)
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.truncate(k);
    scored.into_iter().map(|(_, t)| t).collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    // ── select_top_k_by_keywords ─────────────────────────────────────────────

    fn make_tool(name: &str, description: &str) -> hydeclaw_types::ToolDefinition {
        hydeclaw_types::ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: serde_json::json!({}),
        }
    }

    #[test]
    fn select_top_k_empty_tools_returns_empty() {
        let result = select_top_k_by_keywords(vec![], "search web", 5);
        assert!(result.is_empty());
    }

    #[test]
    fn select_top_k_returns_top_two_by_keyword_match() {
        let tools = vec![
            make_tool("web_search", "search the web for information"),
            make_tool("weather_get", "get current weather data"),
            make_tool("calculator", "perform arithmetic calculations"),
        ];
        let result = select_top_k_by_keywords(tools, "search web information", 2);
        assert_eq!(result.len(), 2);
        // web_search matches 3 words; weather_get matches 0; calculator matches 0
        assert_eq!(result[0].name, "web_search");
    }

    #[test]
    fn select_top_k_short_words_ignored() {
        let tools = vec![
            make_tool("web_search", "search the web"),
            make_tool("do_it", "do it now"),
        ];
        // "do" and "it" are <3 chars, should not contribute to score
        let result = select_top_k_by_keywords(tools, "do it", 2);
        // Neither tool matches; order is stable from sort, but both have score 0
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn select_top_k_no_matches_returns_up_to_k_tools() {
        let tools = vec![
            make_tool("alpha", "does alpha things"),
            make_tool("beta", "does beta things"),
            make_tool("gamma", "does gamma things"),
        ];
        // Query matches nothing — all score 0, still returns up to k
        let result = select_top_k_by_keywords(tools, "zzz yyy xxx", 2);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn denied_tools_list_contains_critical_entries() {
        // Safety: subagent, workspace_delete, workspace_rename, cron must always be denied
        // "agent" is NOT denied — pool agents need it for peer-to-peer communication.
        // Session context is provided via enriched _context.
        assert!(!SUBAGENT_DENIED_TOOLS.contains(&"agent"));
        assert!(SUBAGENT_DENIED_TOOLS.contains(&"workspace_delete"));
        assert!(SUBAGENT_DENIED_TOOLS.contains(&"workspace_rename"));
        assert!(SUBAGENT_DENIED_TOOLS.contains(&"cron"));
        assert!(SUBAGENT_DENIED_TOOLS.contains(&"secret_set"));
        assert!(SUBAGENT_DENIED_TOOLS.contains(&"process"));
    }

    #[test]
    fn denied_tools_do_not_block_safe_tools() {
        assert!(!SUBAGENT_DENIED_TOOLS.contains(&"memory"));
        assert!(!SUBAGENT_DENIED_TOOLS.contains(&"web_fetch"));
        assert!(!SUBAGENT_DENIED_TOOLS.contains(&"workspace_read"));
        assert!(!SUBAGENT_DENIED_TOOLS.contains(&"workspace_list"));
        // SUB-01: workspace_write and workspace_edit unlocked for subagents
        assert!(!SUBAGENT_DENIED_TOOLS.contains(&"workspace_write"));
        assert!(!SUBAGENT_DENIED_TOOLS.contains(&"workspace_edit"));
    }

    // ── parse_subagent_timeout ───────────────────────────────────────────────

    #[test]
    fn parse_subagent_timeout_minutes() {
        assert_eq!(parse_subagent_timeout("2m"), std::time::Duration::from_secs(120));
    }

    #[test]
    fn parse_subagent_timeout_seconds() {
        assert_eq!(parse_subagent_timeout("30s"), std::time::Duration::from_secs(30));
    }

    #[test]
    fn parse_subagent_timeout_invalid_defaults() {
        assert_eq!(parse_subagent_timeout("invalid"), std::time::Duration::from_secs(120));
    }

    #[test]
    fn parse_subagent_timeout_whitespace() {
        assert_eq!(parse_subagent_timeout(" 5m "), std::time::Duration::from_secs(300));
    }
}

