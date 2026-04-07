//! Generic CLI LLM provider — used for Claude CLI, Gemini CLI, and other CLI backends.

use super::*;
use std::sync::Arc;
use crate::agent::cli_backend::{CliBackendConfig, CliRunner, format_messages_for_cli};

/// Generic CLI-based LLM provider. Wraps CliRunner with a provider name.
/// Holds `Arc<SecretsManager>` to resolve API keys from vault at call time.
pub struct CliLlmProvider {
    runner: Arc<CliRunner>,
    provider_name: String,
    model: String,
    sandbox: Option<Arc<crate::containers::sandbox::CodeSandbox>>,
    agent_name: String,
    workspace_dir: String,
    base: bool,
    secrets: Arc<crate::secrets::SecretsManager>,
    env_key: Option<String>,
}

impl CliLlmProvider {
    pub fn new(
        provider_name: &str,
        config: CliBackendConfig,
        model: String,
        sandbox: Option<Arc<crate::containers::sandbox::CodeSandbox>>,
        agent_name: String,
        workspace_dir: String,
        base: bool,
        secrets: Arc<crate::secrets::SecretsManager>,
    ) -> Self {
        let env_key = config.env_key.clone();
        Self {
            runner: Arc::new(CliRunner::new(config)),
            provider_name: provider_name.to_string(),
            model, sandbox, agent_name, workspace_dir, base,
            secrets,
            env_key,
        }
    }
}

#[async_trait]
impl LlmProvider for CliLlmProvider {
    async fn chat(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let (prompt, system) = format_messages_for_cli(messages);

        // Resolve API key from vault at call time (hot-reloadable)
        let mut extra_env = std::collections::HashMap::new();
        if let Some(ref key_name) = self.env_key {
            if let Some(key_value) = self.secrets.get(key_name).await {
                extra_env.insert(key_name.clone(), key_value);
            }
        }

        let result = self.runner.run(
            &self.agent_name,
            &prompt,
            system.as_deref(),
            &self.model,
            self.sandbox.as_deref(),
            &self.workspace_dir,
            self.base,
            &extra_env,
        ).await?;

        Ok(LlmResponse {
            content: result.text,
            tool_calls: vec![],
            usage: result.usage,
            model: Some(format!("{}/{}", self.provider_name, self.model)),
            provider: Some(self.provider_name.clone()),
            fallback_notice: None,
            tools_used: vec![],
            iterations: 0,
            thinking_blocks: vec![],
        })
    }

    fn name(&self) -> &str { &self.provider_name }
    fn current_model(&self) -> String { self.model.clone() }
}

// Type aliases for backward compatibility
pub type ClaudeCliProvider = CliLlmProvider;
