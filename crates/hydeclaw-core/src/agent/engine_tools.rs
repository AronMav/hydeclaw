//! YAML tool management, canvas, skill, and discovery meta-tools —
//! extracted from engine.rs for readability.

use super::*;

impl AgentEngine {
    // ── YAML Tool Management meta-tools ───────────────────────────────────────

    /// Internal tool: create a new YAML HTTP tool in draft status.
    pub(super) async fn handle_tool_create(&self, args: &serde_json::Value) -> String {
        use crate::tools::yaml_tools::{ToolStatus, tool_file_path};

        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => return "Error: 'name' is required".to_string(),
        };

        let valid = !name.is_empty()
            && name.len() <= 64
            && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            && name.starts_with(|c: char| c.is_ascii_lowercase());
        if !valid {
            return "Error: tool name must be snake_case (lowercase letters, digits, underscores, starting with a letter)".to_string();
        }

        let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let endpoint = match args.get("endpoint").and_then(|v| v.as_str()) {
            Some(e) if !e.is_empty() => e.to_string(),
            _ => return "Error: 'endpoint' is required".to_string(),
        };
        let method = args.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_uppercase();

        let mut yaml_parts = vec![
            format!("name: {}", name),
            format!("description: {:?}", description),
            format!("endpoint: {:?}", endpoint),
            format!("method: {}", method),
            "status: draft".to_string(),
            format!("created_by: agent"),
            format!("created_at: {:?}", chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        ];

        if let Some(tags) = args.get("tags").and_then(|v| v.as_array()) {
            let tag_list: Vec<String> = tags.iter()
                .filter_map(|t| t.as_str().map(|s| format!("  - {}", s)))
                .collect();
            if !tag_list.is_empty() {
                yaml_parts.push(format!("tags:\n{}", tag_list.join("\n")));
            }
        }

        if let Some(auth) = args.get("auth") {
            match serde_yaml::to_string(auth) {
                Ok(auth_yaml) => {
                    let indented = auth_yaml.lines()
                        .map(|l| format!("  {}", l))
                        .collect::<Vec<_>>()
                        .join("\n");
                    yaml_parts.push(format!("auth:\n{}", indented));
                }
                Err(e) => return format!("Error serializing auth: {}", e),
            }
        }

        if let Some(headers) = args.get("headers")
            && let Ok(h_yaml) = serde_yaml::to_string(headers) {
                let indented = h_yaml.lines()
                    .map(|l| format!("  {}", l))
                    .collect::<Vec<_>>()
                    .join("\n");
                yaml_parts.push(format!("headers:\n{}", indented));
            }

        if let Some(params) = args.get("parameters") {
            match serde_yaml::to_string(params) {
                Ok(p_yaml) => {
                    let indented = p_yaml.lines()
                        .map(|l| format!("  {}", l))
                        .collect::<Vec<_>>()
                        .join("\n");
                    yaml_parts.push(format!("parameters:\n{}", indented));
                }
                Err(e) => return format!("Error serializing parameters: {}", e),
            }
        }

        if let Some(tmpl) = args.get("body_template").and_then(|v| v.as_str()) {
            yaml_parts.push(format!("body_template: |\n{}", tmpl.lines().map(|l| format!("  {}", l)).collect::<Vec<_>>().join("\n")));
        }

        let yaml_content = yaml_parts.join("\n") + "\n";

        if let Err(e) = serde_yaml::from_str::<crate::tools::yaml_tools::YamlToolDef>(&yaml_content) { return format!("Error: generated YAML is invalid: {}\n\nYAML:\n{}", e, yaml_content) }

        let path = tool_file_path(&self.workspace_dir, &ToolStatus::Draft, &name);
        if let Some(parent) = path.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await {
                return format!("Error creating directory: {}", e);
            }
        match tokio::fs::write(&path, &yaml_content).await {
            Ok(_) => format!(
                "Tool '{}' created in DRAFT status.\nFile: tools/{}.yaml\n\nNext steps:\n1. Test it: tool_test(tool_name=\"{}\", params={{...}})\n2. Verify it: tool_verify(tool_name=\"{}\")",
                name, name, name, name
            ),
            Err(e) => format!("Error writing tool file: {}", e),
        }
    }

    /// Internal tool: list YAML tools by status.
    pub(super) async fn handle_tool_list(&self, args: &serde_json::Value) -> String {
        use crate::tools::yaml_tools::{load_all_yaml_tools, ToolStatus};

        let status_filter = args.get("status").and_then(|v| v.as_str()).unwrap_or("all");

        let all_tools = load_all_yaml_tools(&self.workspace_dir).await;

        let tools: Vec<_> = all_tools.iter().filter(|t| {
            match status_filter {
                "verified" => t.status == ToolStatus::Verified,
                "draft" => t.status == ToolStatus::Draft,
                "disabled" => t.status == ToolStatus::Disabled,
                _ => true,
            }
        }).collect();

        if tools.is_empty() {
            return format!("No {} tools found.", status_filter);
        }

        let lines: Vec<String> = tools.iter().map(|t| {
            let status_icon = match t.status {
                ToolStatus::Verified => "✅",
                ToolStatus::Draft => "✏️",
                ToolStatus::Disabled => "🚫",
            };
            format!("{} **{}** — {}\n   `{} {}`",
                status_icon, t.name, t.description, t.method, t.endpoint)
        }).collect();

        format!("**YAML Tools** ({} {}):\n\n{}", tools.len(), status_filter, lines.join("\n\n"))
    }

    /// Internal tool: test a YAML tool (including draft) with specific parameters.
    pub(super) async fn handle_tool_test(&self, args: &serde_json::Value) -> String {
        let tool_name = match args.get("tool_name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => return "Error: 'tool_name' is required".to_string(),
        };
        let params = args.get("params").cloned().unwrap_or(serde_json::Value::Object(Default::default()));
        let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);

        let tool = match crate::tools::yaml_tools::find_yaml_tool(
            &self.workspace_dir, tool_name,
        ).await {
            Some(t) => t,
            None => return format!("Tool '{}' not found. Use tool_list() to see available tools.", tool_name),
        };

        if dry_run {
            return format!(
                "**Dry run for '{}'** (status: {:?})\n\nEndpoint: {} {}\nAuth: {:?}\nParameters: {}\n\nWould send params: {}",
                tool.name,
                tool.status,
                tool.method,
                tool.endpoint,
                tool.auth.as_ref().map(|a| &a.auth_type),
                serde_json::to_string_pretty(&tool.parameters.keys().collect::<Vec<_>>()).unwrap_or_default(),
                serde_json::to_string_pretty(&params).unwrap_or_default(),
            );
        }

        let resolver = self.make_resolver();
        let oauth_ctx = self.make_oauth_context();
        let start = std::time::Instant::now();
        // Internal endpoints (toolgate, searxng, etc.) bypass SSRF filtering
        let client = if crate::tools::ssrf::is_internal_endpoint(&tool.endpoint) {
            &self.http_client
        } else {
            &self.ssrf_http_client
        };
        let result = tool.execute_oauth(&params, client, Some(&resolver), oauth_ctx.as_ref()).await;
        let elapsed_ms = start.elapsed().as_millis();

        match result {
            Ok(body) => format!(
                "**tool_test('{}')** ✅ ({} ms)\n\nResponse:\n```\n{}\n```",
                tool_name,
                elapsed_ms,
                if body.len() > 2000 { &body[..body.floor_char_boundary(2000)] } else { &body },
            ),
            Err(e) => format!(
                "**tool_test('{}')** ❌ ({} ms)\n\nError: {}",
                tool_name, elapsed_ms, e
            ),
        }
    }

    /// Internal tool: promote a draft tool to verified status.
    pub(super) async fn handle_tool_verify(&self, args: &serde_json::Value) -> String {
        use crate::tools::yaml_tools::{ToolStatus, tool_file_path};
        use regex::Regex;

        let tool_name = match args.get("tool_name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => return "Error: 'tool_name' is required".to_string(),
        };

        let path = tool_file_path(&self.workspace_dir, &ToolStatus::Draft, tool_name);
        if !path.exists() {
            return format!("Tool '{}' not found. Use tool_list(status=\"draft\") to see draft tools.", tool_name);
        }

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return format!("Error reading tool file: {}", e),
        };

        let status_re = Regex::new(r"(?m)^status:\s*verified\s*$").unwrap();
        if status_re.is_match(&content) {
            return format!("Tool '{}' is already verified.", tool_name);
        }

        let draft_re = Regex::new(r"(?m)^status:\s*draft\s*$").unwrap();
        let updated = draft_re.replace(&content, "status: verified").to_string();
        if let Err(e) = tokio::fs::write(&path, &updated).await {
            return format!("Error writing tool file: {}", e);
        }

        format!(
            "Tool '{}' is now VERIFIED ✅\nIt will appear in LLM context on next request.",
            tool_name
        )
    }

    /// Internal tool: move a tool to disabled status.
    pub(super) async fn handle_tool_disable(&self, args: &serde_json::Value) -> String {
        use crate::tools::yaml_tools::{ToolStatus, tool_file_path};
        use regex::Regex;

        let tool_name = match args.get("tool_name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => return "Error: 'tool_name' is required".to_string(),
        };

        // Check both verified and draft paths (tool could be in either status)
        let verified_path = tool_file_path(&self.workspace_dir, &ToolStatus::Verified, tool_name);
        let draft_path = tool_file_path(&self.workspace_dir, &ToolStatus::Draft, tool_name);
        let path = if verified_path.exists() {
            verified_path
        } else if draft_path.exists() {
            draft_path
        } else {
            return format!("Tool '{}' not found.", tool_name);
        };

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return format!("Error reading tool file: {}", e),
        };

        let status_re = Regex::new(r"(?m)^status:\s*(verified|draft)\s*$").unwrap();
        let updated = status_re.replace(&content, "status: disabled").to_string();

        if let Err(e) = tokio::fs::write(&path, &updated).await {
            return format!("Error writing tool file: {}", e);
        }

        format!("Tool '{}' disabled 🚫\nIt will not appear in LLM context.", tool_name)
    }

    // ── Canvas ───────────────────────────────────────────────────────────────

    /// Canvas tool: present/push_data/clear push to UI; run_js/snapshot use browser-renderer.
    pub(super) async fn handle_canvas(&self, args: &serde_json::Value) -> String {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("present");
        match action {
            "present" | "push_data" => {
                let (ct, content, title) = if action == "push_data" {
                    ("json", args.get("content").and_then(|v| v.as_str()).unwrap_or("{}"), None)
                } else {
                    (
                        args.get("content_type").and_then(|v| v.as_str()).unwrap_or("markdown"),
                        args.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                        args.get("title").and_then(|v| v.as_str()),
                    )
                };
                if content.len() > CANVAS_MAX_BYTES {
                    return format!("Error: content too large ({} bytes, max {CANVAS_MAX_BYTES})", content.len());
                }
                *self.canvas_state.write().await = Some(CanvasContent {
                    content_type: ct.to_string(),
                    content: content.to_string(),
                    title: title.map(|s| s.to_string()),
                });
                let event = serde_json::json!({
                    "type": "canvas_update",
                    "agent": self.agent.name,
                    "action": action,
                    "content_type": ct,
                    "content": content,
                    "title": title,
                });
                self.broadcast_ui_event(event);
                "Canvas updated".to_string()
            }
            "clear" => {
                *self.canvas_state.write().await = None;
                self.broadcast_ui_event(serde_json::json!({
                    "type": "canvas_update",
                    "agent": self.agent.name,
                    "action": "clear",
                }));
                "Canvas cleared".to_string()
            }
            "run_js" => {
                let code = match args.get("code").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => return "Error: 'code' parameter is required for run_js".to_string(),
                };
                self.canvas_run_js(code).await
            }
            "snapshot" => {
                self.canvas_snapshot().await
            }
            other => format!("Unknown canvas action: {other}"),
        }
    }

    /// POST JSON to browser-renderer and return the parsed response body.
    pub(super) async fn br_post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
        let br_url = Self::browser_renderer_url();
        let resp = self.http_client
            .post(format!("{br_url}{path}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Cannot reach browser-renderer: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("browser-renderer {status}: {body}"));
        }
        resp.json::<serde_json::Value>().await
            .map_err(|e| format!("Error parsing browser-renderer response: {e}"))
    }

    /// Resolve the navigable URL for the current canvas content.
    pub(super) async fn canvas_resolve_url(&self) -> Result<String, String> {
        let state_guard = self.canvas_state.read().await;
        let state = state_guard.as_ref()
            .ok_or_else(|| "Error: no content on canvas. Use canvas(action='present') first.".to_string())?
            .clone();
        drop(state_guard);
        Self::canvas_content_url(&state)
    }

    /// Execute JavaScript in the current canvas content via browser-renderer.
    pub(super) async fn canvas_run_js(&self, code: &str) -> String {
        let url = match self.canvas_resolve_url().await {
            Ok(u) => u,
            Err(e) => return e,
        };

        let session_id = match self.br_post("/automation", serde_json::json!({"action": "create_session"})).await {
            Ok(v) => v.get("session_id").and_then(|s| s.as_str()).unwrap_or("").to_string(),
            Err(e) => return e,
        };

        if let Err(e) = self.br_post("/automation", serde_json::json!({
            "action": "navigate", "session_id": session_id, "url": url, "timeout": 15,
        })).await {
            let _ = self.br_post("/automation", serde_json::json!({"action": "close", "session_id": session_id})).await;
            return format!("Error navigating: {e}");
        }

        let result = match self.br_post("/automation", serde_json::json!({
            "action": "evaluate", "session_id": session_id, "js": code,
        })).await {
            Ok(v) => {
                let res = &v["result"];
                if res.is_string() { res.as_str().unwrap().to_string() }
                else { serde_json::to_string(res).unwrap_or_default() }
            }
            Err(e) => format!("JS execution error: {e}"),
        };

        let _ = self.br_post("/automation", serde_json::json!({"action": "close", "session_id": session_id})).await;
        result
    }

    /// Take a screenshot of the current canvas content via browser-renderer.
    pub(super) async fn canvas_snapshot(&self) -> String {
        let url = match self.canvas_resolve_url().await {
            Ok(u) => u,
            Err(e) => return e,
        };

        let br_url = Self::browser_renderer_url();
        let resp = match self.http_client
            .post(format!("{br_url}/screenshot"))
            .json(&serde_json::json!({"url": url, "timeout": 15}))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return format!("Cannot reach browser-renderer: {e}"),
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return format!("browser-renderer {status}: {body}");
        }
        let len = resp.content_length().unwrap_or(0);
        let _ = resp.bytes().await;
        format!("Screenshot captured (PNG, {len} bytes).")
    }

    /// Get a URL that browser-renderer can navigate to for the current canvas content.
    pub(super) fn canvas_content_url(state: &CanvasContent) -> Result<String, String> {
        match state.content_type.as_str() {
            "url" => Ok(state.content.clone()),
            "html" => {
                use base64::Engine;
                Ok(format!(
                    "data:text/html;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&state.content)
                ))
            }
            other => Err(format!(
                "Error: run_js/snapshot not supported for content_type '{other}'. Use 'html' or 'url'."
            )),
        }
    }

    /// Resolve browser-renderer service URL.
    pub(super) fn browser_renderer_url() -> String {
        std::env::var("BROWSER_RENDERER_URL")
            .unwrap_or_else(|_| "http://localhost:9020".to_string())
    }

    // ── Rich cards ───────────────────────────────────────────────────────────

    /// Return a `__rich_card__:` marker so the SSE handler emits a RichCard event inline.
    pub(super) fn handle_rich_card(&self, args: &serde_json::Value) -> String {
        let card_type = args.get("card_type").and_then(|v| v.as_str()).unwrap_or("table");
        match card_type {
            "table" | "metric" => {}
            other => return format!("Unknown rich_card type: {other}"),
        }
        format!("{RICH_CARD_PREFIX}{}", serde_json::to_string(args).unwrap_or_default())
    }

    // ── Secrets ──────────────────────────────────────────────────────────────

    pub(super) async fn handle_secret_set(&self, args: &serde_json::Value) -> String {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n,
            _ => return "Error: 'name' is required".to_string(),
        };
        let value = match args.get("value").and_then(|v| v.as_str()) {
            Some(v) if !v.is_empty() => v,
            _ => return "Error: 'value' is required".to_string(),
        };
        let description = args.get("description").and_then(|v| v.as_str());
        let global = args.get("global").and_then(|v| v.as_bool()).unwrap_or(false);

        // Only base agents can set global secrets (prevents credential substitution attacks)
        if global && !self.agent.base {
            return "Error: only base agents can set global secrets. Use scoped secrets or ask Hyde.".to_string();
        }

        let result = if global {
            self.secrets.set(name, value, description).await
        } else {
            self.secrets.set_scoped(name, &self.agent.name, value, description).await
        };

        match result {
            Ok(()) => {
                let scope_label = if global { "global" } else { &self.agent.name };
                format!("Secret '{}' saved (scope: {}). It is now available for YAML tool auth.", name, scope_label)
            }
            Err(e) => format!("Error saving secret: {}", e),
        }
    }

    // ── Skills ───────────────────────────────────────────────────────────────

    /// Skill meta-tool: create a new skill scenario.
    pub(super) async fn handle_skill_create(&self, args: &serde_json::Value) -> String {
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => return "Error: 'name' is required".to_string(),
        };
        let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let triggers: Vec<String> = args
            .get("triggers")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let tools_required: Vec<String> = args
            .get("tools_required")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let instructions = match args.get("instructions").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return "Error: 'instructions' is required".to_string(),
        };
        let priority = args.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        let frontmatter = crate::skills::SkillFrontmatter {
            name: name.clone(),
            description,
            triggers,
            tools_required,
            priority,
        };

        match crate::skills::write_skill(
            &self.workspace_dir,
            &name,
            &frontmatter,
            &instructions,
        ).await {
            Ok(()) => format!("Skill '{}' created in skills/{}.md", name, name.replace(' ', "-")),
            Err(e) => format!("Error creating skill '{}': {}", name, e),
        }
    }

    /// Skill meta-tool: update an existing skill by overwriting it.
    pub(super) async fn handle_skill_update(&self, args: &serde_json::Value) -> String {
        self.handle_skill_create(args).await
    }

    /// Skill use: on-demand skill loading (list catalog or load full instructions).
    pub(super) async fn handle_skill_use(&self, args: &serde_json::Value) -> String {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");
        let skills = if self.agent.base {
            crate::skills::load_skills_for_base(&self.workspace_dir).await
        } else {
            crate::skills::load_skills(&self.workspace_dir).await
        };

        match action {
            "list" => {
                if skills.is_empty() {
                    return "No skills available.".to_string();
                }
                let mut out = String::from("Available skills:\n\n");
                for s in &skills {
                    out.push_str(&format!("- **{}** — {}", s.meta.name, s.meta.description));
                    if !s.meta.triggers.is_empty() {
                        out.push_str(&format!(" (use when: {})", s.meta.triggers.join(", ")));
                    }
                    out.push('\n');
                }
                out
            }
            "load" => {
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() {
                    return "Error: 'name' parameter required for load action.".to_string();
                }
                match skills.iter().find(|s| s.meta.name == name) {
                    Some(skill) => {
                        format!("## Skill: {}\n{}\n\n{}", skill.meta.name, skill.meta.description, skill.instructions)
                    }
                    None => {
                        let available: Vec<&str> = skills.iter().map(|s| s.meta.name.as_str()).collect();
                        format!("Skill '{}' not found. Available: {}", name, available.join(", "))
                    }
                }
            }
            _ => format!("Error: unknown action '{}'. Use: list, load.", action),
        }
    }

    /// Skill meta-tool: list available skills.
    pub(super) async fn handle_skill_list(&self, _args: &serde_json::Value) -> String {
        let skills = if self.agent.base {
            crate::skills::load_skills_for_base(&self.workspace_dir).await
        } else {
            crate::skills::load_skills(&self.workspace_dir).await
        };
        if skills.is_empty() {
            return "No skills found in workspace/skills/".to_string();
        }
        let mut out = format!("Skills ({}):\n", skills.len());
        for s in &skills {
            out.push_str(&format!(
                "- **{}** (priority: {}): {}\n  Triggers: {}\n  Tools: {}\n",
                s.meta.name,
                s.meta.priority,
                s.meta.description,
                s.meta.triggers.join(", "),
                if s.meta.tools_required.is_empty() { "all".to_string() } else { s.meta.tools_required.join(", ") },
            ));
        }
        out
    }

    // ── OpenAPI discovery ────────────────────────────────────────────────────

    /// Tool meta: discover and create draft tools from an OpenAPI/Swagger spec URL.
    pub(super) async fn handle_tool_discover(&self, args: &serde_json::Value) -> String {
        let spec_url = match args.get("spec_url").and_then(|v| v.as_str()) {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => return "Error: 'spec_url' is required".to_string(),
        };
        let prefix = args.get("prefix").and_then(|v| v.as_str()).unwrap_or("").to_string();

        // Use SSRF-safe client to prevent LLM-directed requests to internal services
        let spec_text = match self.ssrf_http_client
            .get(&spec_url)
            .header("Accept", "application/json, application/yaml, */*")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
        {
            Ok(r) => match r.text().await {
                Ok(t) => t,
                Err(e) => return format!("Error reading spec: {}", e),
            },
            Err(e) => return format!("Error fetching spec: {}", e),
        };

        let spec: serde_json::Value = if let Ok(v) = serde_json::from_str(&spec_text) {
            v
        } else if let Ok(v) = serde_yaml::from_str::<serde_json::Value>(&spec_text) {
            v
        } else {
            return "Error: could not parse spec as JSON or YAML".to_string();
        };

        let base_url = discover_base_url(&spec, &spec_url);
        let tools = extract_openapi_tools(&spec, &base_url, &prefix);
        if tools.is_empty() {
            return "No API operations found in spec. Make sure it's a valid OpenAPI 2.x/3.x spec.".to_string();
        }

        let draft_dir = std::path::Path::new(&self.workspace_dir)
            .join("tools")
            .join("draft");
        tokio::fs::create_dir_all(&draft_dir).await.ok();

        let mut created = Vec::new();
        let mut errors = Vec::new();

        for tool in &tools {
            let yaml = match serde_yaml::to_string(tool) {
                Ok(y) => y,
                Err(e) => { errors.push(format!("{}: {}", tool.name, e)); continue; }
            };
            let path = draft_dir.join(format!("{}.yaml", tool.name));
            match tokio::fs::write(&path, &yaml).await {
                Ok(_) => created.push(tool.name.clone()),
                Err(e) => errors.push(format!("{}: {}", tool.name, e)),
            }
        }

        let mut out = format!(
            "Discovered {} tools from {}\nCreated {} draft tools:\n",
            tools.len(), spec_url, created.len()
        );
        for name in &created {
            out.push_str(&format!("- {} (draft)\n", name));
        }
        if !errors.is_empty() {
            out.push_str("\nErrors:\n");
            for e in &errors { out.push_str(&format!("- {}\n", e)); }
        }
        out.push_str("\nUse tool_test to verify, then tool_verify to activate.");
        out
    }
}
