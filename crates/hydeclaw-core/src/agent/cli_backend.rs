//! Unified CLI LLM backend — configurable execution of CLI tools
//! (claude, gemini, etc.) with session management, timeouts, and serialization.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::containers::sandbox::{CodeSandbox, ExecResult};

// ── Configuration ────────────────────────────────────────────────────────────

/// Configuration for a CLI LLM backend (e.g. Claude CLI, Gemini CLI).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CliBackendConfig {
    /// CLI executable path/name
    pub command: String,
    /// Base args for fresh invocation
    #[serde(default)]
    pub args: Vec<String>,
    /// Args for resuming a session (template: {session_id} is replaced)
    #[serde(default)]
    pub resume_args: Vec<String>,
    /// Output format
    #[serde(default)]
    pub output: CliOutputFormat,
    /// How to pass user prompt
    #[serde(default)]
    pub input: CliInputMode,
    /// Flag for model selection (e.g. "--model")
    #[serde(default)]
    pub model_arg: Option<String>,
    /// Model name aliases
    #[serde(default)]
    pub model_aliases: HashMap<String, String>,
    /// Flag for session ID
    #[serde(default)]
    pub session_arg: Option<String>,
    /// Session mode
    #[serde(default)]
    pub session_mode: CliSessionMode,
    /// Flag for system prompt injection
    #[serde(default)]
    pub system_prompt_arg: Option<String>,
    /// Overall timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Serialize concurrent runs (queue, don't parallelize)
    #[serde(default = "default_true")]
    pub serialize: bool,
    /// Extra environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
}

fn default_timeout() -> u64 {
    300
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CliOutputFormat {
    #[default]
    Json,
    Text,
    Jsonl,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CliInputMode {
    #[default]
    Arg,
    Stdin,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CliSessionMode {
    #[default]
    Always,
    Existing,
    None,
}

// ── Default configs ──────────────────────────────────────────────────────────

pub fn default_claude_backend() -> CliBackendConfig {
    CliBackendConfig {
        command: "claude".into(),
        args: vec![
            "-p".into(),
            "--output-format".into(),
            "json".into(),
            "--permission-mode".into(),
            "bypassPermissions".into(),
        ],
        resume_args: vec![
            "-p".into(),
            "--output-format".into(),
            "json".into(),
            "--permission-mode".into(),
            "bypassPermissions".into(),
            "--resume".into(),
            "{session_id}".into(),
        ],
        output: CliOutputFormat::Json,
        input: CliInputMode::Arg,
        model_arg: Some("--model".into()),
        model_aliases: [("opus", "opus"), ("sonnet", "sonnet"), ("haiku", "haiku")]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        session_arg: Some("--session-id".into()),
        session_mode: CliSessionMode::Always,
        system_prompt_arg: Some("--append-system-prompt".into()),
        timeout_secs: 300,
        serialize: true,
        env: HashMap::new(),
    }
}

pub fn default_gemini_backend() -> CliBackendConfig {
    CliBackendConfig {
        command: "gemini".into(),
        args: vec!["-p".into(), "--output-format".into(), "json".into()],
        resume_args: vec![],
        output: CliOutputFormat::Json,
        input: CliInputMode::Arg,
        model_arg: Some("--model".into()),
        model_aliases: HashMap::new(),
        session_arg: None,
        session_mode: CliSessionMode::None,
        system_prompt_arg: None,
        timeout_secs: 300,
        serialize: true,
        env: HashMap::new(),
    }
}

// ── CliOutput ────────────────────────────────────────────────────────────────

/// Parsed CLI output.
pub struct CliOutput {
    pub text: String,
    pub session_id: Option<String>,
    pub usage: Option<hydeclaw_types::TokenUsage>,
}

// ── Error Classification ─────────────────────────────────────────────────────

/// Classified CLI error reason for cooldown decisions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CliErrorReason {
    /// Rate limited (429, "too many requests", quota exceeded)
    RateLimit,
    /// Auth error (401/403, invalid key, revoked, banned)
    Auth,
    /// Billing issue (402, insufficient credits)
    Billing,
    /// Overloaded (503, "high demand")
    Overloaded,
    /// Timeout (process took too long)
    Timeout,
    /// Other/unknown error
    Unknown,
}

impl CliErrorReason {
    /// Cooldown duration for this error type (exponential: base * 5^(n-1), capped).
    fn cooldown_ms(&self, error_count: u32) -> u64 {
        let n = error_count.min(4);
        match self {
            // Rate limit / overload: 1m → 5m → 25m → 1h max
            CliErrorReason::RateLimit | CliErrorReason::Overloaded => {
                let ms = 60_000u64 * 5u64.pow(n.saturating_sub(1));
                ms.min(3_600_000) // 1 hour max
            }
            // Auth / billing: 5h → 10h → 20h → 24h max
            CliErrorReason::Auth | CliErrorReason::Billing => {
                let ms = 5 * 3_600_000u64 * 2u64.pow(n.saturating_sub(1));
                ms.min(24 * 3_600_000) // 24 hours max
            }
            // Timeout / unknown: 30s → 2m → 10m → 30m max
            CliErrorReason::Timeout | CliErrorReason::Unknown => {
                let ms = 30_000u64 * 5u64.pow(n.saturating_sub(1).min(3));
                ms.min(30 * 60_000) // 30 min max
            }
        }
    }
}

/// Classify an error from CLI output using shared error_classify module.
fn classify_cli_error(stderr: &str, stdout: &str, _exit_code: i64) -> CliErrorReason {
    use crate::agent::error_classify::{classify_str, LlmErrorClass};
    let combined = format!("{} {}", stderr, stdout);
    match classify_str(&combined) {
        LlmErrorClass::RateLimit => CliErrorReason::RateLimit,
        LlmErrorClass::AuthPermanent => CliErrorReason::Auth,
        LlmErrorClass::Billing => CliErrorReason::Billing,
        LlmErrorClass::Overloaded | LlmErrorClass::TransientHttp => CliErrorReason::Overloaded,
        _ => CliErrorReason::Unknown,
    }
}

// ── Cooldown Tracker ─────────────────────────────────────────────────────────

struct CooldownState {
    /// Number of consecutive errors
    error_count: u32,
    /// Cooldown expires at this instant
    cooldown_until: Option<Instant>,
    /// Last error reason
    last_reason: Option<CliErrorReason>,
}

impl CooldownState {
    fn new() -> Self {
        Self { error_count: 0, cooldown_until: None, last_reason: None }
    }

    /// Check if currently in cooldown. If expired, reset state.
    fn is_in_cooldown(&mut self) -> Option<Duration> {
        if let Some(until) = self.cooldown_until {
            let now = Instant::now();
            if now < until {
                return Some(until - now);
            }
            // Expired — reset (circuit breaker half-open → closed)
            self.error_count = 0;
            self.cooldown_until = None;
            self.last_reason = None;
        }
        None
    }

    /// Record a failure and start cooldown.
    fn record_failure(&mut self, reason: CliErrorReason) {
        // Don't extend active cooldown window (OpenClaw pattern)
        if self.cooldown_until.is_some_and(|u| Instant::now() < u) {
            return;
        }
        self.error_count += 1;
        self.last_reason = Some(reason);
        let cooldown_ms = reason.cooldown_ms(self.error_count);
        self.cooldown_until = Some(Instant::now() + Duration::from_millis(cooldown_ms));
        tracing::warn!(
            reason = ?reason,
            error_count = self.error_count,
            cooldown_secs = cooldown_ms / 1000,
            "CLI provider entering cooldown"
        );
    }

    /// Record success — reset error count.
    fn record_success(&mut self) {
        self.error_count = 0;
        self.cooldown_until = None;
        self.last_reason = None;
    }
}

// ── CliRunner ────────────────────────────────────────────────────────────────

/// Manages CLI execution with sessions, timeouts, serialization, and cooldown.
pub struct CliRunner {
    config: CliBackendConfig,
    sessions: RwLock<HashMap<String, String>>,
    semaphore: Semaphore,
    cooldown: Mutex<CooldownState>,
}

impl CliRunner {
    pub fn new(config: CliBackendConfig) -> Self {
        let permits = if config.serialize { 1 } else { 64 };
        Self {
            config,
            sessions: RwLock::new(HashMap::new()),
            semaphore: Semaphore::new(permits),
            cooldown: Mutex::new(CooldownState::new()),
        }
    }

    /// Execute CLI with prompt, returning parsed response.
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        agent_name: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        model: &str,
        sandbox: Option<&CodeSandbox>,
        workspace_dir: &str,
        base: bool,
    ) -> Result<CliOutput> {
        // Check cooldown before acquiring permit
        {
            let mut cd = self.cooldown.lock().await;
            if let Some(remaining) = cd.is_in_cooldown() {
                anyhow::bail!(
                    "CLI provider in cooldown ({:?} remaining, reason: {:?}, {} consecutive errors)",
                    remaining, cd.last_reason, cd.error_count
                );
            }
        }

        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("CLI semaphore closed"))?;

        // Resolve model alias
        let resolved_model = self
            .config
            .model_aliases
            .get(model)
            .map(|s| s.as_str())
            .unwrap_or(model);

        // Check for existing session
        let existing_session = self.sessions.read().await.get(agent_name).cloned();
        let use_resume = existing_session.is_some() && !self.config.resume_args.is_empty();

        // Build argv
        let argv = self.build_argv(
            resolved_model,
            prompt,
            system_prompt,
            existing_session.as_deref(),
            use_resume,
        );

        let timeout = Duration::from_secs(self.config.timeout_secs);

        // Execute
        let start = std::time::Instant::now();
        let exec_result = if base && sandbox.is_none() {
            execute_on_host(&argv, &self.config.env, workspace_dir, timeout).await
        } else if let Some(sb) = sandbox {
            let cmd = argv.iter().map(|a| shell_escape(a)).collect::<Vec<_>>().join(" ");
            let host_path = std::fs::canonicalize(workspace_dir)
                .unwrap_or_default().to_string_lossy().to_string();
            sb.execute(agent_name, &cmd, "bash", &[], &host_path, base).await
        } else {
            anyhow::bail!("CLI provider requires either base host access or Docker sandbox")
        };

        let exec_result = match exec_result {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                let reason = if msg.contains("timed out") || msg.contains("timeout") {
                    CliErrorReason::Timeout
                } else {
                    CliErrorReason::Unknown
                };
                self.cooldown.lock().await.record_failure(reason);
                return Err(e);
            }
        };

        let elapsed = start.elapsed();

        if exec_result.exit_code != 0 {
            let reason = classify_cli_error(&exec_result.stderr, &exec_result.stdout, exec_result.exit_code);
            self.cooldown.lock().await.record_failure(reason);
            anyhow::bail!(
                "CLI exited with code {} ({:?}): {}",
                exec_result.exit_code,
                reason,
                exec_result.stderr.chars().take(500).collect::<String>()
            );
        }

        // Success — reset cooldown
        self.cooldown.lock().await.record_success();

        // Parse output
        let output = match self.config.output {
            CliOutputFormat::Json => parse_cli_json(&exec_result.stdout),
            CliOutputFormat::Jsonl => parse_cli_jsonl(&exec_result.stdout),
            CliOutputFormat::Text => CliOutput {
                text: exec_result.stdout.trim().to_string(),
                session_id: None,
                usage: None,
            },
        };

        // Store session for next call
        if let Some(ref sid) = output.session_id {
            self.sessions
                .write()
                .await
                .insert(agent_name.to_string(), sid.clone());
        }

        tracing::info!(
            command = %self.config.command,
            model = %resolved_model,
            content_len = output.text.len(),
            elapsed_ms = elapsed.as_millis() as u64,
            session_id = ?output.session_id,
            "CLI response"
        );

        Ok(output)
    }

    fn build_argv(
        &self,
        model: &str,
        prompt: &str,
        system_prompt: Option<&str>,
        session_id: Option<&str>,
        use_resume: bool,
    ) -> Vec<String> {
        let mut argv = vec![self.config.command.clone()];

        if use_resume {
            // Use resume args, replace {session_id} template
            let sid = session_id.unwrap_or("");
            for arg in &self.config.resume_args {
                argv.push(arg.replace("{session_id}", sid));
            }
        } else {
            // Fresh invocation
            argv.extend(self.config.args.clone());

            // --model
            if let Some(ref model_arg) = self.config.model_arg
                && !model.is_empty() {
                    argv.push(model_arg.clone());
                    argv.push(model.to_string());
                }

            // --append-system-prompt
            if let Some(ref sp_arg) = self.config.system_prompt_arg
                && let Some(sp) = system_prompt
                    && !sp.is_empty() {
                        argv.push(sp_arg.clone());
                        argv.push(sp.to_string());
                    }

            // --session-id
            if self.config.session_mode != CliSessionMode::None
                && let Some(ref s_arg) = self.config.session_arg {
                    let sid = session_id.unwrap_or("");
                    if !sid.is_empty() {
                        argv.push(s_arg.clone());
                        argv.push(sid.to_string());
                    }
                }
        }

        // Add prompt
        if self.config.input == CliInputMode::Arg {
            argv.push(prompt.to_string());
        }

        argv
    }
}

// ── Host execution ───────────────────────────────────────────────────────────

async fn execute_on_host(
    argv: &[String],
    env: &HashMap<String, String>,
    workspace_dir: &str,
    timeout: Duration,
) -> Result<ExecResult> {
    use tokio::process::Command;

    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(workspace_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }

    let child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn CLI '{}': {}", argv[0], e))?;

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(ExecResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1) as i64,
        }),
        Ok(Err(e)) => anyhow::bail!("CLI process error: {}", e),
        Err(_) => anyhow::bail!("CLI timed out after {}s", timeout.as_secs()),
    }
}

// ── Output parsing ───────────────────────────────────────────────────────────

fn parse_cli_json(raw: &str) -> CliOutput {
    #[derive(Deserialize)]
    struct JsonOut {
        #[serde(alias = "result", alias = "response", alias = "content")]
        text: Option<String>,
        #[serde(alias = "session_id", alias = "sessionId", alias = "conversation_id")]
        session_id: Option<String>,
        #[serde(default)]
        cost_usd: Option<f64>,
        #[serde(default)]
        input_tokens: Option<u32>,
        #[serde(default)]
        output_tokens: Option<u32>,
        #[serde(default)]
        usage: Option<serde_json::Value>,
    }

    let parsed: Option<JsonOut> = serde_json::from_str(raw.trim()).ok();
    match parsed {
        Some(p) => {
            if let Some(cost) = p.cost_usd {
                tracing::info!(cost_usd = cost, "CLI cost");
            }
            let usage = match (p.input_tokens, p.output_tokens) {
                (Some(inp), Some(out)) => Some(hydeclaw_types::TokenUsage {
                    input_tokens: inp,
                    output_tokens: out,
                }),
                _ => {
                    // Try nested usage object
                    p.usage.as_ref().and_then(|u| {
                        let inp =
                            u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let out =
                            u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        if inp > 0 || out > 0 {
                            Some(hydeclaw_types::TokenUsage {
                                input_tokens: inp,
                                output_tokens: out,
                            })
                        } else {
                            None
                        }
                    })
                }
            };
            CliOutput {
                text: p.text.unwrap_or_default(),
                session_id: p.session_id,
                usage,
            }
        }
        None => CliOutput {
            text: raw.trim().to_string(),
            session_id: None,
            usage: None,
        },
    }
}

fn parse_cli_jsonl(raw: &str) -> CliOutput {
    let mut texts = Vec::new();
    let mut session_id = None;
    let mut usage = None;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            // Extract session_id
            if session_id.is_none() {
                session_id = v
                    .get("session_id")
                    .or_else(|| v.get("thread_id"))
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }
            // Extract text
            if let Some(text) = v
                .get("text")
                .or_else(|| v.get("result"))
                .and_then(|t| t.as_str())
            {
                texts.push(text.to_string());
            }
            if let Some(item) = v.get("item")
                && let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    texts.push(text.to_string());
                }
            // Extract usage
            if let Some(u) = v.get("usage") {
                let inp =
                    u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let out =
                    u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                if inp > 0 || out > 0 {
                    usage = Some(hydeclaw_types::TokenUsage {
                        input_tokens: inp,
                        output_tokens: out,
                    });
                }
            }
        }
    }

    CliOutput {
        text: texts.join("\n"),
        session_id,
        usage,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Format messages for CLI prompt. Returns (user_prompt, system_prompt).
pub fn format_messages_for_cli(
    messages: &[hydeclaw_types::Message],
) -> (String, Option<String>) {
    use hydeclaw_types::MessageRole;
    let mut system_parts = Vec::new();
    let mut prompt_parts = Vec::new();
    for msg in messages {
        match msg.role {
            MessageRole::System => system_parts.push(msg.content.clone()),
            MessageRole::User => prompt_parts.push(msg.content.clone()),
            MessageRole::Assistant => {
                prompt_parts.push(format!("[Assistant]\n{}", msg.content));
            }
            MessageRole::Tool => {
                prompt_parts.push(format!("[Tool result]\n{}", msg.content));
            }
        }
    }
    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };
    (prompt_parts.join("\n\n"), system)
}

/// Simple shell escaping — wraps in single quotes, escaping inner single quotes.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hydeclaw_types::{Message, MessageRole};

    // ── parse_cli_json ──────────────────────────────────────────────────────

    #[test]
    fn parse_json_valid_result() {
        let json = r#"{"result": "Hello", "session_id": "abc-123", "input_tokens": 10, "output_tokens": 20}"#;
        let out = parse_cli_json(json);
        assert_eq!(out.text, "Hello");
        assert_eq!(out.session_id, Some("abc-123".to_string()));
        let u = out.usage.unwrap();
        assert_eq!(u.input_tokens, 10);
        assert_eq!(u.output_tokens, 20);
    }

    #[test]
    fn parse_json_response_alias() {
        let json = r#"{"response": "World", "sessionId": "s-42"}"#;
        let out = parse_cli_json(json);
        assert_eq!(out.text, "World");
        assert_eq!(out.session_id, Some("s-42".to_string()));
        assert!(out.usage.is_none());
    }

    #[test]
    fn parse_json_content_alias() {
        let json = r#"{"content": "Hi", "conversation_id": "c-1"}"#;
        let out = parse_cli_json(json);
        assert_eq!(out.text, "Hi");
        assert_eq!(out.session_id, Some("c-1".to_string()));
    }

    #[test]
    fn parse_json_invalid_returns_raw() {
        let raw = "Not a JSON at all";
        let out = parse_cli_json(raw);
        assert_eq!(out.text, "Not a JSON at all");
        assert!(out.session_id.is_none());
        assert!(out.usage.is_none());
    }

    #[test]
    fn parse_json_empty_string() {
        let out = parse_cli_json("");
        assert_eq!(out.text, "");
        assert!(out.session_id.is_none());
        assert!(out.usage.is_none());
    }

    #[test]
    fn parse_json_nested_usage() {
        let json = r#"{"result": "ok", "usage": {"input_tokens": 100, "output_tokens": 50}}"#;
        let out = parse_cli_json(json);
        assert_eq!(out.text, "ok");
        let u = out.usage.unwrap();
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
    }

    #[test]
    fn parse_json_cost_usd_present() {
        // cost_usd is logged but not returned in CliOutput; just verify no panic
        let json = r#"{"result": "done", "cost_usd": 0.003, "input_tokens": 5, "output_tokens": 3}"#;
        let out = parse_cli_json(json);
        assert_eq!(out.text, "done");
        let u = out.usage.unwrap();
        assert_eq!(u.input_tokens, 5);
        assert_eq!(u.output_tokens, 3);
    }

    #[test]
    fn parse_json_no_text_field() {
        // JSON is valid but has no recognized text field -> empty string
        let json = r#"{"session_id": "s-99", "input_tokens": 1, "output_tokens": 2}"#;
        let out = parse_cli_json(json);
        assert_eq!(out.text, "");
        assert_eq!(out.session_id, Some("s-99".to_string()));
    }

    // ── parse_cli_jsonl ─────────────────────────────────────────────────────

    #[test]
    fn parse_jsonl_multiple_lines() {
        let raw = r#"{"text": "Hello", "session_id": "s-1"}
{"text": " world"}
{"usage": {"input_tokens": 10, "output_tokens": 20}}"#;
        let out = parse_cli_jsonl(raw);
        assert_eq!(out.text, "Hello\n world");
        assert_eq!(out.session_id, Some("s-1".to_string()));
        let u = out.usage.unwrap();
        assert_eq!(u.input_tokens, 10);
        assert_eq!(u.output_tokens, 20);
    }

    #[test]
    fn parse_jsonl_item_text() {
        let raw = r#"{"item": {"text": "nested content"}, "thread_id": "t-5"}"#;
        let out = parse_cli_jsonl(raw);
        assert_eq!(out.text, "nested content");
        assert_eq!(out.session_id, Some("t-5".to_string()));
    }

    #[test]
    fn parse_jsonl_empty() {
        let out = parse_cli_jsonl("");
        assert_eq!(out.text, "");
        assert!(out.session_id.is_none());
        assert!(out.usage.is_none());
    }

    // ── format_messages_for_cli ─────────────────────────────────────────────

    fn msg(role: MessageRole, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        }
    }

    #[test]
    fn format_system_and_user() {
        let msgs = vec![
            msg(MessageRole::System, "Be helpful"),
            msg(MessageRole::User, "Hi there"),
        ];
        let (prompt, system) = format_messages_for_cli(&msgs);
        assert_eq!(prompt, "Hi there");
        assert_eq!(system, Some("Be helpful".to_string()));
    }

    #[test]
    fn format_user_only() {
        let msgs = vec![msg(MessageRole::User, "Hello")];
        let (prompt, system) = format_messages_for_cli(&msgs);
        assert_eq!(prompt, "Hello");
        assert!(system.is_none());
    }

    #[test]
    fn format_with_assistant_and_tool() {
        let msgs = vec![
            msg(MessageRole::User, "Question"),
            msg(MessageRole::Assistant, "Let me check"),
            msg(MessageRole::Tool, "result=42"),
            msg(MessageRole::User, "Thanks"),
        ];
        let (prompt, system) = format_messages_for_cli(&msgs);
        assert!(prompt.contains("Question"));
        assert!(prompt.contains("[Assistant]\nLet me check"));
        assert!(prompt.contains("[Tool result]\nresult=42"));
        assert!(prompt.contains("Thanks"));
        assert!(system.is_none());
    }

    #[test]
    fn format_empty_messages() {
        let msgs: Vec<Message> = vec![];
        let (prompt, system) = format_messages_for_cli(&msgs);
        assert_eq!(prompt, "");
        assert!(system.is_none());
    }

    // ── shell_escape ────────────────────────────────────────────────────────

    #[test]
    fn shell_escape_simple() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn shell_escape_with_single_quote() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_escape_empty() {
        assert_eq!(shell_escape(""), "''");
    }

    // ── default configs ─────────────────────────────────────────────────────

    #[test]
    fn default_claude_config() {
        let cfg = default_claude_backend();
        assert_eq!(cfg.command, "claude");
        assert!(cfg.serialize);
        assert_eq!(cfg.session_mode, CliSessionMode::Always);
        assert_eq!(cfg.model_arg, Some("--model".to_string()));
        assert_eq!(cfg.system_prompt_arg, Some("--append-system-prompt".to_string()));
        assert_eq!(cfg.timeout_secs, 300);
    }

    #[test]
    fn default_gemini_config() {
        let cfg = default_gemini_backend();
        assert_eq!(cfg.command, "gemini");
        assert_eq!(cfg.session_mode, CliSessionMode::None);
        assert!(cfg.resume_args.is_empty());
        assert!(cfg.system_prompt_arg.is_none());
        assert!(cfg.serialize);
    }

    // ── classify_cli_error ──────────────────────────────────────────────────

    #[test]
    fn classify_rate_limit() {
        let reason = classify_cli_error("429 too many requests", "", 1);
        assert_eq!(reason, CliErrorReason::RateLimit);
    }

    #[test]
    fn classify_auth() {
        let reason = classify_cli_error("401 unauthorized: invalid api key", "", 1);
        assert_eq!(reason, CliErrorReason::Auth);
    }

    #[test]
    fn classify_billing() {
        let reason = classify_cli_error("402 payment required", "", 1);
        assert_eq!(reason, CliErrorReason::Billing);
    }

    #[test]
    fn classify_overloaded() {
        let reason = classify_cli_error("overloaded_error: server at capacity", "", 1);
        assert_eq!(reason, CliErrorReason::Overloaded);
    }

    #[test]
    fn classify_unknown() {
        let reason = classify_cli_error("something weird", "", 1);
        assert_eq!(reason, CliErrorReason::Unknown);
    }

    // ── CooldownState ───────────────────────────────────────────────────────

    #[test]
    fn cooldown_new_not_in_cooldown() {
        let mut state = CooldownState::new();
        assert!(state.is_in_cooldown().is_none());
        assert_eq!(state.error_count, 0);
    }

    #[test]
    fn cooldown_after_failure() {
        let mut state = CooldownState::new();
        state.record_failure(CliErrorReason::RateLimit);
        assert_eq!(state.error_count, 1);
        assert!(state.cooldown_until.is_some());
        assert!(state.is_in_cooldown().is_some());
    }

    #[test]
    fn cooldown_after_success_reset() {
        let mut state = CooldownState::new();
        state.record_failure(CliErrorReason::Unknown);
        assert_eq!(state.error_count, 1);
        state.record_success();
        assert_eq!(state.error_count, 0);
        assert!(state.cooldown_until.is_none());
        assert!(state.last_reason.is_none());
        assert!(state.is_in_cooldown().is_none());
    }

    #[test]
    fn cooldown_no_extend_during_active() {
        let mut state = CooldownState::new();
        state.record_failure(CliErrorReason::RateLimit);
        assert_eq!(state.error_count, 1);
        let remaining1 = state.is_in_cooldown().unwrap();
        // Second failure during active cooldown should NOT increment
        state.record_failure(CliErrorReason::RateLimit);
        assert_eq!(state.error_count, 1); // unchanged
        let remaining2 = state.is_in_cooldown().unwrap();
        assert!(remaining2 <= remaining1);
    }

    // ── CliErrorReason::cooldown_ms ─────────────────────────────────────────

    #[test]
    fn cooldown_ms_rate_limit_escalation() {
        // First error: 60s
        assert_eq!(CliErrorReason::RateLimit.cooldown_ms(1), 60_000);
        // Second: 300s (5min)
        assert_eq!(CliErrorReason::RateLimit.cooldown_ms(2), 300_000);
        // Third: 1500s (25min)
        assert_eq!(CliErrorReason::RateLimit.cooldown_ms(3), 1_500_000);
        // Fourth: capped at 3600s (1h)
        assert_eq!(CliErrorReason::RateLimit.cooldown_ms(4), 3_600_000);
    }

    #[test]
    fn cooldown_ms_unknown_escalation() {
        // First: 30s
        assert_eq!(CliErrorReason::Unknown.cooldown_ms(1), 30_000);
        // Second: 150s
        assert_eq!(CliErrorReason::Unknown.cooldown_ms(2), 150_000);
    }

    // ── CliRunner::build_argv ───────────────────────────────────────────────

    #[test]
    fn build_argv_fresh_with_model_and_system() {
        let runner = CliRunner::new(default_claude_backend());
        let argv = runner.build_argv("sonnet", "Hello world", Some("Be kind"), None, false);
        assert_eq!(argv[0], "claude");
        assert!(argv.contains(&"--model".to_string()));
        assert!(argv.contains(&"sonnet".to_string()));
        assert!(argv.contains(&"--append-system-prompt".to_string()));
        assert!(argv.contains(&"Be kind".to_string()));
        // Prompt is the last element (Arg input mode)
        assert_eq!(argv.last().unwrap(), "Hello world");
    }

    #[test]
    fn build_argv_resume_with_session() {
        let runner = CliRunner::new(default_claude_backend());
        let argv = runner.build_argv("sonnet", "Follow up", None, Some("sess-42"), true);
        assert_eq!(argv[0], "claude");
        assert!(argv.contains(&"--resume".to_string()));
        assert!(argv.contains(&"sess-42".to_string()));
        // Model is NOT added during resume (resume_args used, not args)
        assert!(!argv.contains(&"--model".to_string()));
        assert_eq!(argv.last().unwrap(), "Follow up");
    }

    #[test]
    fn build_argv_no_model_arg() {
        let mut cfg = default_gemini_backend();
        cfg.model_arg = None;
        let runner = CliRunner::new(cfg);
        let argv = runner.build_argv("gemini-pro", "Test", None, None, false);
        // Without model_arg, model should not appear in argv
        assert!(!argv.contains(&"gemini-pro".to_string()));
        assert_eq!(argv.last().unwrap(), "Test");
    }

    #[test]
    fn build_argv_empty_model_skipped() {
        let runner = CliRunner::new(default_claude_backend());
        let argv = runner.build_argv("", "Prompt", None, None, false);
        // Empty model string should not produce --model flag
        assert!(!argv.contains(&"--model".to_string()));
    }

    #[test]
    fn build_argv_session_mode_none_skips_session() {
        let runner = CliRunner::new(default_gemini_backend());
        let argv = runner.build_argv("gemini-pro", "Hi", None, Some("s-1"), false);
        // Gemini has session_mode=None, so session_id should not appear
        assert!(!argv.contains(&"s-1".to_string()));
    }

    #[test]
    fn build_argv_empty_system_prompt_skipped() {
        let runner = CliRunner::new(default_claude_backend());
        let argv = runner.build_argv("sonnet", "Prompt", Some(""), None, false);
        // Empty system prompt should not produce the flag
        assert!(!argv.contains(&"--append-system-prompt".to_string()));
    }
}
