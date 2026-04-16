//! Workspace, message, shell, channel-action, and cron handlers —
//! extracted from engine.rs for readability.
//! Workspace + browser handlers delegate to `pipeline::handlers` free functions.

use super::*;
use crate::agent::pipeline::handlers as ph;
use crate::scheduler::ScheduledJob;

impl AgentEngine {
    /// Internal tool: write a workspace file.
    pub(super) async fn handle_workspace_write(&self, args: &serde_json::Value) -> String {
        ph::handle_workspace_write(
            &self.cfg().workspace_dir,
            &self.cfg().agent.name,
            self.cfg().agent.base,
            args,
        )
        .await
    }

    /// Internal tool: read a file from workspace.
    pub(super) async fn handle_workspace_read(&self, args: &serde_json::Value) -> String {
        ph::handle_workspace_read(
            &self.cfg().workspace_dir,
            &self.cfg().agent.name,
            args,
        )
        .await
    }

    /// Internal tool: list files in workspace directory.
    pub(super) async fn handle_workspace_list(&self, args: &serde_json::Value) -> String {
        ph::handle_workspace_list(
            &self.cfg().workspace_dir,
            &self.cfg().agent.name,
            args,
        )
        .await
    }

    /// Internal tool: edit a file by replacing a text substring.
    pub(super) async fn handle_workspace_edit(&self, args: &serde_json::Value) -> String {
        ph::handle_workspace_edit(
            &self.cfg().workspace_dir,
            &self.cfg().agent.name,
            self.cfg().agent.base,
            args,
        )
        .await
    }

    pub(super) async fn handle_workspace_delete(&self, args: &serde_json::Value) -> String {
        ph::handle_workspace_delete(
            &self.cfg().workspace_dir,
            &self.cfg().agent.name,
            args,
        )
        .await
    }

    pub(super) async fn handle_workspace_rename(&self, args: &serde_json::Value) -> String {
        ph::handle_workspace_rename(
            &self.cfg().workspace_dir,
            &self.cfg().agent.name,
            args,
        )
        .await
    }

    // TODO: extract handle_message_action to pipeline::handlers — depends on self.channel_router (ChannelAction, oneshot, timeout)
    /// Internal tool: perform message actions via channel router.
    pub(super) async fn handle_message_action(&self, args: &serde_json::Value) -> String {
        let router = match &self.channel_router {
            Some(r) => r,
            None => return "Error: message actions not available (no channel connection)".to_string(),
        };

        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        if action.is_empty() {
            return "Error: 'action' is required".to_string();
        }

        let context = args.get("_context").cloned().unwrap_or(serde_json::Value::Null);
        let target_channel = args.get("channel").and_then(|v| v.as_str()).map(|s| s.to_string());

        // Collect action-specific params (exclude internal _context, action, channel fields)
        let params = {
            let mut p = serde_json::Map::new();
            if let Some(obj) = args.as_object() {
                for (k, v) in obj {
                    if k != "_context" && k != "action" && k != "channel" {
                        p.insert(k.clone(), v.clone());
                    }
                }
            }
            serde_json::Value::Object(p)
        };

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

        let channel_action = ChannelAction {
            name: action.to_string(),
            params,
            context,
            reply: reply_tx,
            target_channel,
        };

        if let Err(e) = router.send(channel_action).await {
            return format!("Error: {e}");
        }

        match tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx).await {
            Ok(Ok(Ok(()))) => format!("Successfully performed '{}' action", action),
            Ok(Ok(Err(e))) => format!("Error performing '{}': {}", action, e),
            Ok(Err(_)) => "Error: action reply channel dropped".to_string(),
            Err(_) => "Error: action timed out".to_string(),
        }
    }

    /// Send a message to a specific channel directly (e.g. from cron announce).
    /// Uses channel router to route to the correct channel adapter.
    pub async fn send_channel_message(
        &self,
        channel: &str,
        chat_id: i64,
        text: &str,
    ) -> anyhow::Result<()> {
        let router = self.channel_router.as_ref()
            .ok_or_else(|| anyhow::anyhow!("no channel connection available"))?;
        let (reply_tx, _) = tokio::sync::oneshot::channel();
        let action = ChannelAction {
            name: "send_message".to_string(),
            params: serde_json::json!({ "text": text }),
            context: serde_json::json!({ "channel": channel, "chat_id": chat_id }),
            reply: reply_tx,
            target_channel: Some(channel.to_string()),
        };
        router.send(action).await.map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }

    // TODO: extract execute_yaml_channel_action to pipeline::handlers — depends on self.channel_router,
    //       self.make_resolver(), self.make_oauth_context(), self.http_client(), self.ssrf_http_client()
    /// Execute a system YAML tool that has a channel_action (e.g. TTS → send_voice, screenshot → send_photo).
    /// Calls the tool HTTP endpoint for binary data, then sends it via channel router.
    /// For image actions (send_photo), also saves to uploads/ and returns a FILE_PREFIX marker
    /// so the UI can display the image inline.
    pub(super) async fn execute_yaml_channel_action(
        &self,
        tool: &crate::tools::yaml_tools::YamlToolDef,
        args: &serde_json::Value,
        ca: &crate::tools::yaml_tools::ChannelActionConfig,
    ) -> String {
        let resolver = self.make_resolver();
        let oauth_ctx = self.make_oauth_context();
        tracing::info!(tool = %tool.name, action = %ca.action, "executing channel action: calling tool endpoint");
        // Internal endpoints (toolgate, searxng, etc.) bypass SSRF filtering
        let client = if crate::tools::ssrf::is_internal_endpoint(&tool.endpoint) {
            self.http_client()
        } else {
            self.ssrf_http_client()
        };
        let data_bytes = match tool.execute_binary(args, client, Some(&resolver), oauth_ctx.as_ref()).await {
            Ok(b) => b,
            Err(e) => return format!("Error calling tool '{}': {}", tool.name, e),
        };
        tracing::info!(tool = %tool.name, bytes = data_bytes.len(), "channel action: got binary data");

        // --- Save image/media to uploads/ for UI display ---
        let file_marker = if ca.action == "send_photo" {
            match ph::save_binary_to_uploads(&self.cfg().workspace_dir, &data_bytes, "image").await {
                Ok((url, media_type)) => {
                    let meta = serde_json::json!({"url": url, "mediaType": media_type});
                    Some(format!("{}{}", super::FILE_PREFIX, meta))
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to save image to uploads for UI");
                    None
                }
            }
        } else {
            None
        };

        // --- Send via channel router (Telegram etc.) ---
        // Skip channel action for UI sessions (no chat_id in context).
        let context = args.get("_context").cloned().unwrap_or(serde_json::Value::Null);
        let has_channel_context = context.get("chat_id").is_some();

        let channel_result = if !has_channel_context {
            tracing::info!(tool = %tool.name, "skipping channel action: no chat_id in context (UI session)");
            None
        } else if let Some(ref router) = self.channel_router {
            use base64::Engine as _;
            let data_base64 = base64::engine::general_purpose::STANDARD.encode(&data_bytes);
            tracing::info!(tool = %tool.name, context = %context, "channel action: sending to adapter");

            let param_key = match ca.action.as_str() {
                "send_photo" => "image_base64",
                "send_voice" => "audio_base64",
                other => {
                    tracing::warn!(action = %other, "unknown channel action, using 'data_base64'");
                    "data_base64"
                }
            };

            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if router
                .send(ChannelAction {
                    name: ca.action.clone(),
                    params: serde_json::json!({ param_key: data_base64 }),
                    context,
                    reply: reply_tx,
                    target_channel: None,
                })
                .await
                .is_err()
            {
                Some("Error: channel action channel closed".to_string())
            } else {
                match tokio::time::timeout(std::time::Duration::from_secs(60), reply_rx).await {
                    Ok(Ok(Ok(()))) => Some(format!("{} sent successfully", ca.action)),
                    Ok(Ok(Err(e))) => Some(format!("Error sending {}: {}", ca.action, e)),
                    Ok(Err(_)) => Some(format!("Error: {} reply channel dropped", ca.action)),
                    Err(_) => Some(format!("Error: {} send timed out", ca.action)),
                }
            }
        } else {
            None
        };

        // Return file marker (for UI) + channel result
        match (file_marker, channel_result) {
            (Some(marker), Some(ch_res)) => format!("{}\n{}", marker, ch_res),
            (Some(marker), None) => format!("{}\nImage displayed inline. Do NOT use canvas or other tools to show it again.", marker),
            (None, Some(ch_res)) => ch_res,
            (None, None) => "Error: no channel connection and failed to save media".to_string(),
        }
    }

    // TODO: extract handle_cron to pipeline::handlers — depends on self.cfg().scheduler, self.self_ref,
    //       self.cfg().db, self.cfg().agent, self.cfg().default_timezone, self.run_subagent()
    /// Internal tool: manage scheduled cron jobs.
    /// Mutating actions (create/delete/run) require base agent.
    pub(super) async fn handle_cron(&self, args: &serde_json::Value) -> String {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Only base agents can add/update/remove/run cron jobs.
        // list and history are read-only, allowed for all agents.
        if !self.cfg().agent.base && !matches!(action, "list" | "history" | "runs") {
            return format!("Error: cron '{}' requires a base agent. Only base agents can manage cron jobs.", action);
        }

        let scheduler = match &self.cfg().scheduler {
            Some(s) => s,
            None => return "Error: scheduler not available".to_string(),
        };

        match action {
            "list" => {
                let agent_filter = if self.cfg().agent.base { None } else { Some(self.cfg().agent.name.as_str()) };
                let jobs_result = Scheduler::list_jobs(&self.cfg().db, agent_filter).await;
                match jobs_result {
                    Ok(jobs) => {
                        if jobs.is_empty() {
                            return "No scheduled jobs.".to_string();
                        }
                        let mut out = format!("Scheduled jobs ({}):\n", jobs.len());
                        for job in &jobs {
                            let next = if job.enabled {
                                compute_next_run(&job.cron_expr, &job.timezone)
                                    .unwrap_or_else(|| "unknown".to_string())
                            } else {
                                "disabled".to_string()
                            };
                            let announce = job.announce_to.as_ref()
                                .map(|v| format!("  announce_to: {}\n", v))
                                .unwrap_or_default();
                            let agent_label = if self.cfg().agent.base && job.agent_id != self.cfg().agent.name {
                                format!("  agent: {}\n", job.agent_id)
                            } else {
                                String::new()
                            };
                            out.push_str(&format!(
                                "- **{}** (id: {})\n{}  cron: `{}` ({})\n  task: {}\n  enabled: {}, last run: {}\n  next run: {}\n{}",
                                job.name,
                                job.id,
                                agent_label,
                                job.cron_expr,
                                job.timezone,
                                job.task_message,
                                job.enabled,
                                job.last_run_at
                                    .map(|t| t.to_string())
                                    .unwrap_or_else(|| "never".to_string()),
                                next,
                                announce,
                            ));
                        }
                        out
                    }
                    Err(e) => format!("Error listing jobs: {}", e),
                }
            }
            "add" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let cron_expr = args.get("cron").and_then(|v| v.as_str()).unwrap_or("");
                let timezone = args
                    .get("timezone")
                    .and_then(|v| v.as_str())
                    .unwrap_or(self.cfg().default_timezone.as_str());
                let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
                let announce_to = args.get("announce_to").cloned();
                let target_agent = if self.cfg().agent.base {
                    args.get("agent").and_then(|v| v.as_str()).unwrap_or(&self.cfg().agent.name).to_string()
                } else {
                    self.cfg().agent.name.clone()
                };

                if name.is_empty() || cron_expr.is_empty() || task.is_empty() {
                    return "Error: 'name', 'cron', and 'task' are required for add".to_string();
                }

                // Validate cron expression (5 fields)
                let fields: Vec<&str> = cron_expr.split_whitespace().collect();
                if fields.len() != 5 {
                    return "Error: cron expression must have 5 fields (min hour dom mon dow)".to_string();
                }

                // Insert into DB
                let row = match sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO scheduled_jobs (agent_id, name, cron_expr, timezone, task_message, announce_to) \
                     VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
                )
                .bind(&target_agent)
                .bind(name)
                .bind(cron_expr)
                .bind(timezone)
                .bind(task)
                .bind(&announce_to)
                .fetch_one(&self.cfg().db)
                .await
                {
                    Ok(id) => id,
                    Err(e) => return format!("Error saving job to DB: {}", e),
                };

                // Hot-schedule the job immediately (only for self — other agents activate on restart)
                let is_self = target_agent == self.cfg().agent.name;
                let activated = if is_self {
                    if let Some(arc) = self.self_ref.get().and_then(Weak::upgrade) {
                        match scheduler.add_dynamic_job(
                            row, cron_expr, timezone,
                            task.to_string(), target_agent.clone(),
                            arc, self.cfg().db.clone(), announce_to, false, 0, false, None, None,
                        ).await {
                            Ok(()) => true,
                            Err(e) => {
                                tracing::warn!(error = %e, "failed to hot-schedule job, will load on restart");
                                false
                            }
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                let agent_note = if !is_self { format!(" for agent '{}'", target_agent) } else { String::new() };
                if activated {
                    format!(
                        "Job '{}' created and activated{} (id: {}). Cron: `{}` ({}).",
                        name, agent_note, row, cron_expr, timezone
                    )
                } else {
                    format!(
                        "Job '{}' created{} (id: {}). Cron: `{}` ({}). \
                         It will be activated on next restart. Use action 'run' to execute immediately.",
                        name, agent_note, row, cron_expr, timezone
                    )
                }
            }
            "update" => {
                let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                if job_id.is_empty() {
                    return "Error: 'job_id' is required for update".to_string();
                }
                let uuid = match Uuid::parse_str(job_id) {
                    Ok(u) => u,
                    Err(_) => return "Error: invalid job_id format (expected UUID)".to_string(),
                };

                // Fetch current job (base can update any)
                let current = if self.cfg().agent.base {
                    sqlx::query_as::<_, ScheduledJob>(
                        "SELECT id, agent_id, name, cron_expr, timezone, task_message, enabled, created_at, last_run_at, silent, announce_to, jitter_secs, run_once, run_at \
                         FROM scheduled_jobs WHERE id = $1",
                    )
                    .bind(uuid)
                    .fetch_optional(&self.cfg().db)
                    .await
                } else {
                    sqlx::query_as::<_, ScheduledJob>(
                        "SELECT id, agent_id, name, cron_expr, timezone, task_message, enabled, created_at, last_run_at, silent, announce_to, jitter_secs, run_once, run_at \
                         FROM scheduled_jobs WHERE id = $1 AND agent_id = $2",
                    )
                    .bind(uuid)
                    .bind(&self.cfg().agent.name)
                    .fetch_optional(&self.cfg().db)
                    .await
                };

                let current = match current {
                    Ok(Some(j)) => j,
                    Ok(None) => return "Error: job not found or belongs to another agent".to_string(),
                    Err(e) => return format!("Error fetching job: {}", e),
                };

                // Merge: use provided values or keep current
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or(&current.name);
                let cron_expr = args.get("cron").and_then(|v| v.as_str()).unwrap_or(&current.cron_expr);
                let timezone = args.get("timezone").and_then(|v| v.as_str()).unwrap_or(&current.timezone);
                let task = args.get("task").and_then(|v| v.as_str()).unwrap_or(&current.task_message);
                let enabled = args.get("enabled").and_then(|v| v.as_bool()).unwrap_or(current.enabled);
                let announce_to = args.get("announce_to").cloned().or(current.announce_to);

                // Validate cron if changed
                if args.get("cron").is_some() {
                    let fields: Vec<&str> = cron_expr.split_whitespace().collect();
                    if fields.len() != 5 {
                        return "Error: cron expression must have 5 fields (min hour dom mon dow)".to_string();
                    }
                }

                match sqlx::query(
                    "UPDATE scheduled_jobs SET name = $2, cron_expr = $3, timezone = $4, task_message = $5, \
                     enabled = $6, announce_to = $7 WHERE id = $1",
                )
                .bind(uuid)
                .bind(name)
                .bind(cron_expr)
                .bind(timezone)
                .bind(task)
                .bind(enabled)
                .bind(&announce_to)
                .execute(&self.cfg().db)
                .await
                {
                    Ok(_) => {
                        // Reschedule
                        scheduler.remove_dynamic_job(uuid).await.ok();
                        if enabled
                            && let Some(arc) = self.self_ref.get().and_then(Weak::upgrade)
                                && current.agent_id == self.cfg().agent.name {
                                    scheduler.add_dynamic_job(
                                        uuid, cron_expr, timezone,
                                        task.to_string(), current.agent_id.clone(),
                                        arc, self.cfg().db.clone(), announce_to, current.silent,
                                        current.jitter_secs, current.run_once, current.run_at,
                                        current.tool_policy.clone(),
                                    ).await.ok();
                                }
                        format!("Job '{}' updated (id: {}).", name, uuid)
                    }
                    Err(e) => format!("Error updating job: {}", e),
                }
            }
            "remove" => {
                let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                if job_id.is_empty() {
                    return "Error: 'job_id' is required for remove".to_string();
                }

                let uuid = match Uuid::parse_str(job_id) {
                    Ok(u) => u,
                    Err(_) => return "Error: invalid job_id format (expected UUID)".to_string(),
                };

                // Remove from scheduler if running
                if let Err(e) = scheduler.remove_dynamic_job(uuid).await {
                    tracing::warn!(error = %e, "job not in scheduler (may not have been loaded)");
                }

                // Remove from DB (base can remove any job)
                let delete_result = if self.cfg().agent.base {
                    sqlx::query("DELETE FROM scheduled_jobs WHERE id = $1")
                        .bind(uuid)
                        .execute(&self.cfg().db)
                        .await
                } else {
                    sqlx::query("DELETE FROM scheduled_jobs WHERE id = $1 AND agent_id = $2")
                        .bind(uuid)
                        .bind(&self.cfg().agent.name)
                        .execute(&self.cfg().db)
                        .await
                };
                match delete_result {
                    Ok(result) => {
                        if result.rows_affected() == 0 {
                            "Error: job not found or belongs to another agent".to_string()
                        } else {
                            format!("Job {} removed successfully", uuid)
                        }
                    }
                    Err(e) => format!("Error removing job: {}", e),
                }
            }
            "history" | "runs" => {
                let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10).min(50);

                if job_id.is_empty() {
                    // Show recent runs (base: all agents, regular: own only)
                    let rows = if self.cfg().agent.base {
                        sqlx::query_as::<_, (String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, String, Option<String>, Option<String>)>(
                            "SELECT COALESCE(j.name, 'unknown'), r.started_at, r.finished_at, r.status, r.error, r.response_preview \
                             FROM cron_runs r LEFT JOIN scheduled_jobs j ON r.job_id = j.id \
                             ORDER BY r.started_at DESC LIMIT $1",
                        )
                        .bind(limit)
                        .fetch_all(&self.cfg().db)
                        .await
                    } else {
                        sqlx::query_as::<_, (String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, String, Option<String>, Option<String>)>(
                            "SELECT COALESCE(j.name, 'unknown'), r.started_at, r.finished_at, r.status, r.error, r.response_preview \
                             FROM cron_runs r LEFT JOIN scheduled_jobs j ON r.job_id = j.id \
                             WHERE r.agent_id = $1 ORDER BY r.started_at DESC LIMIT $2",
                        )
                        .bind(&self.cfg().agent.name)
                        .bind(limit)
                        .fetch_all(&self.cfg().db)
                        .await
                    };

                    match rows {
                        Ok(runs) if runs.is_empty() => "No cron runs found.".to_string(),
                        Ok(runs) => {
                            let mut out = format!("Recent cron runs ({}):\n", runs.len());
                            for (name, started, finished, status, error, preview) in &runs {
                                let duration = finished
                                    .map(|f| {
                                        let secs = (f - *started).num_seconds();
                                        format!("{}s", secs)
                                    })
                                    .unwrap_or_else(|| "running".to_string());
                                out.push_str(&format!("- **{}** [{}] {}\n  started: {}, duration: {}\n",
                                    name, status,
                                    error.as_deref().map(|e| format!("error: {}", e)).unwrap_or_default(),
                                    started, duration,
                                ));
                                if let Some(p) = preview {
                                    out.push_str(&format!("  preview: {}\n", p));
                                }
                            }
                            out
                        }
                        Err(e) => format!("Error fetching runs: {}", e),
                    }
                } else {
                    let uuid = match Uuid::parse_str(job_id) {
                        Ok(u) => u,
                        Err(_) => return "Error: invalid job_id format (expected UUID)".to_string(),
                    };

                    let rows = if self.cfg().agent.base {
                        sqlx::query_as::<_, (chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, String, Option<String>, Option<String>)>(
                            "SELECT started_at, finished_at, status, error, response_preview \
                             FROM cron_runs WHERE job_id = $1 ORDER BY started_at DESC LIMIT $2",
                        )
                        .bind(uuid)
                        .bind(limit)
                        .fetch_all(&self.cfg().db)
                        .await
                    } else {
                        sqlx::query_as::<_, (chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>, String, Option<String>, Option<String>)>(
                            "SELECT started_at, finished_at, status, error, response_preview \
                             FROM cron_runs WHERE job_id = $1 AND agent_id = $2 ORDER BY started_at DESC LIMIT $3",
                        )
                        .bind(uuid)
                        .bind(&self.cfg().agent.name)
                        .bind(limit)
                        .fetch_all(&self.cfg().db)
                        .await
                    };

                    match rows {
                        Ok(runs) if runs.is_empty() => format!("No runs found for job {}", uuid),
                        Ok(runs) => {
                            let mut out = format!("Runs for job {} ({}):\n", uuid, runs.len());
                            for (started, finished, status, error, preview) in &runs {
                                let duration = finished
                                    .map(|f| {
                                        let secs = (f - *started).num_seconds();
                                        format!("{}s", secs)
                                    })
                                    .unwrap_or_else(|| "running".to_string());
                                out.push_str(&format!("- [{}] {}\n  started: {}, duration: {}\n",
                                    status,
                                    error.as_deref().map(|e| format!("error: {}", e)).unwrap_or_default(),
                                    started, duration,
                                ));
                                if let Some(p) = preview {
                                    out.push_str(&format!("  preview: {}\n", p));
                                }
                            }
                            out
                        }
                        Err(e) => format!("Error fetching runs: {}", e),
                    }
                }
            }
            "run" => {
                let task_arg = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
                let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");

                // Resolve task text: from job_id (lookup DB) or direct task argument
                let task = if !job_id.is_empty() {
                    let uuid = match Uuid::parse_str(job_id) {
                        Ok(u) => u,
                        Err(_) => return "Error: invalid job_id format (expected UUID)".to_string(),
                    };
                    let row = if self.cfg().agent.base {
                        sqlx::query_scalar::<_, String>(
                            "SELECT task_message FROM scheduled_jobs WHERE id = $1",
                        )
                        .bind(uuid)
                        .fetch_optional(&self.cfg().db)
                        .await
                    } else {
                        sqlx::query_scalar::<_, String>(
                            "SELECT task_message FROM scheduled_jobs WHERE id = $1 AND agent_id = $2",
                        )
                        .bind(uuid)
                        .bind(&self.cfg().agent.name)
                        .fetch_optional(&self.cfg().db)
                        .await
                    };
                    match row {
                        Ok(Some(t)) => t,
                        Ok(None) => return "Error: job not found or belongs to another agent".to_string(),
                        Err(e) => return format!("Error looking up job: {}", e),
                    }
                } else if !task_arg.is_empty() {
                    task_arg.to_string()
                } else {
                    return "Error: 'task' or 'job_id' is required for run".to_string();
                };

                // Execute the task immediately as a subagent
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
                match tokio::time::timeout(
                    std::time::Duration::from_secs(120),
                    self.run_subagent(&task, 5, Some(deadline), None, None, None),
                )
                .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => format!("Error running task: {}", e),
                    Err(_) => "Task timed out (120s limit).".to_string(),
                }
            }
            _ => format!("Error: unknown action '{}'. Use: list, history, add, update, remove, run", action),
        }
    }

    /// Handle browser automation actions via browser-renderer /automation endpoint.
    pub(super) async fn handle_browser_action(&self, args: &serde_json::Value) -> String {
        let br_url = Self::browser_renderer_url();
        ph::handle_browser_action(
            self.http_client(),
            &br_url,
            args,
        )
        .await
    }

    // service_manage, service_exec, call_services_api removed —
    // base agent uses code_exec on host directly
}
