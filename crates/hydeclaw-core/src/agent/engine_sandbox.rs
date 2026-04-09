//! Sandbox execution, host code exec, and background process tools.
//! This is the SandboxExecutor module -- satisfies ENG-04.

use super::*;

impl AgentEngine {
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
        if self.agent.base && self.sandbox().is_none() {
            return self.execute_host_code(code, language, &packages).await;
        }

        let sandbox = match self.sandbox() {
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


    // в”Ђв”Ђ Background process tools (base agents only) в”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђв”Ђ

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
            let mut procs = self.tex().bg_processes.lock().await;
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
            let mut procs = self.tex().bg_processes.lock().await;
            procs.retain(|_id, p| {
                p.pid.is_some_and(|pid| std::path::Path::new(&format!("/proc/{}", pid)).exists())
            });
        }

        let process_id = match args.get("process_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return "Error: 'process_id' is required".to_string(),
        };

        let (pid, log_path) = {
            let procs = self.tex().bg_processes.lock().await;
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
            let procs = self.tex().bg_processes.lock().await;
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
            let procs = self.tex().bg_processes.lock().await;
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

    pub async fn handle_openai(
        &self,
        openai_messages: &[crate::gateway::OpenAiMessage],
        chunk_tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        // 1. Build tool list (same as build_context but without session)
        let yaml_tools = crate::tools::yaml_tools::load_yaml_tools(&self.workspace_dir, false).await;
        let mut raw_tools = self.internal_tool_definitions();
        raw_tools.extend(yaml_tools.into_iter().map(|t| t.to_tool_definition()));
        if let Some(mcp) = self.mcp() {
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

            let mcp_schemas: Vec<String> = if let Some(mcp) = self.mcp() {
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
                has_host_exec: self.agent.base && self.sandbox().is_none(),
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
            finish_reason: None,
            model: None,
            provider: None,
            fallback_notice: None,
            tools_used: tools_used_acc,
            iterations: final_iteration,
            thinking_blocks: vec![],
        })
    }
}
