//! Subagent, inter-agent, web-fetch, code-exec, tool-selection, and OpenAI handlers —
//! extracted from engine.rs for readability.

use super::*;
use crate::agent::subagent_state;

/// Sentinel prefix for subagent cancellation errors — matched in spawned task.
const SUBAGENT_CANCELLED: &str = "subagent cancelled";

/// Maximum number of retry attempts for failed subagents (not counting the first attempt).
const MAX_SUBAGENT_RETRIES: usize = 2;

/// Determine whether a subagent result JSON string warrants an automatic retry.
///
/// Returns `true` if the result represents a transient failure (network, LLM, timeout, channel drop).
/// Returns `false` for successes or explicit cancellations — those must not be retried.
fn is_subagent_result_retryable(result_str: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(result_str) else {
        // Unparseable — treat as unknown error, retry
        return true;
    };
    // Success: never retry
    if value.get("status").and_then(|v| v.as_str()) == Some("ok") {
        return false;
    }
    // Explicit cancellation by parent: never retry
    if value.get("error").and_then(|v| v.as_str()).is_some_and(|e| e.contains("cancelled by parent")) {
        return false;
    }
    true
}

/// Inject `retry_count` field into a subagent result JSON string.
/// If parsing fails, wraps the raw string in a new JSON object with retry_count.
fn augment_with_retry_count(result_str: &str, retry_count: usize) -> String {
    match serde_json::from_str::<serde_json::Value>(result_str) {
        Ok(mut value) => {
            if let Some(obj) = value.as_object_mut() {
                obj.insert("retry_count".to_string(), serde_json::json!(retry_count));
            }
            value.to_string()
        }
        Err(_) => {
            serde_json::json!({
                "status": "error",
                "output": "",
                "error": result_str,
                "retry_count": retry_count,
            }).to_string()
        }
    }
}

/// Parse a duration string like "2m", "30s" for subagent timeout.
/// Defaults to 2m (120s) on invalid input — matches the config default.
pub(super) fn parse_subagent_timeout(s: &str) -> std::time::Duration {
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
    "subagent", "workspace_delete",
    "workspace_rename", "cron", "secret_set", "process",
];

impl AgentEngine {
    /// Fetch URL content, extract readable text, truncate for LLM context.
    /// Uses 10s timeout to avoid blocking message processing on slow URLs.
    pub(super) async fn fetch_url_content(&self, url: &str) -> Result<String> {
        // Local Core API calls bypass SSRF filtering (same as web_fetch tool).
        let is_local = url.starts_with("http://localhost:") || url.starts_with("http://127.0.0.1:");

        if !is_local {
            // SSRF protection: scheme + internal blocklist check (sync),
            // private IP blocking is handled by ssrf_http_client's DNS resolver.
            crate::tools::ssrf::validate_url_scheme(url)?;
        }

        let client = if is_local { &self.http_client } else { &self.ssrf_http_client };
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
            &mut enriched, attachments, &toolgate_url, &self.agent.language, &self.http_client,
        ).await;

        enriched
    }

    /// Spawn an async subagent or wait for an existing one.
    pub(super) async fn handle_spawn_subagent(&self, args: &serde_json::Value) -> String {
        // Clean up old finished subagents (older than 1 hour)
        self.subagent_registry.cleanup(chrono::Duration::hours(1)).await;

        let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
        let allowed_tools: Option<Vec<String>> = args.get("allowed_tools")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        // Lookup mode: return stored result from registry (subagent completed via push notification)
        if let Some(sid) = args.get("subagent_id").and_then(|v| v.as_str())
            && !sid.is_empty() {
                return match self.subagent_registry.get(sid).await {
                    Some(h) => {
                        let h = h.read().await;
                        match (&h.status, &h.result, &h.error) {
                            (subagent_state::SubagentStatus::Running, _, _) =>
                                serde_json::json!({"status": "running", "output": "", "error": "still running"}).to_string(),
                            (subagent_state::SubagentStatus::Completed, Some(r), _) =>
                                serde_json::json!({"status": "ok", "output": r}).to_string(),
                            (_, _, Some(e)) =>
                                serde_json::json!({"status": "error", "output": "", "error": e}).to_string(),
                            _ =>
                                serde_json::json!({"status": "error", "output": "", "error": "no result"}).to_string(),
                        }
                    }
                    None => format!("Error: subagent '{}' not found", sid),
                };
            }

        if task.is_empty() {
            return "Error: 'task' is required (or provide subagent_id to wait for existing)".to_string();
        }

        // Resolve engine Arc once before the retry loop (same engine for all attempts)
        let engine = match self.self_ref.get().and_then(std::sync::Weak::upgrade) {
            Some(arc) => arc,
            None => {
                return "Error: engine self_ref not set, cannot spawn async subagent".to_string();
            }
        };

        let task_owned = task.to_string();
        let loop_max = self.tool_loop_config().effective_max_iterations();
        let timeout_dur = parse_subagent_timeout(&self.app_config.subagents.in_process_timeout);
        let task_preview: String = task.chars().take(80).collect();

        let mut last_result_str = String::new();
        let mut last_id = String::new();

        for attempt in 0..=MAX_SUBAGENT_RETRIES {
            if attempt > 0 {
                tracing::warn!(
                    subagent_task_len = task.len(),
                    attempt,
                    max_retries = MAX_SUBAGENT_RETRIES,
                    "retrying failed subagent"
                );
            }

            // Acquire a fresh owned permit each attempt (moves into tokio::spawn).
            // Semaphore exhaustion is not a transient error — break without retrying.
            let permit = match self.subagent_semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    return "Error: too many concurrent subagents. Use subagent_status to check.".to_string();
                }
            };

            // Clone allowed_tools for this attempt (consumed by spawn closure)
            let allowed_tools_attempt = allowed_tools.clone();

            let (id, handle, cancel, completion_rx) = self.subagent_registry.register(task).await;
            last_id = id.clone();

            tracing::info!(subagent_id = %id, task_len = task.len(), attempt, "spawning async subagent");

            let handle_clone = handle.clone();
            let engine_clone = engine.clone();
            let task_for_spawn = task_owned.clone();
            let deadline = Some(std::time::Instant::now() + timeout_dur);

            tokio::spawn(async move {
                let _permit = permit; // held until task completes
                let result = tokio::time::timeout(
                    timeout_dur,
                    engine_clone.run_subagent(
                        &task_for_spawn, loop_max, deadline, Some(cancel), Some(handle_clone.clone()), allowed_tools_attempt,
                    ),
                ).await;
                let mut h = handle_clone.write().await;
                h.finished_at = Some(chrono::Utc::now());
                match result {
                    Err(_elapsed) => {
                        // Hard timeout from tokio::time::timeout
                        h.status = subagent_state::SubagentStatus::Failed;
                        h.error = Some("timeout".to_string());
                    }
                    Ok(Ok(text)) => {
                        h.status = subagent_state::SubagentStatus::Completed;
                        h.result = Some(text);
                    }
                    Ok(Err(e)) if e.to_string().contains(SUBAGENT_CANCELLED) => {
                        h.status = subagent_state::SubagentStatus::Killed;
                        h.error = Some("cancelled by parent".to_string());
                    }
                    Ok(Err(e)) => {
                        h.status = subagent_state::SubagentStatus::Failed;
                        h.error = Some(e.to_string());
                    }
                }
                // Build push result and fire the oneshot — take() BEFORE drop(h) to avoid holding write lock
                let sub_result = subagent_state::SubagentResult {
                    status: h.status,
                    result: h.result.clone(),
                    error: h.error.clone(),
                };
                let maybe_tx = h.completion_tx.take();
                drop(h); // Release write lock before sending
                if let Some(tx) = maybe_tx {
                    let _ = tx.send(sub_result);
                }
            });

            // Block until subagent completes or times out
            let (result_str, succeeded) = match tokio::time::timeout(timeout_dur, completion_rx).await {
                Ok(Ok(sub_result)) => {
                    let ok = sub_result.status == subagent_state::SubagentStatus::Completed;
                    let s = match (&sub_result.status, &sub_result.result, &sub_result.error) {
                        (subagent_state::SubagentStatus::Completed, Some(r), _) =>
                            serde_json::json!({"status": "ok", "output": r}).to_string(),
                        (_, _, Some(e)) =>
                            serde_json::json!({"status": "error", "output": "", "error": e}).to_string(),
                        _ =>
                            serde_json::json!({"status": "error", "output": "", "error": "no result"}).to_string(),
                    };
                    (s, ok)
                }
                Ok(Err(_)) =>
                    (serde_json::json!({"status": "error", "output": "", "error": "subagent channel dropped"}).to_string(), false),
                Err(_) =>
                    (serde_json::json!({"status": "error", "output": "", "error": "timeout"}).to_string(), false),
            };

            // Classify result — success and explicit cancellations do not retry
            if !is_subagent_result_retryable(&result_str) {
                // Augment JSON with retry_count when retries were needed (success after retry)
                let final_str = if attempt > 0 {
                    augment_with_retry_count(&result_str, attempt)
                } else {
                    result_str
                };
                if let Some(ref tx) = *self.sse_event_tx.lock().await {
                    let status_str = if succeeded { "completed" } else { "failed" };
                    let _ = tx.send(super::StreamEvent::RichCard {
                        card_type: "subagent-complete".to_string(),
                        data: serde_json::json!({
                            "subagent_id": last_id,
                            "status": status_str,
                            "task_preview": task_preview,
                            "retry_count": attempt,
                        }),
                    });
                }
                return final_str;
            }

            // Retryable failure — store and continue unless we've exhausted attempts
            last_result_str = result_str;
            if attempt == MAX_SUBAGENT_RETRIES {
                break;
            }
        }

        // All retries exhausted — inject retry_count into the final error JSON
        let final_str = augment_with_retry_count(&last_result_str, MAX_SUBAGENT_RETRIES);

        // Emit SSE event once for final failure
        if let Some(ref tx) = *self.sse_event_tx.lock().await {
            let _ = tx.send(super::StreamEvent::RichCard {
                card_type: "subagent-complete".to_string(),
                data: serde_json::json!({
                    "subagent_id": last_id,
                    "status": "failed",
                    "task_preview": task_preview,
                    "retry_count": MAX_SUBAGENT_RETRIES,
                }),
            });
        }
        final_str
    }

    /// Check status of one or all subagents.
    pub(super) async fn handle_subagent_status(&self, args: &serde_json::Value) -> String {
        if let Some(id) = args.get("subagent_id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            match self.subagent_registry.get(id).await {
                Some(h) => {
                    let h = h.read().await;
                    let dur = (h.finished_at.unwrap_or_else(chrono::Utc::now) - h.started_at).num_seconds();
                    serde_json::json!({
                        "id": h.id, "status": h.status, "iterations": h.log.len(),
                        "duration_secs": dur, "task": h.task,
                        "result_preview": h.result.as_deref().map(|r| &r[..r.floor_char_boundary(300)]),
                    }).to_string()
                }
                None => format!("Subagent '{}' not found (registry is in-memory — cleared on service restart; the subagent was likely killed by restart)", id),
            }
        } else {
            let all = self.subagent_registry.list_summary().await;
            if all.is_empty() { return "No active subagents (registry is empty — subagents from before a service restart are gone).".to_string(); }
            all.iter().map(|h| {
                let dur = (h.finished_at.unwrap_or_else(chrono::Utc::now) - h.started_at).num_seconds();
                format!("- {} {:?} ({}s, {} iters): {}", h.id, h.status, dur, h.iterations, h.task)
            }).collect::<Vec<_>>().join("\n")
        }
    }

    /// View iteration logs of a subagent.
    pub(super) async fn handle_subagent_logs(&self, args: &serde_json::Value) -> String {
        let id = args.get("subagent_id").and_then(|v| v.as_str()).unwrap_or("");
        let last_n = args.get("last_n").and_then(|v| v.as_u64()).map(|n| n as usize);
        match self.subagent_registry.get(id).await {
            Some(h) => {
                let h = h.read().await;
                let entries = match last_n {
                    Some(n) => &h.log[h.log.len().saturating_sub(n)..],
                    None => &h.log[..],
                };
                if entries.is_empty() { return format!("Subagent {}: no log entries yet.", id); }
                entries.iter().map(|e| {
                    format!("[iter {}] {} tools=[{}] \"{}\"",
                        e.iteration, e.timestamp.format("%H:%M:%S"),
                        e.tool_calls.join(", "), e.content_preview)
                }).collect::<Vec<_>>().join("\n")
            }
            None => format!("Subagent '{}' not found (lost after service restart)", id),
        }
    }

    /// Kill a running subagent.
    pub(super) async fn handle_subagent_kill(&self, args: &serde_json::Value) -> String {
        let id = args.get("subagent_id").and_then(|v| v.as_str()).unwrap_or("");
        match self.subagent_registry.get(id).await {
            Some(h) => {
                let h = h.read().await;
                if h.status != subagent_state::SubagentStatus::Running {
                    return format!("Subagent {} already {:?}", id, h.status);
                }
                h.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                format!("Kill signal sent to {}. Will stop at next iteration.", id)
            }
            None => format!("Subagent '{}' not found (already terminated or lost after service restart — no action needed)", id),
        }
    }

    // Legacy send_to_agent removed — use `handoff` tool instead

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
        // Local calls use the regular http_client (no SSRF filtering) since
        // they go to our own gateway which has its own auth layer.
        let is_local = url.starts_with("http://localhost:") || url.starts_with("http://127.0.0.1:");

        if !is_local {
            // SSRF protection: scheme + internal blocklist (sync);
            // private IP blocking via ssrf_http_client's DNS resolver.
            if let Err(e) = crate::tools::ssrf::validate_url_scheme(url) {
                return format!("Error: {}", e);
            }
        }

        let client = if is_local { &self.http_client } else { &self.ssrf_http_client };
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
        let ws_prompt =
            workspace::load_workspace_prompt(&self.workspace_dir, &self.agent.name).await?;
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
        let runtime = workspace::RuntimeContext {
            agent_name: self.agent.name.clone(),
            owner_id: self.agent.access.as_ref().and_then(|a| a.owner_id.clone()),
            channel: "subagent".to_string(),
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
        available_tools.extend(yaml_tools.into_iter().map(|t| t.to_tool_definition()));
        if let Some(ref mcp) = self.mcp {
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

            let loop_broken = match self.execute_tool_calls_partitioned(
                &response.tool_calls, &serde_json::Value::Null, uuid::Uuid::nil(), crate::agent::channel_kind::channel::INTER_AGENT,
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

// ── invite_agent handler ─────────────────────────────────────────────────────

impl AgentEngine {
    /// Internal tool: invite another agent into the current chat session.
    #[allow(dead_code)]
    pub(super) async fn handle_invite_agent(&self, args: &serde_json::Value) -> String {
        let agent_name = match args.get("agent_name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => return "Error: 'agent_name' is required".to_string(),
        };

        if agent_name == self.agent.name {
            return "Error: cannot invite yourself into your own session".to_string();
        }

        // Verify target agent exists in the agent_map
        let agent_map = match &self.agent_map {
            Some(m) => m,
            None => return "Error: agent registry not available".to_string(),
        };
        {
            let map = agent_map.read().await;
            if !map.contains_key(agent_name) {
                return format!("Error: agent '{}' not found. Use agents_list to see available agents.", agent_name);
            }
        }

        // Get session_id from the processing context
        let session_id = match *self.processing_session_id.lock().await {
            Some(id) => id,
            None => return "Error: no active session (invite_agent only works during chat processing)".to_string(),
        };

        // Add to session participants
        match crate::db::sessions::add_participant(&self.db, session_id, agent_name).await {
            Ok(participants) => {
                // Broadcast join event to WebSocket (UI sidebar refresh + live notification)
                self.broadcast_ui_event(serde_json::json!({
                    "type": "agent_joined",
                    "agent_name": agent_name,
                    "session_id": session_id.to_string(),
                    "invited_by": self.agent.name,
                    "participants": participants,
                }));

                format!("{} has joined the conversation. You can now @-mention them to direct messages.", agent_name)
            }
            Err(e) => format!("Error adding participant: {}", e),
        }
    }
}

// ── code_exec handler ────────────────────────────────────────────────────────

impl AgentEngine {
    /// Internal tool: execute code in an isolated Docker sandbox.
    pub(super) async fn handle_code_exec(&self, args: &serde_json::Value) -> String {
        let code = args.get("code").and_then(|v| v.as_str()).unwrap_or("");
        if code.is_empty() { return "Error: 'code' is required".to_string(); }
        let language = args.get("language").and_then(|v| v.as_str()).unwrap_or("python");
        let packages: Vec<String> = args
            .get("packages")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        // Privileged agents without Docker sandbox execute directly on host
        if self.agent.base && self.sandbox.is_none() {
            return self.execute_host_code(code, language, &packages).await;
        }

        let sandbox = match &self.sandbox {
            Some(s) => s.clone(),
            None => return "Error: Docker sandbox unavailable.".to_string(),
        };
        let host_path = std::fs::canonicalize(&self.workspace_dir).unwrap_or_default().to_string_lossy().to_string();
        match sandbox.execute(&self.agent.name, code, language, &packages, &host_path, self.agent.base).await {
            Ok(result) => {
                let mut out = result.stdout;
                if !result.stderr.is_empty() { out.push_str("\n--- stderr ---\n"); out.push_str(&result.stderr); }
                if out.is_empty() { out = format!("Exit code: {}", result.exit_code); }
                out
            }
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Execute code directly on host (base agents only, no Docker sandbox).
    /// Runs in the hydeclaw working directory with full host access.
    async fn execute_host_code(&self, code: &str, language: &str, packages: &[String]) -> String {
        use tokio::process::Command;

        let timeout = std::time::Duration::from_secs(120);

        // Install packages if requested (avoid shell to prevent command injection via package names)
        if !packages.is_empty() && language == "python" {
            let valid = packages.iter().all(|p| p.chars().all(|c| c.is_alphanumeric() || "-_.[]<>=!,".contains(c)));
            if !valid {
                return "Error: invalid characters in package name".to_string();
            }
            let mut cmd = Command::new("pip");
            cmd.args(["install", "-q"]);
            for p in packages { cmd.arg(p); }
            let _ = cmd.output().await;
        }

        let (cmd, args) = match language {
            "python" => ("python3", vec!["-c".to_string(), code.to_string()]),
            "bash" | "sh" => ("bash", vec!["-c".to_string(), code.to_string()]),
            _ => return format!("Error: unsupported language '{}' for host execution", language),
        };

        match tokio::time::timeout(timeout, Command::new(cmd).args(&args).output()).await {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let mut result = stdout;
                if !stderr.is_empty() {
                    result.push_str("\n--- stderr ---\n");
                    result.push_str(&stderr);
                }
                if result.is_empty() {
                    result = format!("Exit code: {}", output.status.code().unwrap_or(-1));
                }
                // Truncate to prevent LLM context overflow
                if result.len() > 16000 {
                    result.truncate(16000);
                    result.push_str("\n... (truncated)");
                }
                result
            }
            Ok(Err(e)) => format!("Error executing on host: {}", e),
            Err(_) => "Error: host execution timed out (120s)".to_string(),
        }
    }

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
            "memory", "code_exec", "subagent",
            "tool_create", "tool_list", "tool_test", "tool_verify", "tool_disable",
            "skill", "git",
            // UI-critical tools: must never be filtered out by top-K selection
            "canvas", "rich_card", "web_fetch", "handoff",
            // Agent interaction & system tools
            "message", "cron", "session", "agents_list", "browser_action",
            "process", "secret_set", "skill_use", "graph_query", "tool_discover",
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
        let query_vec = self.memory_store.embed(query).await?;

        let mut scored: Vec<(f32, usize)> = Vec::with_capacity(tools.len());
        for (idx, tool) in tools.iter().enumerate() {
            let tool_text = format!("{} {}", tool.name, tool.description);
            let cache_key = format!("tool::{}", tool.name);
            let tool_vec = self
                .tool_embed_cache
                .get_or_embed(&cache_key, &tool_text, self.memory_store.as_ref())
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

    /// Handle a stateless OpenAI-compatible request: the full message history is provided
    /// by the caller, no DB session is used for context.  Tools still execute normally.
    /// The interaction is NOT saved to DB (stateless — the client manages history).
    pub async fn handle_openai(
        &self,
        openai_messages: &[crate::gateway::OpenAiMessage],
        chunk_tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        // 1. Build tool list (same as build_context but without session)
        let yaml_tools = crate::tools::yaml_tools::load_yaml_tools(&self.workspace_dir, false).await;
        let mut raw_tools = self.internal_tool_definitions();
        raw_tools.extend(yaml_tools.into_iter().map(|t| t.to_tool_definition()));
        if let Some(ref mcp) = self.mcp {
            raw_tools.extend(mcp.all_tool_definitions().await);
        }
        let available_tools = self.filter_tools_by_policy(raw_tools);

        // 2. Determine the last user query for memory context
        let _last_user_text = openai_messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .and_then(|m| m.content.as_deref())
            .unwrap_or("");

        // 3. Convert OpenAI messages → internal Message format.
        //    If the caller didn't provide a system message, prepend the agent's system prompt.
        let has_system = openai_messages.iter().any(|m| m.role == "system");
        let mut messages: Vec<Message> = Vec::with_capacity(openai_messages.len() + 1);

        if !has_system {
            let ws_prompt =
                workspace::load_workspace_prompt(&self.workspace_dir, &self.agent.name)
                    .await
                    .unwrap_or_default();

            let mcp_schemas: Vec<String> = if let Some(ref mcp) = self.mcp {
                let defs = mcp.all_tool_definitions().await;
                defs.iter()
                    .map(|t| {
                        format!(
                            "- **{}**: {}\n  Parameters: {}",
                            t.name,
                            t.description,
                            serde_json::to_string(&t.input_schema).unwrap_or_default()
                        )
                    })
                    .collect()
            } else {
                vec![]
            };

            let capabilities = workspace::CapabilityFlags {
                has_search: self.has_tool("search_web").await || self.has_tool("search_web_fresh").await,
                has_memory: self.memory_store.is_available(),
                has_message_actions: false, // no channel adapter in API mode
                has_cron: self.scheduler.is_some(),
                has_yaml_tools: true,
                has_browser: Self::browser_renderer_url() != "disabled",
                has_host_exec: self.agent.base && self.sandbox.is_none(),
                is_base: self.agent.base,
            };

            let runtime = workspace::RuntimeContext {
                agent_name: self.agent.name.clone(),
                owner_id: self.agent.access.as_ref().and_then(|a| a.owner_id.clone()),
                channel: "api".to_string(),
                model: self.provider.current_model(),
                datetime_display: workspace::format_local_datetime(&self.default_timezone),
                formatting_prompt: None,
                channels: vec![],
            };
            let system_prompt = workspace::build_system_prompt(
                &ws_prompt,
                &mcp_schemas,
                &capabilities,
                &self.agent.language,
                &runtime,
            );

            // Skill auto-injection removed — skills are loaded on-demand via skill_use tool.

            messages.push(Message {
                role: MessageRole::System,
                content: system_prompt,
                tool_calls: None,
                tool_call_id: None,
                thinking_blocks: vec![],
            });
        }

        for m in openai_messages {
            messages.push(Message {
                role: match m.role.as_str() {
                    "system" => MessageRole::System,
                    "assistant" => MessageRole::Assistant,
                    "tool" => MessageRole::Tool,
                    _ => MessageRole::User,
                },
                content: m.content.clone().unwrap_or_default(),
                tool_calls: None,
                tool_call_id: None,
                thinking_blocks: vec![],
            });
        }

        // 4. Tool execution loop (no DB saves)
        let mut final_response = String::new();
        let mut last_usage: Option<hydeclaw_types::TokenUsage> = None;
        let loop_config = self.tool_loop_config();
        let mut detector = LoopDetector::new(&loop_config);
        let mut tools_used_acc: Vec<String> = Vec::new();
        let mut final_iteration: u32 = 0;

        for iteration in 0..loop_config.effective_max_iterations() {
            let response = if loop_config.compact_on_overflow {
                self.chat_with_overflow_recovery(&mut messages, &available_tools).await?
            } else {
                self.provider.chat(&messages, &available_tools).await?
            };
            last_usage = response.usage.clone();

            if response.tool_calls.is_empty() {
                final_response = response.content.clone();
                break;
            }

            // Accumulate tool names for API response
            for tc in &response.tool_calls {
                if !tools_used_acc.contains(&tc.name) {
                    tools_used_acc.push(tc.name.clone());
                }
            }
            final_iteration = iteration as u32 + 1;

            tracing::info!(
                iteration,
                max = loop_config.effective_max_iterations(),
                tools = response.tool_calls.len(),
                "openai api: executing tool calls"
            );

            messages.push(Message {
                role: MessageRole::Assistant,
                content: response.content.clone(),
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
                thinking_blocks: vec![],
            });

            let loop_broken = match self.execute_tool_calls_partitioned(
                &response.tool_calls, &serde_json::Value::Null, uuid::Uuid::nil(), crate::agent::channel_kind::channel::INTER_AGENT,
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
                Err(_) => true,
            };

            if loop_broken || iteration == loop_config.effective_max_iterations() - 1 {
                let forced = self.provider.chat(&messages, &[]).await?;
                last_usage = forced.usage.clone();
                final_response = forced.content.clone();
                break;
            }
        }

        let final_response = strip_thinking(&final_response);

        // Send to chunk consumer if streaming requested (MiniMax sends full response at once)
        if let Some(ref tx) = chunk_tx
            && !final_response.is_empty() {
                tx.send(final_response.clone()).ok();
            }

        Ok(hydeclaw_types::LlmResponse {
            content: final_response,
            tool_calls: vec![],
            usage: last_usage,
            model: None,
            provider: None,
            fallback_notice: None,
            tools_used: tools_used_acc,
            iterations: final_iteration,
            thinking_blocks: vec![],
        })
    }

    // ── Background process tools (base agents only) ──────────────────────

    pub(super) async fn handle_process_start(&self, args: &serde_json::Value) -> String {
        use tokio::process::Command;
        
        use rand::Rng;

        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => return "Error: 'command' is required".to_string(),
        };

        let process_id = format!("{:08x}", rand::rng().random::<u32>());
        let log_dir = format!("/tmp/hydeclaw-bg/{}", self.agent.name);
        let log_path = format!("{}/{}.log", log_dir, process_id);

        if let Err(e) = tokio::fs::create_dir_all(&log_dir).await {
            return format!("Error creating log dir: {}", e);
        }

        let log_file = match tokio::fs::File::create(&log_path).await {
            Ok(f) => f,
            Err(e) => return format!("Error creating log file: {}", e),
        };
        let log_file_std = log_file.into_std().await;

        let mut cmd = Command::new("bash");
        cmd.args(["-c", &command]);
        if let Some(wd) = args.get("working_directory").and_then(|v| v.as_str()).filter(|d| !d.is_empty()) {
            cmd.current_dir(wd);
        }
        let mut child = match cmd
            .stdout(std::process::Stdio::from(log_file_std.try_clone().expect("clone stdout")))
            .stderr(std::process::Stdio::from(log_file_std))
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return format!("Error spawning process: {}", e),
        };

        let pid = child.id();

        {
            let mut procs = self.bg_processes.lock().await;
            procs.insert(process_id.clone(), crate::agent::engine::BgProcess {
                process_id: process_id.clone(),
                command: command.clone(),
                log_path: log_path.clone(),
                pid,
                started_at: std::time::Instant::now(),
            });
        }

        // Detach: wait in background so the child doesn't become a zombie
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        format!("Started background process.\nprocess_id: {}\nlog: {}\ncommand: {}", process_id, log_path, command)
    }

    pub(super) async fn handle_process_status(&self, args: &serde_json::Value) -> String {
        // Clean up finished processes on access
        {
            let mut procs = self.bg_processes.lock().await;
            procs.retain(|_id, p| {
                p.pid.is_some_and(|pid| std::path::Path::new(&format!("/proc/{}", pid)).exists())
            });
        }

        let process_id = match args.get("process_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return "Error: 'process_id' is required".to_string(),
        };

        let (pid, log_path) = {
            let procs = self.bg_processes.lock().await;
            match procs.get(&process_id) {
                Some(p) => (p.pid, p.log_path.clone()),
                None => return format!("Error: process '{}' not found", process_id),
            }
        };

        let running = if let Some(pid) = pid {
            std::path::Path::new(&format!("/proc/{}", pid)).exists()
        } else {
            false
        };

        let log_content = tokio::fs::read_to_string(&log_path).await.unwrap_or_default();
        let lines: Vec<&str> = log_content.lines().collect();
        let tail: Vec<&str> = lines.iter().rev().take(20).copied().collect();
        let tail_str = tail.into_iter().rev().collect::<Vec<_>>().join("\n");

        format!(
            "process_id: {}\nstatus: {}\n\n--- last 20 log lines ---\n{}",
            process_id,
            if running { "running" } else { "done" },
            tail_str
        )
    }

    pub(super) async fn handle_process_logs(&self, args: &serde_json::Value) -> String {
        let process_id = match args.get("process_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return "Error: 'process_id' is required".to_string(),
        };
        let tail_lines = args.get("tail_lines").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        let log_path = {
            let procs = self.bg_processes.lock().await;
            match procs.get(&process_id) {
                Some(p) => p.log_path.clone(),
                None => return format!("Error: process '{}' not found", process_id),
            }
        };

        let log_content = tokio::fs::read_to_string(&log_path).await.unwrap_or_default();
        let lines: Vec<&str> = log_content.lines().collect();
        let tail: Vec<&str> = lines.iter().rev().take(tail_lines).copied().collect();
        let tail_str = tail.into_iter().rev().collect::<Vec<_>>().join("\n");

        format!("process_id: {}\n--- last {} lines ---\n{}", process_id, tail_lines, tail_str)
    }

    pub(super) async fn handle_process_kill(&self, args: &serde_json::Value) -> String {
        use tokio::process::Command;

        let process_id = match args.get("process_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return "Error: 'process_id' is required".to_string(),
        };

        let pid = {
            let procs = self.bg_processes.lock().await;
            match procs.get(&process_id) {
                Some(p) => p.pid,
                None => return format!("Error: process '{}' not found", process_id),
            }
        };

        match pid {
            Some(pid) => {
                let result = Command::new("kill").arg(pid.to_string()).output().await;
                match result {
                    Ok(_) => format!("Sent SIGTERM to process {} (pid {})", process_id, pid),
                    Err(e) => format!("Error killing process: {}", e),
                }
            }
            None => format!("Error: process '{}' has no known PID", process_id),
        }
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
        assert!(SUBAGENT_DENIED_TOOLS.contains(&"subagent"));
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

    // ── is_subagent_result_retryable ─────────────────────────────────────────

    #[test]
    fn test_retryable_error_status() {
        let s = r#"{"status":"error","output":"","error":"LLM call failed"}"#;
        assert!(is_subagent_result_retryable(s));
    }

    #[test]
    fn test_retryable_timeout() {
        let s = r#"{"status":"error","output":"","error":"timeout"}"#;
        assert!(is_subagent_result_retryable(s));
    }

    #[test]
    fn test_not_retryable_ok_status() {
        let s = r#"{"status":"ok","output":"done"}"#;
        assert!(!is_subagent_result_retryable(s));
    }

    #[test]
    fn test_not_retryable_cancelled() {
        let s = r#"{"status":"error","output":"","error":"cancelled by parent"}"#;
        assert!(!is_subagent_result_retryable(s));
    }

    #[test]
    fn test_retryable_channel_dropped() {
        let s = r#"{"status":"error","output":"","error":"subagent channel dropped"}"#;
        assert!(is_subagent_result_retryable(s));
    }

    #[test]
    fn test_retryable_non_json() {
        let s = "random non-json string";
        assert!(is_subagent_result_retryable(s));
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

