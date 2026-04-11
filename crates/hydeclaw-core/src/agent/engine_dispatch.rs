//! Tool dispatch: execute_tool_call, execute_tool_call_inner, approval flow,
//! usage recording, and tool policy filtering.
//! Extracted from engine.rs for readability.

use super::*;

impl AgentEngine {
    /// Execute a tool call — routes to internal tools, MCP servers, or ToolRegistry.
    /// Returns a boxed future to allow recursive calls (approval re-injection → execute_tool_call).
    pub(super) fn execute_tool_call<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let audit_start = std::time::Instant::now();
            let result = self.execute_tool_call_inner(name, arguments).await;

            // Fire-and-forget audit record
            let duration_ms = audit_start.elapsed().as_millis() as i32;
            let is_error = result.contains("\"status\":\"error\"")
                || result.starts_with("Error:")
                || result.starts_with("Tool '") && result.contains("timed out");
            let (status, error_msg) = if is_error {
                ("error", Some(result.clone()))
            } else {
                ("ok", None)
            };

            // Extract session_id from enriched _context
            let session_id = arguments
                .get("_context")
                .and_then(|c| c.get("session_id"))
                .and_then(|s| s.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());

            // Strip _context from parameters before storing (contains internal routing data)
            let clean_params = {
                let mut p = arguments.clone();
                if let Some(obj) = p.as_object_mut() {
                    obj.remove("_context");
                }
                p
            };

            // Hook: AfterToolResult (fire-and-forget, non-blocking)
            self.hooks().fire(&crate::agent::hooks::HookEvent::AfterToolResult {
                agent: self.agent.name.clone(),
                tool_name: name.to_string(),
                duration_ms: duration_ms as u64,
            });

            let db = self.db.clone();
            let agent_name = self.agent.name.clone();
            let tool_name = name.to_string();
            let error_msg_for_quality = error_msg.clone();
            tokio::spawn(async move {
                let _ = crate::db::tool_audit::record_tool_execution(
                    &db,
                    &agent_name,
                    session_id,
                    &tool_name,
                    Some(&clean_params),
                    status,
                    Some(duration_ms),
                    error_msg.as_deref(),
                )
                .await;
            });

            // Record tool quality (non-system tools only)
            if !tool_defs_impl::all_system_tool_names().contains(&name) {
                let db2 = self.db.clone();
                let tool_name2 = name.to_string();
                let is_ok = !is_error;
                let dur = duration_ms;
                let err_msg = error_msg_for_quality;
                tokio::spawn(async move {
                    let _ = crate::db::tool_quality::record_tool_result(
                        &db2, &tool_name2, is_ok, dur, err_msg.as_deref(),
                    ).await;
                });
            }

            result
        })
    }

    /// Inner tool dispatch (separated for audit wrapping).
    pub(super) fn execute_tool_call_inner<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            // 0. Approval check — if tool requires confirmation, wait for owner.
            // Skip approval for automated channels (cron, heartbeat, inter-agent).
            let context = arguments.get("_context").cloned().unwrap_or_default();
            let is_automated = context.get("_channel")
                .and_then(|v| v.as_str())
                .map(crate::agent::channel_kind::channel::is_automated)
                .unwrap_or(false);
            let has_interactive_channel = context.get("chat_id").is_some() && !is_automated;
            if self.needs_approval(name) && has_interactive_channel {
                // Skip if tool is in allowlist
                if let Ok(true) = crate::db::approvals::check_allowlist(&self.db, &self.agent.name, name).await {
                    // fall through to execution
                } else {
                let session_id = context.get("session_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok());

                // Create DB record
                let approval_id = match crate::db::approvals::create_approval(
                    &self.db,
                    &self.agent.name,
                    session_id,
                    name,
                    arguments,
                    &context,
                ).await {
                    Ok(id) => {
                        self.audit(crate::db::audit::event_types::APPROVAL_REQUESTED, None, serde_json::json!({
                            "tool": name, "approval_id": id.to_string()
                        }));
                        self.broadcast_ui_event(serde_json::json!({
                            "type": "approval_requested",
                            "approval_id": id.to_string(),
                            "agent": self.agent.name,
                            "tool_name": name,
                        }));
                        if let Some(ref ui_tx) = self.ui_event_tx {
                            let db = self.db.clone();
                            let tx = ui_tx.clone();
                            let tool_name = name.to_string();
                            let agent_name = self.agent.name.clone();
                            let approval_id_str = id.to_string();
                            tokio::spawn(async move {
                                crate::gateway::notify(
                                    &db, &tx, "tool_approval",
                                    "Tool Approval Required",
                                    &format!("Agent {} is requesting approval to use tool: {}", agent_name, tool_name),
                                    serde_json::json!({"agent": agent_name, "tool_name": tool_name, "approval_id": approval_id_str}),
                                ).await.ok();
                            });
                        }
                        id
                    }
                    Err(e) => return format!("Error creating approval: {}", e),
                };

                // Send approval request via channel (adapter formats with localization)
                let clean_args = {
                    let mut args_clone = arguments.clone();
                    if let Some(obj) = args_clone.as_object_mut() {
                        obj.remove("_context");
                    }
                    args_clone
                };

                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                let action = crate::agent::channel_actions::ChannelAction {
                    name: "approval_request".to_string(),
                    params: serde_json::json!({
                        "tool_name": name,
                        "args": clean_args,
                        "approval_id": approval_id.to_string(),
                    }),
                    context: context.clone(),
                    reply: reply_tx,
                    target_channel: None, // approval buttons go to originating channel
                };
                if let Some(ref router) = self.channel_router {
                    if let Err(e) = router.send(action).await {
                        tracing::error!(approval_id = %approval_id, error = %e, "failed to send approval_request to channel");
                    }
                    tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await.ok();
                } else {
                    tracing::warn!(tool = %name, "no channel_router — cannot send approval buttons");
                }

                // Create oneshot channel for waiting
                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                {
                    let mut waiters = self.approval_waiters().write().await;
                    // Opportunistic cleanup: remove expired entries (>5 min).
                    // Dropping the sender causes RecvError on the receiver, handled as "cancelled" below.
                    let cutoff = std::time::Instant::now() - std::time::Duration::from_secs(300);
                    waiters.retain(|_, (_, created_at)| *created_at > cutoff);
                    waiters.insert(approval_id, (result_tx, std::time::Instant::now()));
                }

                // Wait for approval with timeout
                let timeout_secs = self.agent.approval
                    .as_ref()
                    .map(|a| a.timeout_seconds)
                    .unwrap_or(300);

                // Emit SSE event for inline approval in chat UI
                if let Some(tx) = self.sse_event_tx().lock().await.as_ref() {
                    let clean_input = {
                        let mut args_clone = arguments.clone();
                        if let Some(obj) = args_clone.as_object_mut() {
                            obj.remove("_context");
                        }
                        args_clone
                    };
                    tx.send(StreamEvent::ApprovalNeeded {
                        approval_id: approval_id.to_string(),
                        tool_name: name.to_string(),
                        tool_input: clean_input,
                        timeout_ms: timeout_secs * 1000,
                    }).ok();
                }

                let effective_args: Option<serde_json::Value> = match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    result_rx,
                ).await {
                    Ok(Ok(ApprovalResult::Approved)) => {
                        tracing::info!(tool = %name, approval_id = %approval_id, "tool approved");
                        None // Fall through to normal execution with original args
                    }
                    Ok(Ok(ApprovalResult::ApprovedWithModifiedArgs(modified_args))) => {
                        tracing::info!(tool = %name, approval_id = %approval_id, "tool approved with modified args");
                        Some(modified_args) // Use modified args for execution
                    }
                    Ok(Ok(ApprovalResult::Rejected(reason))) => {
                        return format!("Tool `{}` was rejected: {}", name, reason);
                    }
                    Ok(Err(_)) => {
                        // Sender dropped — cleanup waiter
                        let mut waiters = self.approval_waiters().write().await;
                        waiters.remove(&approval_id);
                        return format!("Tool `{}` approval was cancelled.", name);
                    }
                    Err(_) => {
                        // Timeout fired — attempt to mark as timed out in DB.
                        // WHERE status='pending' ensures only one resolution wins.
                        let was_pending = crate::db::approvals::resolve_approval(
                            &self.db, approval_id, "timeout", "system",
                        ).await.unwrap_or(false);

                        let mut waiters = self.approval_waiters().write().await;
                        waiters.remove(&approval_id);

                        // Emit SSE event for timeout
                        if let Some(tx) = self.sse_event_tx().lock().await.as_ref() {
                            tx.send(StreamEvent::ApprovalResolved {
                                approval_id: approval_id.to_string(),
                                action: "timeout_rejected".to_string(),
                                modified_input: None,
                            }).ok();
                        }

                        if !was_pending {
                            tracing::warn!(
                                tool = %name,
                                approval_id = %approval_id,
                                "approval timeout raced with resolution — timeout takes precedence"
                            );
                        }
                        return format!("Tool `{}` approval timed out after {}s.", name, timeout_secs);
                    }
                };

                // If approved with modified args, re-inject _context and recurse into execute_tool_call
                if let Some(mut modified) = effective_args {
                    // Preserve internal _context from original arguments
                    if let Some(ctx) = arguments.get("_context")
                        && let Some(obj) = modified.as_object_mut()
                    {
                        obj.insert("_context".to_string(), ctx.clone());
                    }
                    return self.execute_tool_call(name, &modified).await;
                }
            } // else: allowlist
            }

            // Hook: BeforeToolCall
            if let crate::agent::hooks::HookAction::Block(reason) = self.hooks().fire(&crate::agent::hooks::HookEvent::BeforeToolCall {
                agent: self.agent.name.clone(),
                tool_name: name.to_string(),
            }) {
                return format!("Tool blocked by hook: {}", reason);
            }

            // 1. Internal tools
            if name == "workspace_write" {
                return self.handle_workspace_write(arguments).await;
            }
            if name == "workspace_read" {
                return self.handle_workspace_read(arguments).await;
            }
            if name == "workspace_list" {
                return self.handle_workspace_list(arguments).await;
            }
            if name == "workspace_edit" {
                return self.handle_workspace_edit(arguments).await;
            }
            if name == "workspace_delete" {
                return self.handle_workspace_delete(arguments).await;
            }
            if name == "workspace_rename" {
                return self.handle_workspace_rename(arguments).await;
            }
            if name == "memory" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");
                return match action {
                    "search" => self.handle_memory_search(arguments).await,
                    "index" => self.handle_memory_index(arguments).await,
                    "reindex" => self.handle_memory_reindex(arguments).await,
                    "get" => self.handle_memory_get(arguments).await,
                    "delete" => self.handle_memory_delete(arguments).await,
                    "compress" => self.handle_memory_compress(arguments).await,
                    "update" => {
                        // Remap sub_action -> action for handle_memory_update compatibility
                        let mut args = arguments.clone();
                        if let Some(sa) = arguments.get("sub_action").cloned()
                            && let Some(obj) = args.as_object_mut() {
                                obj.insert("action".to_string(), sa);
                            }
                        self.handle_memory_update(&args).await
                    }
                    _ => format!("Error: unknown memory action '{}'. Use: search, index, reindex, get, delete, update, compress.", action),
                };
            }
            if name == "message" {
                return self.handle_message_action(arguments).await;
            }
            // shell_exec removed — use code_exec(language="bash") instead
            if name == "cron" {
                return self.handle_cron(arguments).await;
            }
            if name == "agent" {
                return self.handle_agent_tool(arguments).await;
            }
            if name == "web_fetch" {
                return self.handle_web_fetch(arguments).await;
            }
            if name == "graph_query" {
                return self.handle_graph_query(arguments).await;
            }
            if name == "tool_create" {
                return self.handle_tool_create(arguments).await;
            }
            if name == "tool_list" {
                return self.handle_tool_list(arguments).await;
            }
            if name == "tool_test" {
                return self.handle_tool_test(arguments).await;
            }
            if name == "tool_verify" {
                return self.handle_tool_verify(arguments).await;
            }
            if name == "tool_disable" {
                return self.handle_tool_disable(arguments).await;
            }
            if name == "skill" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");
                return match action {
                    "create" => self.handle_skill_create(arguments).await,
                    "update" => self.handle_skill_update(arguments).await,
                    "list" => self.handle_skill_list(arguments).await,
                    _ => format!("Error: unknown skill action '{}'. Use: create, update, list.", action),
                };
            }
            if name == "skill_use" {
                return self.handle_skill_use(arguments).await;
            }
            if name == "tool_discover" {
                return self.handle_tool_discover(arguments).await;
            }
            if name == "secret_set" {
                return self.handle_secret_set(arguments).await;
            }
            if name == "session" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");
                return match action {
                    "list" => self.handle_sessions_list(arguments).await,
                    "history" => self.handle_sessions_history(arguments).await,
                    "search" => self.handle_session_search(arguments).await,
                    "context" => self.handle_session_context(arguments).await,
                    "send" => self.handle_session_send(arguments).await,
                    "export" => self.handle_session_export(arguments).await,
                    _ => format!("Error: unknown session action '{}'. Use: list, history, search, context, send, export.", action),
                };
            }
            if name == "agents_list" {
                return self.handle_agents_list(arguments).await;
            }
            if name == "browser_action" {
                return self.handle_browser_action(arguments).await;
            }
            // service_manage and service_exec removed — base agent uses code_exec on host
            if name == "code_exec" {
                return self.handle_code_exec(arguments).await;
            }
            if name == "git" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");

                // Clone is special — doesn't need existing git dir
                if action == "clone" {
                    let url = match arguments.get("url").and_then(|v| v.as_str()).filter(|u| !u.is_empty()) {
                        Some(u) => u.to_string(),
                        None => return "Error: url parameter required.".to_string(),
                    };
                    let url = if url.starts_with("https://github.com/") {
                        url.replace("https://github.com/", "git@github.com:")
                    } else { url };
                    let dir_name = arguments.get("directory").and_then(|v| v.as_str()).filter(|d| !d.is_empty())
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| {
                            url.rsplit('/').next().or_else(|| url.rsplit(':').next())
                                .unwrap_or("repo").trim_end_matches(".git").to_string()
                        });
                    let target = std::path::PathBuf::from(&self.workspace_dir).join(&dir_name);
                    if target.exists() {
                        return format!("Error: directory '{}' already exists in workspace.", dir_name);
                    }
                    let output = tokio::process::Command::new("git")
                        .args(["clone", &url, &target.to_string_lossy()])
                        .output().await;
                    return match output {
                        Ok(o) => {
                            let stdout = String::from_utf8_lossy(&o.stdout);
                            let stderr = String::from_utf8_lossy(&o.stderr);
                            if o.status.success() { format!("Cloned {} into {}\n{}{}", url, dir_name, stdout, stderr) }
                            else { format!("git clone failed:\n{}{}", stdout, stderr) }
                        }
                        Err(e) => format!("Error running git clone: {}", e),
                    };
                }

                // All other actions need a git working directory
                let git_dir = match arguments.get("directory").and_then(|v| v.as_str()).filter(|d| !d.is_empty()) {
                    Some(sub) => {
                        let p = std::path::PathBuf::from(&self.workspace_dir).join(sub);
                        if !p.exists() || !p.is_dir() { return format!("Error: directory '{}' not found in workspace.", sub); }
                        p.to_string_lossy().to_string()
                    }
                    None => {
                        let ws = std::path::PathBuf::from(&self.workspace_dir);
                        if !ws.join(".git").exists() {
                            let mut git_dirs = Vec::new();
                            if let Ok(mut entries) = tokio::fs::read_dir(&ws).await {
                                while let Ok(Some(entry)) = entries.next_entry().await {
                                    let p = entry.path();
                                    if p.is_dir() && p.join(".git").exists()
                                        && let Some(dn) = p.file_name().and_then(|n| n.to_str()) { git_dirs.push(dn.to_string()); }
                                }
                            }
                            if !git_dirs.is_empty() {
                                return format!("Error: workspace root is not a git repo. Use directory parameter. Found: {}", git_dirs.join(", "));
                            }
                            return "Error: no git repository found in workspace.".to_string();
                        }
                        ws.to_string_lossy().to_string()
                    }
                };

                return match action {
                    "commit" => {
                        let message = arguments.get("message").and_then(|v| v.as_str()).unwrap_or("chore: update files");
                        match tokio::process::Command::new("git").args(["commit", "-am", message]).current_dir(&git_dir).output().await {
                            Ok(o) => { let s = String::from_utf8_lossy(&o.stdout); let e = String::from_utf8_lossy(&o.stderr);
                                if o.status.success() { s.to_string() } else { format!("git commit failed: {}{}", s, e) } }
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "log" => {
                        let limit = arguments.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
                        let oneline = arguments.get("oneline").and_then(|v| v.as_bool()).unwrap_or(true);
                        let mut args = vec!["log".to_string(), format!("-{}", limit)];
                        if oneline { args.push("--oneline".to_string()); }
                        else { args.push("--format=%h %ad %an: %s".to_string()); args.push("--date=short".to_string()); }
                        match tokio::process::Command::new("git").args(&args).current_dir(&git_dir).output().await {
                            Ok(o) => { let out = String::from_utf8_lossy(&o.stdout).to_string();
                                if out.is_empty() { "No commits found.".to_string() } else { out } }
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "add" => {
                        let files: Vec<String> = arguments.get("files").and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|f| f.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                        if files.is_empty() { return "Error: files parameter required.".to_string(); }
                        let mut args = vec!["add".to_string()]; args.extend(files);
                        match tokio::process::Command::new("git").args(&args).current_dir(&git_dir).output().await {
                            Ok(o) => if o.status.success() { let s = String::from_utf8_lossy(&o.stdout);
                                if s.is_empty() { "Files staged.".to_string() } else { s.to_string() } }
                                else { format!("git add failed: {}", String::from_utf8_lossy(&o.stderr)) }
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "branch" => {
                        let branch_act = arguments.get("branch_action").and_then(|v| v.as_str()).unwrap_or("list");
                        let branch_name = arguments.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let args: Vec<&str> = match branch_act {
                            "list" => vec!["branch", "-a"],
                            "create" => { if branch_name.is_empty() { return "Error: name required.".to_string(); } vec!["checkout", "-b", branch_name] }
                            "switch" => { if branch_name.is_empty() { return "Error: name required.".to_string(); } vec!["checkout", branch_name] }
                            "delete" => { if branch_name.is_empty() { return "Error: name required.".to_string(); } vec!["branch", "-d", branch_name] }
                            _ => return format!("Error: unknown branch_action '{}'.", branch_act),
                        };
                        match tokio::process::Command::new("git").args(&args).current_dir(&git_dir).output().await {
                            Ok(o) => { let mut out = String::from_utf8_lossy(&o.stdout).to_string();
                                let stderr = String::from_utf8_lossy(&o.stderr); if !stderr.is_empty() { out.push_str(&stderr); }
                                if out.is_empty() { format!("Exit code: {}", o.status.code().unwrap_or(-1)) } else { out } }
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "status" | "diff" | "push" | "pull" => {
                        match tokio::process::Command::new("git").args([action]).current_dir(&git_dir).output().await {
                            Ok(o) => { let mut out = String::from_utf8_lossy(&o.stdout).to_string();
                                let stderr = String::from_utf8_lossy(&o.stderr);
                                if !stderr.is_empty() { out.push_str("\n--- stderr ---\n"); out.push_str(&stderr); }
                                if out.is_empty() { format!("Exit code: {}", o.status.code().unwrap_or(-1)) } else { out } }
                            Err(e) => format!("Error running git {}: {}", action, e),
                        }
                    }
                    _ => format!("Error: unknown git action '{}'. Use: status, diff, log, commit, add, push, pull, branch, clone.", action),
                };
            }
            if name == "canvas" {
                return self.handle_canvas(arguments).await;
            }
            if name == "rich_card" {
                return self.handle_rich_card(arguments);
            }
            if name == "process" {
                let action = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("");
                return match action {
                    "start" => self.handle_process_start(arguments).await,
                    "status" => self.handle_process_status(arguments).await,
                    "logs" => self.handle_process_logs(arguments).await,
                    "kill" => self.handle_process_kill(arguments).await,
                    _ => format!("Error: unknown process action '{}'. Use: start, status, logs, kill.", action),
                };
            }
            // 2. YAML-defined tools (workspace/tools/) — only VERIFIED may be called directly.
            // Draft tools are blocked here; they can only be invoked through tool_test.
            if let Some(yaml_tool) = crate::tools::yaml_tools::find_yaml_tool(
                &self.workspace_dir,
                name,
            ).await {
                if yaml_tool.status == crate::tools::yaml_tools::ToolStatus::Draft {
                    return format!(
                        "Tool '{}' is in DRAFT status and cannot be called directly. \
                        Use tool_test(tool_name=\"{}\", test_params={{...}}) to test it, \
                        then tool_verify(tool_name=\"{}\") to promote it to verified.",
                        name, name, name
                    );
                }
                if yaml_tool.required_base && !self.agent.base {
                    return format!("Tool '{}' requires base agent.", name);
                }
                // GitHub repo access enforcement: tools starting with "github_" require allowed repo
                if name.starts_with("github_") {
                    let owner = arguments.get("owner").and_then(|v| v.as_str()).unwrap_or("");
                    let repo_name = arguments.get("repo").and_then(|v| v.as_str()).unwrap_or("");
                    if owner.is_empty() || repo_name.is_empty() {
                        return "GitHub tools require 'owner' and 'repo' parameters.".to_string();
                    }
                    match crate::db::github::check_repo_access(&self.db, &self.agent.name, owner, repo_name).await {
                        Ok(true) => {} // allowed
                        Ok(false) => {
                            return format!(
                                "Repository {}/{} is not in the allowed list for agent '{}'. \
                                Add it via POST /api/agents/{}/github/repos",
                                owner, repo_name, self.agent.name, self.agent.name
                            );
                        }
                        Err(e) => {
                            return format!("Error checking repo access: {}", e);
                        }
                    }
                }
                if let Some(ref ca) = yaml_tool.channel_action.clone() {
                    return self.execute_yaml_channel_action(&yaml_tool, arguments, ca).await;
                }
                if CACHEABLE_SEARCH_TOOLS.contains(&name)
                    && let Some(q) = arguments.get("query").and_then(|v| v.as_str())
                    && let Some(cached) = self.check_search_cache(q).await
                {
                    return cached;
                }
                let resolver = self.make_resolver();
                let oauth_ctx = self.make_oauth_context();
                // Internal endpoints (toolgate, searxng, browser-renderer) bypass SSRF filtering
                let client = if crate::tools::ssrf::is_internal_endpoint(&yaml_tool.endpoint) {
                    self.http_client()
                } else {
                    self.ssrf_http_client()
                };
                return match yaml_tool.execute_oauth(arguments, client, Some(&resolver), oauth_ctx.as_ref()).await {
                    Ok(result) => {
                        if CACHEABLE_SEARCH_TOOLS.contains(&name)
                            && let Some(q) = arguments.get("query").and_then(|v| v.as_str())
                        {
                            self.store_search_cache(q, &result).await;
                        }
                        result
                    },
                    Err(e) => Self::format_tool_error(name, &e.to_string()),
                };
            }

            // 3. MCP tools (via MCP)
            if let Some(mcp) = self.mcp()
                && let Some(mcp_name) = mcp.find_mcp_for_tool(name).await {
                    return match mcp.call_tool(&mcp_name, name, arguments).await {
                        Ok(result) => result,
                        Err(e) => Self::format_tool_error(name, &e.to_string()),
                    };
                }

            // 5. External tools via ToolRegistry (fallback)
            match self.tools.call(name, arguments).await {
                Ok(result) => serde_json::to_string(&result).unwrap_or_default(),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("tool not found") {
                        tracing::warn!(tool = %name, "LLM called non-existent tool");
                        format!("Error: tool '{}' does not exist. Use tool_list to see available tools.", name)
                    } else {
                        Self::format_tool_error(name, &msg)
                    }
                }
            }
        })
    }


    /// Record LLM token usage to the database (fire-and-forget).
    pub(super) fn record_usage(&self, response: &hydeclaw_types::LlmResponse, session_id: Option<uuid::Uuid>) {
        if let Some(ref usage) = response.usage {
            let db = self.db.clone();
            let agent = self.agent.name.clone();
            let provider = response.provider.clone()
                .unwrap_or_else(|| self.provider.name().to_string());
            let model = response.model.clone().unwrap_or_default();
            let input = usage.input_tokens;
            let output = usage.output_tokens;
            tokio::spawn(async move {
                if let Err(e) = crate::db::usage::record_usage(
                    &db, &agent, &provider, &model, input, output, session_id,
                ).await {
                    tracing::debug!(error = %e, "failed to record usage");
                }
            });
        }
    }

    /// Filter tools based on per-agent allow/deny policy.
    /// Merge a cron-job tool policy override on top of the agent's base policy,
    /// then re-filter the already-filtered tool list.
    ///
    /// Logic:
    ///  - deny list is unioned (base deny ∪ override deny)
    ///  - allow list: if override has non-empty allow, restrict to those tools only (intersection with current list)
    pub(super) fn apply_tool_policy_override(
        &self,
        tools: Vec<ToolDefinition>,
        override_policy: &crate::config::AgentToolPolicy,
    ) -> Vec<ToolDefinition> {
        let base_deny = self.agent.tools.as_ref().map(|p| &p.deny);

        tools.into_iter().filter(|t| {
            // Union of deny lists
            if override_policy.deny.iter().any(|d| d == &t.name) {
                return false;
            }
            if let Some(bd) = base_deny
                && bd.iter().any(|d| d == &t.name) {
                    return false;
                }
            // If override has a non-empty allow list, restrict to those tools only
            if !override_policy.allow.is_empty() {
                return override_policy.allow.iter().any(|a| a == &t.name);
            }
            true
        }).collect()
    }

    pub(super) fn filter_tools_by_policy(&self, tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        let policy = match &self.agent.tools {
            Some(p) => p,
            None => return tools,
        };

        let before = tools.len();
        let filtered: Vec<ToolDefinition> = tools
            .into_iter()
            .filter(|t| {
                let name = t.name.as_str();

                // Check deny list first (applies to ALL tools including core)
                if policy.deny.iter().any(|d| d == name) {
                    return false;
                }

                // Core internal tools (workspace, memory, system) always allowed unless denied above
                if matches!(
                    name,
                    "workspace_write" | "workspace_read" | "workspace_list" | "workspace_edit" | "workspace_delete" | "workspace_rename" |
                    "web_fetch" | "agent" |
                    "message" | "cron" | "code_exec" | "browser_action" |
                    "git" | "session" | "skill" | "skill_use" |
                    "canvas" | "rich_card" | "agents_list" | "secret_set" |
                    "process" | "graph_query"
                ) {
                    return true;
                }

                // Memory tool requires memory_store to be available
                if name == "memory" {
                    return self.memory_store.is_available();
                }

                // Tool management tools
                if name.starts_with("tool_") {
                    return true;
                }
                // allow_all = everything not denied
                if policy.allow_all {
                    return true;
                }
                // deny_all_others = only explicitly allowed
                if policy.deny_all_others {
                    return policy.allow.iter().any(|a| a == &t.name);
                }
                // Non-empty allow list = only those
                if !policy.allow.is_empty() {
                    return policy.allow.iter().any(|a| a == &t.name);
                }
                true
            })
            .collect();

        if filtered.len() != before {
            tracing::info!(
                agent = %self.agent.name,
                before,
                after = filtered.len(),
                "tool policy applied"
            );
        }
        filtered
    }
}
