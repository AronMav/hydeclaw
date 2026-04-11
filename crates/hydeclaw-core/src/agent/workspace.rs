use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Workspace file order for system prompt assembly (per-agent files).
const WORKSPACE_FILES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "MEMORY.md",
];

/// Shared files loaded from workspace root (same content for all agents).
const SHARED_ROOT_PROMPT_FILES: &[&str] = &["TOOLS.md", "AGENTS.md", "USER.md"];

/// Directories excluded from memory indexing — system/binary/config dirs not meant for knowledge base.
pub const MEMORY_INDEX_EXCLUDE_DIRS: &[&str] = &["tools", "skills", "mcp", "uploads", "agents"];

/// Resolve the per-agent workspace directory: `{workspace_dir}/agents/{agent_name}`.
fn agent_dir(workspace_dir: &str, agent_name: &str) -> PathBuf {
    Path::new(workspace_dir).join("agents").join(agent_name)
}

// ── Capability flags ────────────────────────────────────────────────────────

/// Flags indicating which optional capabilities are configured for this deployment.
pub struct CapabilityFlags {
    pub has_search: bool,
    pub has_memory: bool,
    pub has_message_actions: bool,
    pub has_cron: bool,
    pub has_yaml_tools: bool,
    pub has_browser: bool,
    pub has_host_exec: bool,
    pub is_base: bool,
}

/// A channel available to this agent (for system prompt injection).
#[derive(Clone)]
pub struct ChannelInfo {
    pub channel_id: String,
    pub channel_type: String,
    pub display_name: String,
    pub online: bool,
}

/// Runtime context injected into system prompt (not from workspace files).
pub struct RuntimeContext {
    pub agent_name: String,
    pub owner_id: Option<String>,
    pub channel: String,
    pub model: String,
    /// e.g. "2026-03-13 14:30 (Europe/Samara, UTC+4)"
    pub datetime_display: String,
    /// Channel-specific formatting instructions (from channel adapter Ready message).
    /// Only set when the message arrives through a connected channel.
    pub formatting_prompt: Option<String>,
    /// Connected channels for this agent (injected into system prompt).
    pub channels: Vec<ChannelInfo>,
}

/// Format current datetime for system prompt display.
pub fn format_local_datetime(timezone: &str) -> String {
    let offset = crate::scheduler::timezone_offset_hours(timezone);
    let utc_now = chrono::Utc::now();
    let local = utc_now + chrono::Duration::hours(offset as i64);
    format!(
        "{} ({}, UTC{:+})",
        local.format("%Y-%m-%d %H:%M"),
        timezone,
        offset
    )
}

/// Workspace root files that agents cannot modify regardless of base status.
const READ_ONLY_FILES: &[&str] = &["AGENTS.md"];

/// Workspace root files that only base agents can modify.
const PRIVILEGED_ROOT_FILES: &[&str] = &["TOOLS.md"];

// Service dirs (toolgate/, channels/) and tools/ are base-only.
// Non-base agents cannot create tools or modify service code.

/// Tool definitions directory — only base agents can create/modify YAML tools.
const TOOLS_DIR: &str = "tools";

/// Per-agent identity files that cannot be deleted (but can be edited).
const IDENTITY_FILES: &[&str] = &["SOUL.md", "IDENTITY.md", "MEMORY.md", "HEARTBEAT.md"];

/// Extract the filename component from a path (e.g. "agents/main/SOUL.md" → "SOUL.md").
fn file_basename(path: &str) -> &str {
    Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("")
}

/// Check if a resolved path points to a read-only or protected file.
///
/// `base`: if true, agent is a system (base) agent — can write to service source files
///   and tools, but SOUL.md and IDENTITY.md are read-only (protected system prompt files).
fn is_read_only(workspace_dir: &str, resolved: &Path, base: bool) -> bool {
    let root = Path::new(workspace_dir);
    // Root-level read-only files (blocked for all agents)
    if READ_ONLY_FILES.iter().any(|name| resolved == root.join(name)) {
        return true;
    }
    // Root-level base files (only base agents can modify)
    if !base && PRIVILEGED_ROOT_FILES.iter().any(|name| resolved == root.join(name)) {
        return true;
    }
    // Base agent: SOUL.md and IDENTITY.md are always read-only (even for the agent itself).
    // Paths without a filename component are treated as read-only as a safe default.
    if base {
        match resolved.file_name().and_then(|n| n.to_str()) {
            Some("SOUL.md" | "IDENTITY.md") => return true,
            None => return true,
            _ => {}
        }
    }
    // Tools directory — only base agents can create/modify YAML tools
    let tools_root = root.join(TOOLS_DIR);
    if resolved.starts_with(&tools_root) {
        return !base;
    }

    // toolgate/ and channels/ no longer in workspace — base agent uses code_exec on host
    false
}

// ── Prompt assembly ─────────────────────────────────────────────────────────

/// Maximum bytes per workspace file included in system prompt.
/// Files exceeding this are truncated with a warning to the LLM.
const MAX_PROMPT_FILE_BYTES: usize = 12 * 1024; // 12 KB

/// Append file content to prompt, truncating if over the size limit.
fn append_with_limit(prompt: &mut String, content: &str, filename: &str) {
    if content.trim().is_empty() {
        return;
    }
    if content.len() <= MAX_PROMPT_FILE_BYTES {
        prompt.push_str(content);
    } else {
        let boundary = content.floor_char_boundary(MAX_PROMPT_FILE_BYTES);
        prompt.push_str(&content[..boundary]);
        prompt.push_str(&format!(
            "\n\n[{}: truncated at {} KB — keep this file concise]\n",
            filename,
            MAX_PROMPT_FILE_BYTES / 1024
        ));
        tracing::warn!(file = %filename, bytes = content.len(), "workspace file truncated for system prompt");
    }
    prompt.push('\n');
}

/// Read all workspace files for an agent and build the workspace portion of the system prompt.
pub async fn load_workspace_prompt(workspace_dir: &str, agent_name: &str) -> Result<String> {
    let dir = agent_dir(workspace_dir, agent_name);
    let mut prompt = String::new();

    // 1. Load priority files first (SOUL, IDENTITY, MEMORY) in defined order
    for file in WORKSPACE_FILES {
        let path = dir.join(file);
        match fs::read_to_string(&path).await {
            Ok(content) => append_with_limit(&mut prompt, &content, file),
            Err(_) => {
                tracing::debug!(file = %path.display(), "workspace file not found, skipping");
            }
        }
    }

    // 2. Load all other .md files from agent dir (guides, notes, etc.)
    //    Skip files already loaded above + HEARTBEAT.md (loaded separately by scheduler).
    if let Ok(mut entries) = fs::read_dir(&dir).await {
        let mut extra_files: Vec<String> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md")
                && !WORKSPACE_FILES.contains(&name.as_str())
                && name != "HEARTBEAT.md"
            {
                extra_files.push(name);
            }
        }
        extra_files.sort();
        for file in &extra_files {
            let path = dir.join(file);
            if let Ok(content) = fs::read_to_string(&path).await { append_with_limit(&mut prompt, &content, file) }
        }
    }

    // 3. Shared files from workspace root (same for all agents)
    for file in SHARED_ROOT_PROMPT_FILES {
        let path = Path::new(workspace_dir).join(file);
        match fs::read_to_string(&path).await {
            Ok(content) => append_with_limit(&mut prompt, &content, file),
            Err(_) => {
                tracing::debug!(file, "workspace root file not found, skipping");
            }
        }
    }

    Ok(prompt)
}

/// Map language code to full name for LLM instructions.
fn language_name(code: &str) -> &'static str {
    match code {
        "ru" => "Russian",
        "en" => "English",
        "es" => "Spanish",
        "de" => "German",
        "fr" => "French",
        "zh" => "Chinese",
        "ja" => "Japanese",
        "ko" => "Korean",
        "pt" => "Portuguese",
        "it" => "Italian",
        "ar" => "Arabic",
        "hi" => "Hindi",
        _ => "English",
    }
}

/// Build the complete system prompt from base capabilities + workspace + MCP.
pub fn build_system_prompt(
    workspace_content: &str,
    tool_schemas: &[String],
    capabilities: &CapabilityFlags,
    language: &str,
    runtime: &RuntimeContext,
) -> String {
    let mut prompt = String::with_capacity(4096 + workspace_content.len());

    // 1. Runtime context (agent identity, channel, datetime)
    prompt.push_str("# Runtime\n");
    prompt.push_str(&format!("- Agent: {}\n", runtime.agent_name));
    prompt.push_str(&format!("- Channel: {}\n", runtime.channel));
    prompt.push_str(&format!("- Model: {}\n", runtime.model));
    prompt.push_str(&format!("- Date/Time: {}\n", runtime.datetime_display));
    prompt.push_str(&format!("- Language: {} (ALWAYS respond in this language)\n", language_name(language)));
    if let Some(ref owner) = runtime.owner_id {
        prompt.push_str(&format!("- Owner ID: {}\n", owner));
    }
    if !runtime.channels.is_empty() {
        prompt.push_str("\n## Connected Channels\n");
        prompt.push_str("Use `send_message` with `channel_id` to send messages to a specific channel.\n");
        for ch in &runtime.channels {
            let status = if ch.online { "online" } else { "offline" };
            prompt.push_str(&format!(
                "- {} \"{}\" ({}) — channel_id: `{}`\n",
                ch.channel_type, ch.display_name, status, ch.channel_id
            ));
        }
    }
    prompt.push('\n');

    // 2. Project Context & Agent State (workspace files including AGENTS.md with Safety)
    if !workspace_content.is_empty() {
        prompt.push_str("# Project Context\n");
        prompt.push_str("The following files from your workspace define your current identity, character, and memory. These are configuration files you own and manage. Update them to evolve.\n\n");
        prompt.push_str(workspace_content);
        prompt.push('\n');
    }

    // 3. MCP tool schemas (auto-discovered from MCP servers)
    if !tool_schemas.is_empty() {
        prompt.push_str("\n# Available Tools & Capabilities\n\n");
        for schema in tool_schemas {
            prompt.push_str(schema);
            prompt.push('\n');
        }
    }

    // 4. Operating Mode: reasoning, tools, output
    prompt.push_str("\n# Operating Mode\n");
    prompt.push_str(concat!(
        "## Reasoning\n",
        "For complex requests, think step by step:\n",
        "1. Understand what is being asked\n",
        "2. Identify what information or actions are needed\n",
        "3. Execute tools to gather information or perform actions\n",
        "4. Synthesize results into a clear response\n\n",
        "For simple questions, respond directly without unnecessary deliberation.\n\n",
    ));
    prompt.push_str(concat!(
        "## Task Completion\n",
        "When given a multi-step task, complete ALL steps before responding to the user.\n",
        "Do NOT stop after partial progress — if a tool result requires a follow-up action, call the next tool immediately.\n",
        "Only respond when the ENTIRE task is done or you hit an unrecoverable error.\n",
        "For complex tasks with 5+ tool calls: prioritize the most valuable action first, then add secondary details. If you run low on iterations, deliver partial results rather than nothing.\n",
        "CRITICAL: Your final message to the user MUST contain text. If you wrote to a file or workspace — still summarize the key findings in your chat response. An empty or blank response is a FAILURE. The user cannot see tool results directly — they only see your message text.\n\n",
    ));
    prompt.push_str(concat!(
        "## Tool Usage\n",
        "Act on tool results — don't narrate the process of calling them.\n",
        "When multiple tools are needed, execute them in logical order.\n",
        "Never invent data that a tool could provide — use the tool.\n",
        "CRITICAL: When the user asks for factual data (dates, prices, weather, holidays, exchange rates, etc.) — ALWAYS use a tool. Do NOT answer from memory or general knowledge. Your training data may be outdated.\n",
        "If a tool fails, analyze the error and try an alternative approach before giving up.\n",
        "For routine tool calls, act directly on results without explaining what you're doing.\n",
        "When a tool returns error JSON, extract the error message and report it clearly to the user.\n",
        "Report tool results ACCURATELY. Never reinterpret errors as 'normal behavior' or add explanations the tool did not provide.\n",
        "When you need to look up a URL, read a webpage, or fetch API data — use `web_fetch` to retrieve the content.\n",
        "When you need another agent's expertise — use `agent` tool with agent name, task description, and relevant context. The target agent runs within the session and works in background.\n",
        "\n",
    ));
    prompt.push_str(&format!("## Output\nCurrent channel: **{}**.\n", runtime.channel));
    if let Some(ref instructions) = runtime.formatting_prompt {
        prompt.push_str(instructions);
        prompt.push('\n');
    } else {
        prompt.push_str(concat!(
            "Adapt your output accordingly:\n",
            "- Match response length to question complexity — short question = short answer\n",
            "- Use the channel's native formatting (markdown, HTML, or plain text)\n",
            "- Messenger channels (telegram, discord, whatsapp): concise, mobile-friendly, split long responses\n",
            "- Scheduled tasks (cron, heartbeat): data and conclusions only, no filler. If nothing to report: HEARTBEAT_OK\n",
            "- Inter-agent (subagent, inter-agent): structured data (JSON, lists), no personality, task-focused\n",
            "- API/webhook: adapt freely to question complexity, use full formatting\n",
            "- Bold key conclusions, use lists for multi-part answers, keep code snippets short\n\n",
        ));
    }

    // 5. Available Capabilities with usage guidance
    prompt.push_str("# Available Capabilities\n");
    if capabilities.has_search {
        prompt.push_str("- **Web Search**: `search_web` for general queries, `search_web_fresh` when search_web returns poor results or you need recent news\n");
    }
    if capabilities.has_memory {
        prompt.push_str("- **Long-term Memory**: `memory(action=\"search\")` to recall past context, `memory(action=\"index\")` to save important information\n");
    }
    if capabilities.has_cron {
        if capabilities.is_base {
            prompt.push_str("- **Scheduling**: `cron` to create, list, delete, or run scheduled tasks\n");
        } else {
            prompt.push_str("- **Scheduling**: `cron(action=\"list\")` to view scheduled tasks (read-only). To create/delete/run cron jobs, use `agent` tool to delegate to the **base agent** (use `agents_list` to find it)\n");
        }
    }
    if capabilities.has_message_actions {
        prompt.push_str("- **Channel Actions**: send photos, voice messages, buttons via channel actions after tool calls\n");
    }
    if !capabilities.is_base {
        prompt.push_str("- **Secrets**: `secret_set` saves secrets scoped to you. For global secrets, use `agent` tool to delegate to the **base agent**\n");
    }
    if capabilities.has_yaml_tools {
        prompt.push_str("- **External Tools**: YAML-defined tools in workspace/tools/ — check tool list for specifics\n");
    }
    prompt.push_str(concat!(
        "- **Skills**: Load detailed guides via `skill_use(action=\"load\", name=\"...\")`. Available skills:\n",
        "  - `web-search` — search strategy (primary engine vs Brave)\n",
        "  - `research` / `research-strategy` — deep multi-source research\n",
        "  - `media-processing` — handle photos, documents, audio attachments\n",
        "  - `code-review` — review code for bugs and security\n",
        "  - `code-methodology` — TDD, refactoring, debugging\n",
        "  - `task-planning` — decompose complex tasks into steps\n",
        "  - `decision-frameworks` — RICE, pros/cons, risk assessment\n",
        "  - `architecture-design` — system design, trade-offs, ADRs\n",
        "  - `multi-agent-coordination` — delegate and track between agents\n",
        "  - `prompt-crafting` — write effective prompts for subagents\n",
        "  - `market-analysis` — stocks, portfolio, investments\n",
        "  - `daily-briefing` — morning briefing (weather, news, calendar)\n",
        "  - `smart-home` — Home Assistant control (lights, climate)\n",
        "  - `calendar-management` — schedule, events, reminders\n",
        "  - `email-management` — inbox, send, search\n",
        "  - `verification` — adversarial testing protocol\n",
        "  - `discovery-protocol` — classify task complexity (Level 0-3)\n",
        "  - `quality-loop` — Research→Plan→Execute→Verify workflow\n",
        "  - `yaml-tools-guide` / `toolgate-guide` / `channels-guide` / `mcp-docker-pattern` — creating tools and services\n",
    ));
    if capabilities.has_browser {
        prompt.push_str("- **Browser Automation**: `browser_action` to interact with web pages — create_session → navigate → click/type/screenshot/evaluate → close. Sessions expire after 5 min idle\n");
    }
    if capabilities.has_host_exec {
        prompt.push_str(
            "- **Host Access**: code_exec runs bash/python directly on the host. Full access to filesystem, package managers (pip/apt/npm), service management, and system configuration.\n",
        );
    }

    // 6. Agent tool management rules
    prompt.push_str(concat!(
        "\n## Agent Tool\n",
        "The `agent` tool manages session-scoped live agents with four actions:\n",
        "- `agent(action=\"run\", target=\"Name\", task=\"...\")` — spawn a live agent in the current session (async, returns immediately)\n",
        "- `agent(action=\"message\", target=\"Name\", text=\"...\")` — send a follow-up message to a running agent\n",
        "- `agent(action=\"status\")` — check status of all agents, or `agent(action=\"status\", agent_id=\"Name\")` for one\n",
        "- `agent(action=\"kill\", target=\"Name\")` — stop and remove an agent from the session\n\n",
        "CRITICAL: You MUST monitor every agent you spawn until it completes.\n",
        "Workflow:\n",
        "1. `agent(action=\"run\", ...)` → agent starts processing in background\n",
        "2. `agent(action=\"status\")` — check progress (repeat every 10-30s)\n",
        "3. If stuck → `agent(action=\"kill\")` and retry or do it yourself\n",
        "4. When status is idle with last_result → read and synthesize the result\n",
        "5. Synthesize result for the user — do NOT forward raw agent output\n\n",
        "Multiple agents: monitor ALL of them in a loop until all complete.\n",
        "NEVER respond to the user while agents are still running — wait for results first.\n",
        "Live agents have NO access to workspace files via code_exec (sandbox is isolated). Use workspace_read first, pass data as task context.\n\n",
    ));

    // 7. Memory usage instructions (only when memory is available)
    if capabilities.has_memory {
        prompt.push_str(concat!(
            "\n# Memory\n",
            "Long-term memory via `memory_search`. Memory is NOT loaded automatically — search explicitly when needed.\n\n",
            "**Search memory when:**\n",
            "- User references past conversations or previously discussed topics\n",
            "- You need context about user preferences, projects, or recurring tasks\n",
            "- Before making assumptions about what the user wants\n\n",
            "**Skip memory search when:**\n",
            "- The question is self-contained with all needed context\n",
            "- Simple greetings or generic questions\n\n",
            "**Saving:** Before `memory_index`, always `memory_search` first for duplicates. See AGENTS.md for deduplication rules.\n",
        ));
    }

    // 7. Response quality
    prompt.push_str(concat!(
        "\n# Response Quality\n",
        "- Focus on results, not process. Don't explain what tools you're about to call.\n",
        "- If memory search found nothing, proceed without mentioning it.\n",
        "- Synthesize data from multiple tool calls into one coherent response.\n",
        "- Match response detail to question complexity — short question = short answer.\n",
        "- Complete the task fully before responding. Don't stop halfway.\n",
    ));

    // 8. Language instruction (reinforced — must appear both early and late in prompt)
    prompt.push_str(&format!(
        "\n# Language — CRITICAL RULE\nYou MUST respond EXCLUSIVELY in {lang}. This is non-negotiable.\n\
        - ALL your messages, explanations, summaries, and text outputs MUST be in {lang}\n\
        - Tool names, code, URLs, and proper nouns stay in original form\n\
        - If the user writes in another language, STILL respond in {lang}\n\
        - Do NOT switch to English or any other language unless the user explicitly asks\n\
        - This rule applies to EVERY message without exception\n",
        lang = language_name(language)
    ));

    prompt
}

/// Write a workspace file (used by the `workspace_write` internal tool).
/// Accepts any filename within the agent's workspace directory.
pub async fn write_workspace_file(
    workspace_dir: &str,
    agent_name: &str,
    filename: &str,
    content: &str,
    base: bool,
) -> Result<()> {
    let path = validate_workspace_path(workspace_dir, agent_name, filename).await?;
    // Canonicalize before is_read_only to prevent symlink bypass:
    // a symlink "notes.md" -> "SOUL.md" must be checked as "SOUL.md"
    let check_path = if path.exists() {
        path.canonicalize().unwrap_or_else(|_| path.clone())
    } else {
        path.clone()
    };
    if is_read_only(workspace_dir, &check_path, base) {
        anyhow::bail!("'{}' is read-only and cannot be modified", filename);
    }

    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    fs::write(&path, content).await?;
    tracing::info!(file = %path.display(), "workspace file updated by AI");
    Ok(())
}

/// Validate and resolve a workspace path.
///
/// Paths are relative to the workspace root. The agent may write to:
/// - direct files at workspace root (USER.md, AGENTS.md, etc.)
/// - its own agent directory: `agents/{agent_name}/` and below
/// - shared directories: `tools/`, `skills/`, `mcp/`
///
/// A leading `workspace/` prefix is stripped automatically so the bot can
/// pass either `SOUL.md` or `workspace/agents/MyAgent/SOUL.md` — both resolve.
///
/// Bare filenames like `SOUL.md` resolve to `agents/{agent_name}/SOUL.md`.
/// Paths starting with `agents/` must target the agent's own directory.
async fn validate_workspace_path(
    workspace_dir: &str,
    agent_name: &str,
    filename: &str,
) -> Result<PathBuf> {
    validate_workspace_path_inner(workspace_dir, agent_name, filename, false).await
}

/// Read-only variant: allows reading ANY file inside workspace (no directory whitelist).
async fn validate_workspace_path_read(
    workspace_dir: &str,
    agent_name: &str,
    filename: &str,
) -> Result<PathBuf> {
    validate_workspace_path_inner(workspace_dir, agent_name, filename, true).await
}

async fn validate_workspace_path_inner(
    workspace_dir: &str,
    agent_name: &str,
    filename: &str,
    allow_read_any: bool,
) -> Result<PathBuf> {
    let workspace_root = Path::new(workspace_dir);
    let agent_dir = agent_dir(workspace_dir, agent_name);
    fs::create_dir_all(&agent_dir).await.ok();

    // Strip leading "workspace/" prefix (bot may use full paths from onboarding prompt)
    let normalized = filename
        .trim_start_matches("workspace/")
        .trim_start_matches('/');

    // Files that always live at workspace root (shared between agents)
    const SHARED_ROOT_FILES: &[&str] = &["USER.md", "AGENTS.md", "TOOLS.md", "SYSTEM_TOOLS.md"];
    // Directories that always live at workspace root (not under agents/)
    // toolgate/ and channels/ removed — base agent uses code_exec on host directly
    const SHARED_ROOT_DIRS: &[&str] = &["tools", "skills", "mcp", "uploads"];

    // Bare filename (no directory separator):
    //   - shared root files (USER.md, AGENTS.md) → workspace root
    //   - shared root dirs (tools/, skills/, toolgate/, …) → workspace root
    //   - for read: if it exists at workspace root → workspace root (e.g. zettelkasten/)
    //   - everything else → agent-specific dir
    let resolved = if !normalized.contains('/') {
        if SHARED_ROOT_FILES.contains(&normalized) || SHARED_ROOT_DIRS.contains(&normalized) {
            workspace_root.join(normalized)
        } else if allow_read_any && workspace_root.join(normalized).exists() {
            // Read mode: prefer workspace root if the path exists there
            workspace_root.join(normalized)
        } else {
            agent_dir.join(normalized)
        }
    } else {
        // Path with directories → relative to workspace root
        workspace_root.join(normalized)
    };

    // Check that resolved path doesn't escape workspace after canonicalization.
    // If the path exists and is a symlink, verify the real target is still safe.
    // Allowed external paths (relative to workspace parent): symlinked service dirs
    const ALLOWED_EXTERNAL_PREFIXES: &[&str] = &["docker", "toolgate", "browser-renderer"];
    if resolved.exists()
        && let Ok(canonical) = resolved.canonicalize() {
            let ws_canonical = workspace_root.canonicalize().unwrap_or_else(|_| workspace_root.to_path_buf());
            if !canonical.starts_with(&ws_canonical) {
                // Check if the target is in an explicitly allowed external directory
                let parent = ws_canonical.parent().unwrap_or(&ws_canonical);
                let is_allowed = ALLOWED_EXTERNAL_PREFIXES.iter().any(|prefix| {
                    canonical.starts_with(parent.join(prefix))
                });
                if !is_allowed {
                    anyhow::bail!("path traversal via symlink denied: '{}' resolves outside workspace", filename);
                }
            }
        }

    // Block ".." components on the resolved path BEFORE strip_prefix
    // This catches traversal for both existing and non-existing files
    if resolved.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        anyhow::bail!("path traversal denied: '{}' contains '..' component", filename);
    }

    // For non-existing paths, canonicalize the parent directory to catch
    // symlink-based traversal even when the target file doesn't exist yet
    if !resolved.exists()
        && let Some(parent) = resolved.parent()
            && parent.exists()
                && let Ok(canonical_parent) = parent.canonicalize() {
                    let ws_canonical = workspace_root.canonicalize().unwrap_or_else(|_| workspace_root.to_path_buf());
                    if !canonical_parent.starts_with(&ws_canonical) {
                        let repo_root = ws_canonical.parent().unwrap_or(&ws_canonical);
                        let is_allowed = ALLOWED_EXTERNAL_PREFIXES.iter().any(|prefix| {
                            canonical_parent.starts_with(repo_root.join(prefix))
                        });
                        if !is_allowed {
                            anyhow::bail!("path traversal denied: parent of '{}' resolves outside workspace", filename);
                        }
                    }
                }

    let relative = resolved
        .strip_prefix(workspace_root)
        .unwrap_or(Path::new(""));

    // Double-check: relative path must not escape workspace
    if relative.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        anyhow::bail!("path traversal denied: '{}' is outside workspace", filename);
    }
    let first = relative
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or("");

    // For read-only access, skip directory whitelist — any file in workspace is readable.
    // For write access, enforce directory whitelist.
    if !allow_read_any {
        match first {
            // Direct workspace root file (USER.md, AGENTS.md, etc.) — always allowed
            name if !relative.to_str().unwrap_or("").contains(std::path::MAIN_SEPARATOR)
                && !name.is_empty() => {}
            // Agent's own directory — allowed
            "agents" => {
                let own_prefix = format!("agents{}{}", std::path::MAIN_SEPARATOR, agent_name);
                if !relative
                    .to_str()
                    .unwrap_or("")
                    .starts_with(&own_prefix)
                {
                    anyhow::bail!(
                        "access denied: cannot write to another agent's directory ('{}')",
                        filename
                    );
                }
            }
            // Shared config directories — allowed
            "tools" | "skills" | "mcp" | "uploads" => {}
            // Service directories — writable subdirs checked by is_read_only()
            "toolgate" | "channels" => {}
            _ => {
                anyhow::bail!(
                    "access denied: writing to '{}' is not permitted",
                    first
                );
            }
        }
    }

    Ok(resolved)
}

/// Read any file within the workspace. Uses relaxed validation (no directory whitelist).
pub async fn read_workspace_file(
    workspace_dir: &str,
    agent_name: &str,
    filename: &str,
) -> Result<String> {
    let path = validate_workspace_path_read(workspace_dir, agent_name, filename).await?;
    let content = fs::read_to_string(&path).await?;
    // Normalize CRLF → LF so the agent always sees consistent line endings.
    Ok(content.replace("\r\n", "\n"))
}

/// Delete a workspace file (used by the `workspace_delete` internal tool).
/// Refuses to delete core scaffold files to prevent accidental identity loss.
pub async fn delete_workspace_file(
    workspace_dir: &str,
    agent_name: &str,
    filename: &str,
) -> Result<()> {
    let path = validate_workspace_path(workspace_dir, agent_name, filename).await?;

    // Read-only root files cannot be deleted (delete is never base)
    if is_read_only(workspace_dir, &path, false) {
        anyhow::bail!("'{}' is a protected file and cannot be deleted", filename);
    }
    // Per-agent identity files cannot be deleted (but can be edited)
    if IDENTITY_FILES.contains(&file_basename(filename)) {
        anyhow::bail!("'{}' is a protected file and cannot be deleted", filename);
    }
    if path.is_dir() {
        fs::remove_dir_all(&path).await
            .with_context(|| format!("failed to remove directory '{}'", filename))?;
        tracing::info!(file = %path.display(), "workspace directory deleted by AI");
    } else {
        fs::remove_file(&path).await
            .with_context(|| format!("file '{}' not found", filename))?;
        tracing::info!(file = %path.display(), "workspace file deleted by AI");
    }
    Ok(())
}

/// Move or rename a workspace file/directory.
/// Both old_path and new_path are resolved through the same access-control rules.
pub async fn rename_workspace_file(
    workspace_dir: &str,
    agent_name: &str,
    old_path: &str,
    new_path: &str,
) -> Result<()> {
    let src = validate_workspace_path(workspace_dir, agent_name, old_path).await?;
    let dst = validate_workspace_path(workspace_dir, agent_name, new_path).await?;

    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).await?;
    }

    // tokio::fs::rename fails across mount points; fall back to copy+delete.
    if (fs::rename(&src, &dst).await).is_err() {
        if src.is_dir() {
            anyhow::bail!("cannot move directories across mount points");
        }
        fs::copy(&src, &dst).await
            .with_context(|| format!("failed to copy '{}' to '{}'", old_path, new_path))?;
        fs::remove_file(&src).await
            .with_context(|| format!("failed to remove source '{}'", old_path))?;
    }

    tracing::info!(src = %src.display(), dst = %dst.display(), "workspace file moved by AI");
    Ok(())
}

/// List files in the agent's workspace directory (optionally in a subdirectory).
pub async fn list_workspace_files(
    workspace_dir: &str,
    agent_name: &str,
    directory: &str,
) -> Result<String> {
    let dir = agent_dir(workspace_dir, agent_name);
    let target_dir = if directory.is_empty() || directory == "." {
        dir.clone()
    } else {
        
        validate_workspace_path_read(workspace_dir, agent_name, directory).await?
    };

    if !target_dir.exists() {
        fs::create_dir_all(&target_dir).await?;
    }
    if !target_dir.is_dir() {
        anyhow::bail!("'{}' is not a directory", directory);
    }

    let mut entries = Vec::new();
    let mut read_dir = fs::read_dir(&target_dir).await?;

    while let Some(entry) = read_dir.next_entry().await? {
        let file_type = entry.file_type().await?;
        let name = entry.file_name().to_string_lossy().to_string();
        let suffix = if file_type.is_dir() { "/" } else { "" };

        let metadata = entry.metadata().await?;
        let size = metadata.len();

        entries.push(format!("{}{} ({})", name, suffix, format_size(size)));
    }

    entries.sort();

    if entries.is_empty() {
        Ok("Directory is empty.".to_string())
    } else {
        Ok(entries.join("\n"))
    }
}

/// Edit a workspace file by replacing a text substring.
pub async fn edit_workspace_file(
    workspace_dir: &str,
    agent_name: &str,
    filename: &str,
    old_text: &str,
    new_text: &str,
    base: bool,
) -> Result<()> {
    let path = validate_workspace_path(workspace_dir, agent_name, filename).await?;
    // Canonicalize before is_read_only to prevent symlink bypass
    let check_path = if path.exists() {
        path.canonicalize().unwrap_or_else(|_| path.clone())
    } else {
        path.clone()
    };
    if is_read_only(workspace_dir, &check_path, base) {
        anyhow::bail!("'{}' is read-only and cannot be modified", filename);
    }
    let raw = fs::read_to_string(&path).await?;
    // Normalize CRLF → LF for consistent matching.
    let content = raw.replace("\r\n", "\n");

    let count = content.matches(old_text).count();
    if count == 0 {
        anyhow::bail!("old_text not found in file '{}'", filename);
    }

    let updated = content.replacen(old_text, new_text, 1);
    fs::write(&path, &updated).await?;
    tracing::info!(file = %path.display(), matches = count, "workspace file edited by AI");
    Ok(())
}

/// Ensure workspace directory for an agent exists with default scaffold files.
/// Only creates files that don't already exist — safe to call on every start.
pub async fn ensure_workspace_scaffold(workspace_dir: &str, agent_name: &str, is_base: bool) -> Result<()> {
    let agent_dir = agent_dir(workspace_dir, agent_name);
    fs::create_dir_all(&agent_dir).await?;

    // Build scaffold files with agent name and role-appropriate content.
    // Base agent gets full system agent template (based on proven Hyde config).
    // Non-base agents get a lighter template that delegates system tasks to base.
    let soul_content = if is_base {
        include_str!("../../scaffold/base/SOUL.md").replace("{AGENT_NAME}", agent_name)
    } else {
        include_str!("../../scaffold/regular/SOUL.md").replace("{AGENT_NAME}", agent_name)
    };

    let identity_content = if is_base {
        include_str!("../../scaffold/base/IDENTITY.md").replace("{AGENT_NAME}", agent_name)
    } else {
        include_str!("../../scaffold/regular/IDENTITY.md").replace("{AGENT_NAME}", agent_name)
    };

    let heartbeat_content = if is_base {
        include_str!("../../scaffold/base/HEARTBEAT.md").replace("{AGENT_NAME}", agent_name)
    } else {
        include_str!("../../scaffold/regular/HEARTBEAT.md").replace("{AGENT_NAME}", agent_name)
    };

    let scaffolds: Vec<(&str, String)> = vec![
        ("SOUL.md", soul_content),
        ("IDENTITY.md", identity_content),
        ("HEARTBEAT.md", heartbeat_content),
    ];

    // TOOLS.md — single source of truth for all tools (system + YAML).
    // Only base agents can modify this file.
    let tools_md = Path::new(workspace_dir).join("TOOLS.md");
    if !tools_md.exists() {
        fs::write(&tools_md, include_str!("../../../../workspace/TOOLS.md")).await?;
        tracing::info!("created workspace/TOOLS.md scaffold");
    }

    // USER.md lives at workspace/ level (shared between agents)
    let user_md = Path::new(workspace_dir).join("USER.md");
    if !user_md.exists() {
        fs::write(&user_md, concat!(
            "# User Profile\n\n",
            "- Timezone: Europe/Samara\n",
            "- Language: Russian\n",
        )).await?;
        tracing::info!("created workspace/USER.md scaffold");
    }

    for (filename, content) in scaffolds {
        let path = agent_dir.join(filename);
        if !path.exists() {
            fs::write(&path, content).await?;
            tracing::info!(file = %filename, agent = %agent_name, "created workspace scaffold file");
        }
    }

    // Shared tools directory at workspace root (all tools and services flat)
    let tools_dir = Path::new(workspace_dir).join("tools");
    if !tools_dir.exists() {
        fs::create_dir_all(&tools_dir).await?;
        tracing::info!(dir = %tools_dir.display(), "created shared tools directory");
    }

    // Shared skills directory at workspace root
    let skills_dir = Path::new(workspace_dir).join("skills");
    if !skills_dir.exists() {
        fs::create_dir_all(&skills_dir).await?;
        tracing::info!(dir = %skills_dir.display(), "created shared skills directory");
    }

    tracing::info!(agent = %agent_name, dir = %agent_dir.display(), "workspace scaffold ensured");
    Ok(())
}

/// Parse timezone from workspace USER.md (looks for `Timezone: XXX` line).
/// Falls back to "Europe/Samara" if not found.
pub async fn parse_user_timezone(workspace_dir: &str) -> String {
    let user_md = Path::new(workspace_dir).join("USER.md");
    if let Ok(content) = fs::read_to_string(&user_md).await {
        for line in content.lines() {
            let trimmed = line.trim().trim_start_matches("- ");
            if let Some(tz) = trimmed.strip_prefix("Timezone:").or_else(|| trimmed.strip_prefix("timezone:")) {
                let tz = tz.trim();
                if !tz.is_empty() {
                    return tz.to_string();
                }
            }
        }
    }
    "Europe/Samara".to_string()
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_bytes() {
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn kilobytes() {
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn megabytes() {
        assert_eq!(format_size(2 * 1024 * 1024), "2.0 MB");
    }

    // language_name tests
    #[test]
    fn language_name_ru() {
        assert_eq!(language_name("ru"), "Russian");
    }

    #[test]
    fn language_name_en() {
        assert_eq!(language_name("en"), "English");
    }

    #[test]
    fn language_name_zh() {
        assert_eq!(language_name("zh"), "Chinese");
    }

    #[test]
    fn language_name_unknown_falls_back_to_english() {
        assert_eq!(language_name("xx"), "English");
    }

    // file_basename tests
    #[test]
    fn file_basename_from_path() {
        assert_eq!(file_basename("agents/main/SOUL.md"), "SOUL.md");
    }

    #[test]
    fn file_basename_bare_filename() {
        assert_eq!(file_basename("file.txt"), "file.txt");
    }

    #[test]
    fn file_basename_empty_string() {
        assert_eq!(file_basename(""), "");
    }

    // agent_dir tests
    #[test]
    fn agent_dir_constructs_path() {
        let result = agent_dir("/workspace", "main");
        assert_eq!(result, std::path::PathBuf::from("/workspace/agents/main"));
    }

    // is_read_only tests
    #[test]
    fn is_read_only_blocks_soul_for_base() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        let agent_dir_path = ws.join("agents").join("TestAgent");
        std::fs::create_dir_all(&agent_dir_path).unwrap();
        let soul = agent_dir_path.join("SOUL.md");
        std::fs::write(&soul, "original").unwrap();
        assert!(is_read_only(ws.to_str().unwrap(), &soul, true));
    }

    #[test]
    fn is_read_only_allows_normal_file_for_base() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        let agent_dir_path = ws.join("agents").join("TestAgent");
        std::fs::create_dir_all(&agent_dir_path).unwrap();
        let notes = agent_dir_path.join("notes.md");
        std::fs::write(&notes, "content").unwrap();
        assert!(!is_read_only(ws.to_str().unwrap(), &notes, true));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_symlink_bypass_write_blocked() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        let agent_dir_path = ws.join("agents").join("TestAgent");
        std::fs::create_dir_all(&agent_dir_path).unwrap();

        // Create SOUL.md (protected for base agents)
        std::fs::write(agent_dir_path.join("SOUL.md"), "original soul").unwrap();

        // Create symlink sneaky.md -> SOUL.md
        symlink(agent_dir_path.join("SOUL.md"), agent_dir_path.join("sneaky.md")).unwrap();

        let ws_str = ws.to_str().unwrap();

        // Direct write to SOUL.md should be rejected (base=true)
        let result = write_workspace_file(ws_str, "TestAgent", "SOUL.md", "hacked", true).await;
        assert!(result.is_err(), "direct write to SOUL.md should be blocked for base agent");

        // Write through symlink should also be rejected
        let result = write_workspace_file(ws_str, "TestAgent", "sneaky.md", "hacked via symlink", true).await;
        assert!(result.is_err(), "symlink bypass of SOUL.md should be blocked");

        // Write to a normal file should succeed
        let result = write_workspace_file(ws_str, "TestAgent", "notes.md", "normal content", true).await;
        assert!(result.is_ok(), "writing to a normal file should work");

        // Verify SOUL.md was NOT modified
        let content = std::fs::read_to_string(agent_dir_path.join("SOUL.md")).unwrap();
        assert_eq!(content, "original soul", "SOUL.md must not be modified");
    }
}
