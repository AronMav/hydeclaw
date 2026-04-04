use anyhow::Result;
use async_trait::async_trait;
use hydeclaw_types::{LlmResponse, Message, MessageRole, ToolDefinition};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::agent::cli_backend;
use crate::secrets::SecretsManager;

// Extracted provider implementations (submodules for full super:: access)
#[path = "providers_openai.rs"]
mod openai_impl;
use openai_impl::OpenAiCompatibleProvider;
#[path = "providers_anthropic.rs"]
mod anthropic_impl;
use anthropic_impl::AnthropicProvider;
#[cfg(test)]
use anthropic_impl::{AnthropicContentBlock, AnthropicResponse, AnthropicUsage, parse_anthropic_response};
#[path = "providers_google.rs"]
mod google_impl;
use google_impl::GoogleProvider;
#[cfg(test)]
use google_impl::messages_to_gemini_format;
#[path = "providers_claude_cli.rs"]
mod claude_cli_impl;
use claude_cli_impl::ClaudeCliProvider;
#[path = "providers_gemini_cli.rs"]
mod gemini_cli_impl;
use gemini_cli_impl::GeminiCliProvider;

/// Pluggable LLM provider trait.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse>;

    /// Streaming chat: sends content chunks via mpsc channel.
    /// Returns the complete LlmResponse when done.
    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<LlmResponse> {
        // Default: fall back to non-streaming and send entire content at once
        let response = self.chat(messages, tools).await?;
        if response.tool_calls.is_empty() {
            let filtered = super::thinking::strip_thinking(&response.content);
            if !filtered.is_empty() {
                chunk_tx.send(filtered).ok();
            }
        }
        Ok(response)
    }

    #[allow(dead_code)]
    fn name(&self) -> &str;

    /// Override the model for subsequent calls. None clears the override.
    fn set_model_override(&self, _model: Option<String>) {}

    /// Return the effective model name (override or default).
    fn current_model(&self) -> String {
        self.name().to_string()
    }
}

// ── ModelOverride ─────────────────────────────────────────────────────────────

/// Shared model-override logic: stores a default model name and an optional
/// runtime override (set via `/model` command). Eliminates identical code
/// across OpenAI, Anthropic, and Google providers.
pub(crate) struct ModelOverride {
    default: String,
    current: std::sync::RwLock<Option<String>>,
}

impl std::fmt::Display for ModelOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.effective())
    }
}

impl ModelOverride {
    pub fn new(default: String) -> Self {
        Self {
            default,
            current: std::sync::RwLock::new(None),
        }
    }

    /// Return the override if set, otherwise the default.
    pub fn effective(&self) -> String {
        self.current
            .read()
            .unwrap_or_else(|e| {
                tracing::warn!("ModelOverride RwLock poisoned on read, recovering");
                e.into_inner()
            })
            .clone()
            .unwrap_or_else(|| self.default.clone())
    }

    /// Set or clear the runtime override.
    pub fn set(&self, model: Option<String>) {
        *self.current.write().unwrap_or_else(|e| {
            tracing::warn!("ModelOverride RwLock poisoned on write, recovering");
            e.into_inner()
        }) = model;
    }
}

/// Known OpenAI-compatible providers: (name, default_base_url, api_key_env).
/// base_url is the base URL without path — chat path is resolved via `resolve_chat_url()`.
pub(crate) const OPENAI_COMPAT_PROVIDERS: &[(&str, &str, &str)] = &[
    ("minimax",    "https://api.minimax.io",         "MINIMAX_API_KEY"),
    ("deepseek",   "https://api.deepseek.com",       "DEEPSEEK_API_KEY"),
    ("groq",       "https://api.groq.com/openai",    "GROQ_API_KEY"),
    ("together",   "https://api.together.xyz",       "TOGETHER_API_KEY"),
    ("openrouter", "https://openrouter.ai/api",      "OPENROUTER_API_KEY"),
    ("mistral",    "https://api.mistral.ai",         "MISTRAL_API_KEY"),
    ("xai",        "https://api.x.ai",               "XAI_API_KEY"),
    ("perplexity", "https://api.perplexity.ai",      "PERPLEXITY_API_KEY"),
];

/// Create a provider from agent config.
/// API keys are read from SecretsManager on each LLM call (hot-reloadable).
/// `sandbox` + `agent_name` + `workspace_dir` are required for CLI providers (claude-cli, gemini-cli)
/// that execute inside the agent's Docker container.
#[allow(clippy::too_many_arguments)]
pub fn create_provider(
    provider_name: &str,
    model: &str,
    temperature: f64,
    max_tokens: Option<u32>,
    secrets: Arc<SecretsManager>,
    sandbox: Option<Arc<crate::containers::sandbox::CodeSandbox>>,
    agent_name: &str,
    workspace_dir: &str,
    base: bool,
) -> Arc<dyn LlmProvider> {
    match provider_name {
        "anthropic" => Arc::new(AnthropicProvider::new(
            model.to_string(),
            temperature,
            max_tokens,
            secrets,
        )),
        "google" | "gemini" => Arc::new(GoogleProvider::new(
            model.to_string(),
            temperature,
            max_tokens,
            secrets,
        )),
        "claude-cli" => Arc::new(ClaudeCliProvider::new(
            "claude-cli", cli_backend::default_claude_backend(), model.to_string(), sandbox, agent_name.to_string(), workspace_dir.to_string(), base,
        )),
        "gemini-cli" => Arc::new(GeminiCliProvider::new(
            "gemini-cli", cli_backend::default_gemini_backend(), model.to_string(), sandbox, agent_name.to_string(), workspace_dir.to_string(), base,
        )),
        "openai" => {
            let base = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string());
            let url = resolve_chat_url("openai", &base);
            Arc::new(OpenAiCompatibleProvider::new(
                "openai",
                &url,
                "OPENAI_API_KEY",
                model.to_string(),
                temperature,
                max_tokens,
                secrets,
            ))
        }
        "ollama" => {
            // Default URL used when OLLAMA_URL secret is not set
            let fallback = std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let url = resolve_chat_url("ollama", &fallback);
            Arc::new(OpenAiCompatibleProvider::new(
                "ollama",
                &url,
                "OLLAMA_API_KEY",
                model.to_string(),
                temperature,
                max_tokens,
                secrets,
            ).with_base_url_env("OLLAMA_URL", "/v1/chat/completions"))
        }
        other => {
            // Check known OpenAI-compatible providers
            if let Some((_, base_url, key_env)) = OPENAI_COMPAT_PROVIDERS.iter().find(|(n, _, _)| *n == other) {
                let url = resolve_chat_url(other, base_url);
                return Arc::new(OpenAiCompatibleProvider::new(
                    other,
                    &url,
                    key_env,
                    model.to_string(),
                    temperature,
                    max_tokens,
                    secrets,
                ));
            }
            tracing::warn!(provider = %other, "unknown provider, defaulting to minimax");
            let url = resolve_chat_url("minimax", "https://api.minimax.io");
            Arc::new(OpenAiCompatibleProvider::new(
                "minimax",
                &url,
                "MINIMAX_API_KEY",
                model.to_string(),
                temperature,
                max_tokens,
                secrets,
            ))
        }
    }
}

/// Build a provider from a routing rule entry, falling back to env-based defaults.
pub fn create_provider_from_route(
    route: &crate::config::ProviderRouteConfig,
    default_temperature: f64,
    secrets: Arc<SecretsManager>,
) -> Arc<dyn LlmProvider> {
    let temperature = route.temperature.unwrap_or(default_temperature);

    // Anthropic and Google have native APIs — delegate to their providers
    match route.provider.as_str() {
        "anthropic" => {
            return Arc::new(AnthropicProvider::with_options(
                route.model.clone(),
                temperature,
                route.max_tokens,
                secrets,
                route.base_url.clone(),
                route.api_key_env.clone(),
                route.prompt_cache,
            ));
        }
        "google" | "gemini" => {
            return Arc::new(GoogleProvider::with_options(
                route.model.clone(),
                temperature,
                route.max_tokens,
                secrets,
                route.base_url.clone(),
                route.api_key_env.clone(),
            ));
        }
        "claude-cli" | "gemini-cli" => {
            tracing::error!(provider = %route.provider, "CLI providers cannot be used in routing rules — use as primary provider instead");
            // Return a dummy provider that always errors
            return Arc::new(OpenAiCompatibleProvider::new(
                &route.provider, "http://invalid", "", route.model.clone(), 0.0, None, secrets,
            ));
        }
        _ => {}
    }

    let (url, api_key_name) = match route.provider.as_str() {
        "openai" => {
            let default_base = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string());
            let base = route.base_url.clone().unwrap_or(default_base);
            let url = resolve_chat_url("openai", &base);
            (
                url,
                route.api_key_env.clone().unwrap_or_else(|| "OPENAI_API_KEY".to_string()),
            )
        }
        "ollama" => {
            let default_host = std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let base = route.base_url.clone().unwrap_or(default_host);
            let url = resolve_chat_url("ollama", &base);
            (url, route.api_key_env.clone().unwrap_or_default())
        }
        other => {
            // Check known OpenAI-compatible providers
            if let Some((_, default_base, default_key)) = OPENAI_COMPAT_PROVIDERS.iter().find(|(n, _, _)| *n == other) {
                let base = route.base_url.clone().unwrap_or_else(|| default_base.to_string());
                let url = resolve_chat_url(other, &base);
                (
                    url,
                    route.api_key_env.clone().unwrap_or_else(|| default_key.to_string()),
                )
            } else {
                tracing::warn!(provider = %other, "unknown provider in routing rule, using minimax");
                let base = route.base_url.clone().unwrap_or_else(|| "https://api.minimax.io".to_string());
                let url = resolve_chat_url("minimax", &base);
                (
                    url,
                    route.api_key_env.clone().unwrap_or_else(|| "MINIMAX_API_KEY".to_string()),
                )
            }
        }
    };

    let provider = OpenAiCompatibleProvider::new(
        &route.provider,
        &url,
        &api_key_name,
        route.model.clone(),
        temperature,
        route.max_tokens,
        secrets,
    );

    // Apply round-robin keys if configured
    if !route.api_key_envs.is_empty() {
        Arc::new(provider.with_keys(route.api_key_envs.clone()))
    } else {
        Arc::new(provider)
    }
}

/// Create a routing provider from ordered route configs.
/// Returns a RoutingProvider that picks the right backend per request.
pub fn create_routing_provider(
    routes: &[crate::config::ProviderRouteConfig],
    default_temperature: f64,
    secrets: Arc<SecretsManager>,
) -> Arc<dyn LlmProvider> {
    let entries: Vec<RouteEntry> = routes
        .iter()
        .map(|r| {
            let p = create_provider_from_route(r, default_temperature, secrets.clone());
            RouteEntry {
                condition: r.condition.clone(),
                provider: p,
                cooldown_duration: std::time::Duration::from_secs(r.cooldown_secs),
            }
        })
        .collect();

    Arc::new(RoutingProvider {
        routes: entries,
        cooldowns: std::sync::Mutex::new(std::collections::HashMap::new()),
    })
}

/// Build full chat completions URL from base_url + provider's chat_path.
pub fn resolve_chat_url(provider_type: &str, base_url: &str) -> String {
    let chat_path = PROVIDER_TYPES.iter()
        .find(|pt| pt.id == provider_type)
        .map(|pt| pt.chat_path)
        .unwrap_or("/v1/chat/completions");
    if chat_path.is_empty() {
        return base_url.to_string();
    }
    format!("{}{}", base_url.trim_end_matches('/'), chat_path)
}

/// Default base URL for a provider type (from PROVIDER_TYPES).
pub fn default_base_url_for_type(provider_type: &str) -> &'static str {
    PROVIDER_TYPES.iter()
        .find(|pt| pt.id == provider_type)
        .map(|pt| pt.default_base_url)
        .unwrap_or("")
}

/// Migrate legacy agents (using `provider` field) to named connections (using `provider_connection`).
/// Runs once at startup. Idempotent — skips agents that already have `provider_connection`.
pub async fn migrate_legacy_providers(
    db: &sqlx::PgPool,
    agent_configs: &mut [crate::config::AgentConfig],
) {
    for agent_cfg in agent_configs.iter_mut() {
        // Skip if already migrated
        if agent_cfg.agent.provider_connection.as_ref().is_some_and(|c| !c.is_empty()) {
            continue;
        }

        // Skip if no legacy provider set
        if agent_cfg.agent.provider.is_empty() {
            continue;
        }

        let provider_name = format!("{}-default", agent_cfg.agent.provider);
        let agent_name = agent_cfg.agent.name.clone();

        // Check if provider already exists in DB
        match crate::db::providers::get_provider_by_name(db, &provider_name).await {
            Ok(Some(_)) => {
                // Provider already exists, just link the agent
            }
            Ok(None) => {
                // Create the provider
                let input = crate::db::providers::CreateProvider {
                    name: provider_name.clone(),
                    category: "llm".to_string(),
                    provider_type: agent_cfg.agent.provider.clone(),
                    base_url: {
                        let base = default_base_url_for_type(&agent_cfg.agent.provider);
                        if base.is_empty() { None } else { Some(base.to_string()) }
                    },
                    default_model: Some(agent_cfg.agent.model.clone()),
                    enabled: None,
                    options: None,
                    notes: Some(format!("Auto-migrated from legacy provider '{}'", agent_cfg.agent.provider)),
                };
                match crate::db::providers::create_provider(db, input).await {
                    Ok(row) => {
                        tracing::info!(
                            agent = %agent_name,
                            provider = %provider_name,
                            model = ?row.default_model,
                            "created LLM provider from legacy config"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            agent = %agent_name,
                            provider = %provider_name,
                            error = %e,
                            "failed to create LLM provider during migration"
                        );
                        continue;
                    }
                }
            }
            Err(e) => {
                tracing::error!(agent = %agent_name, error = %e, "DB error during provider migration");
                continue;
            }
        }

        // Update agent config
        agent_cfg.agent.provider_connection = Some(provider_name.clone());

        // Save updated TOML
        let path = format!("config/agents/{}.toml", agent_name);
        match agent_cfg.to_toml() {
            Ok(toml_str) => {
                if let Err(e) = std::fs::write(&path, &toml_str) {
                    tracing::error!(agent = %agent_name, error = %e, "failed to save migrated agent TOML");
                } else {
                    tracing::info!(
                        agent = %agent_name,
                        provider_connection = %provider_name,
                        "migrated agent to named provider connection"
                    );
                }
            }
            Err(e) => {
                tracing::error!(agent = %agent_name, error = %e, "failed to serialize migrated agent config");
            }
        }
    }
}

// ── Named connection provider types ───────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderTypeMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub chat_path: &'static str,
    pub default_base_url: &'static str,
    pub default_secret_name: &'static str,
    pub requires_api_key: bool,
    pub supports_model_listing: bool,
}

/// Known provider types with extended metadata.
pub(crate) const PROVIDER_TYPES: &[ProviderTypeMeta] = &[
    ProviderTypeMeta {
        id: "minimax",
        name: "MiniMax",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.minimax.io",
        default_secret_name: "MINIMAX_API_KEY",
        requires_api_key: true,
        supports_model_listing: false,
    },
    ProviderTypeMeta {
        id: "openai",
        name: "OpenAI",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.openai.com",
        default_secret_name: "OPENAI_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "anthropic",
        name: "Anthropic",
        chat_path: "",
        default_base_url: "https://api.anthropic.com",
        default_secret_name: "ANTHROPIC_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "google",
        name: "Google Gemini",
        chat_path: "",
        default_base_url: "https://generativelanguage.googleapis.com",
        default_secret_name: "GOOGLE_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "deepseek",
        name: "DeepSeek",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.deepseek.com",
        default_secret_name: "DEEPSEEK_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "groq",
        name: "Groq",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.groq.com/openai",
        default_secret_name: "GROQ_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "openrouter",
        name: "OpenRouter",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://openrouter.ai/api",
        default_secret_name: "OPENROUTER_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "mistral",
        name: "Mistral",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.mistral.ai",
        default_secret_name: "MISTRAL_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "xai",
        name: "xAI",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.x.ai",
        default_secret_name: "XAI_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "perplexity",
        name: "Perplexity",
        chat_path: "/chat/completions",
        default_base_url: "https://api.perplexity.ai",
        default_secret_name: "PERPLEXITY_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "together",
        name: "Together AI",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.together.xyz",
        default_secret_name: "TOGETHER_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "ollama",
        name: "Ollama",
        chat_path: "/v1/chat/completions",
        default_base_url: "http://localhost:11434",
        default_secret_name: "",
        requires_api_key: false,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "openai_compat",
        name: "OpenAI Compatible",
        chat_path: "/v1/chat/completions",
        default_base_url: "",
        default_secret_name: "API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "claude-cli",
        name: "Claude CLI",
        chat_path: "",
        default_base_url: "",
        default_secret_name: "",
        requires_api_key: false,
        supports_model_listing: false,
    },
    ProviderTypeMeta {
        id: "gemini-cli",
        name: "Gemini CLI",
        chat_path: "",
        default_base_url: "",
        default_secret_name: "",
        requires_api_key: false,
        supports_model_listing: false,
    },
    // ── Additional OpenAI-compatible providers ──────────────────────────────
    ProviderTypeMeta {
        id: "huggingface",
        name: "Hugging Face",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api-inference.huggingface.co",
        default_secret_name: "HF_API_KEY",
        requires_api_key: true,
        supports_model_listing: false,
    },
    ProviderTypeMeta {
        id: "moonshot",
        name: "Moonshot AI (Kimi)",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.moonshot.cn",
        default_secret_name: "MOONSHOT_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "nvidia",
        name: "NVIDIA",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://integrate.api.nvidia.com",
        default_secret_name: "NVIDIA_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "venice",
        name: "Venice AI",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.venice.ai",
        default_secret_name: "VENICE_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "cloudflare",
        name: "Cloudflare AI Gateway",
        chat_path: "/v1/chat/completions",
        default_base_url: "",
        default_secret_name: "CF_AI_API_KEY",
        requires_api_key: true,
        supports_model_listing: false,
    },
    ProviderTypeMeta {
        id: "litellm",
        name: "LiteLLM",
        chat_path: "/v1/chat/completions",
        default_base_url: "http://localhost:4000",
        default_secret_name: "LITELLM_API_KEY",
        requires_api_key: false,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "volcengine",
        name: "Volcengine (Doubao)",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://ark.cn-beijing.volces.com/api",
        default_secret_name: "VOLCENGINE_API_KEY",
        requires_api_key: true,
        supports_model_listing: false,
    },
    ProviderTypeMeta {
        id: "qwen",
        name: "Qwen (Alibaba)",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://dashscope.aliyuncs.com/compatible-mode",
        default_secret_name: "DASHSCOPE_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "glm",
        name: "GLM (Zhipu AI)",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://open.bigmodel.cn/api/paas",
        default_secret_name: "GLM_API_KEY",
        requires_api_key: true,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "sglang",
        name: "SGLang",
        chat_path: "/v1/chat/completions",
        default_base_url: "http://localhost:30000",
        default_secret_name: "",
        requires_api_key: false,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "vllm",
        name: "vLLM",
        chat_path: "/v1/chat/completions",
        default_base_url: "http://localhost:8000",
        default_secret_name: "",
        requires_api_key: false,
        supports_model_listing: true,
    },
    ProviderTypeMeta {
        id: "qianfan",
        name: "Qianfan (Baidu)",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://qianfan.baidubce.com",
        default_secret_name: "QIANFAN_API_KEY",
        requires_api_key: true,
        supports_model_listing: false,
    },
    ProviderTypeMeta {
        id: "xiaomi",
        name: "Xiaomi MiLM",
        chat_path: "/v1/chat/completions",
        default_base_url: "https://api.ai.xiaomi.com",
        default_secret_name: "XIAOMI_API_KEY",
        requires_api_key: true,
        supports_model_listing: false,
    },
];

/// Vault secret name for all provider credentials (scoped by provider UUID).
pub(crate) const PROVIDER_CREDENTIALS: &str = "PROVIDER_CREDENTIALS";

/// Legacy vault secret name — kept only for migration lookups.
pub(crate) const LLM_CREDENTIALS: &str = "LLM_CREDENTIALS";

/// Resolve API key from vault-scoped credential, falling back to legacy secret name.
pub(crate) async fn resolve_credential(
    secrets: &SecretsManager,
    credential_scope: Option<&str>,
    fallback_name: &str,
) -> Option<String> {
    if let Some(scope) = credential_scope
        && let Some(val) = secrets.get_scoped(PROVIDER_CREDENTIALS, scope).await {
            return Some(val);
        }
    if !fallback_name.is_empty() {
        return secrets.get(fallback_name).await;
    }
    None
}

/// Create an LLM provider from a named DB connection.
#[allow(clippy::too_many_arguments)]
pub fn create_provider_from_connection(
    conn: &crate::db::providers::ProviderRow,
    model_override: Option<&str>,
    temperature: f64,
    max_tokens: Option<u32>,
    secrets: Arc<SecretsManager>,
    sandbox: Option<Arc<crate::containers::sandbox::CodeSandbox>>,
    agent_name: &str,
    workspace_dir: &str,
    base: bool,
) -> Arc<dyn LlmProvider> {
    let model = model_override.unwrap_or(conn.default_model.as_deref().unwrap_or("")).to_string();
    let key_env = "";
    let credential_scope = conn.id.to_string();

    match conn.provider_type.as_str() {
        "anthropic" => Arc::new(AnthropicProvider::with_options(
            model,
            temperature,
            max_tokens,
            secrets,
            conn.base_url.clone(),
            Some(key_env.to_string()),
            false,
        ).with_credential_scope(credential_scope)),
        "google" | "gemini" => Arc::new(GoogleProvider::with_options(
            model,
            temperature,
            max_tokens,
            secrets,
            conn.base_url.clone(),
            Some(key_env.to_string()),
        ).with_credential_scope(credential_scope)),
        "claude-cli" => Arc::new(ClaudeCliProvider::new(
            "claude-cli", cli_backend::default_claude_backend(), model, sandbox, agent_name.to_string(), workspace_dir.to_string(), base,
        )),
        "gemini-cli" => Arc::new(GeminiCliProvider::new(
            "gemini-cli", cli_backend::default_gemini_backend(), model, sandbox, agent_name.to_string(), workspace_dir.to_string(), base,
        )),
        "openai" => {
            let base = conn.base_url.as_deref().unwrap_or("https://api.openai.com");
            let url = resolve_chat_url("openai", base);
            Arc::new(OpenAiCompatibleProvider::new(
                "openai", &url, key_env, model, temperature, max_tokens, secrets,
            ).with_credential_scope(credential_scope))
        }
        "ollama" => {
            let base = conn.base_url.as_deref().unwrap_or("http://localhost:11434");
            let url = resolve_chat_url("ollama", base);
            Arc::new(OpenAiCompatibleProvider::new(
                "ollama", &url, key_env, model, temperature, max_tokens, secrets,
            ).with_credential_scope(credential_scope))
        }
        other => {
            let default_base = default_base_url_for_type(other);
            let base = conn.base_url.as_deref()
                .unwrap_or(if default_base.is_empty() { "https://api.minimax.io" } else { default_base });
            let url = resolve_chat_url(other, base);
            Arc::new(OpenAiCompatibleProvider::new(
                other, &url, key_env, model, temperature, max_tokens, secrets,
            ).with_credential_scope(credential_scope))
        }
    }
}

/// Resolve LLM provider for an agent from a named connection in the DB.
/// The agent MUST have `provider_connection` set (auto-migrated at startup).
/// Falls back to legacy `provider` field only if named connection lookup fails.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_provider_for_agent(
    db: &sqlx::PgPool,
    agent: &crate::config::AgentSettings,
    temperature: f64,
    max_tokens: Option<u32>,
    secrets: Arc<SecretsManager>,
    sandbox: Option<Arc<crate::containers::sandbox::CodeSandbox>>,
    agent_name: &str,
    workspace_dir: &str,
    base: bool,
) -> Arc<dyn LlmProvider> {
    if let Some(conn_name) = agent.provider_connection.as_deref().filter(|s| !s.is_empty()) {
        match crate::db::providers::get_provider_by_name(db, conn_name).await {
            Ok(Some(conn)) if conn.category == "text" => {
                tracing::debug!(agent = %agent_name, connection = %conn_name, "using named LLM provider");
                let model_override = if agent.model.is_empty() { None } else { Some(agent.model.as_str()) };
                return create_provider_from_connection(
                    &conn,
                    model_override,
                    temperature,
                    max_tokens,
                    secrets,
                    sandbox,
                    agent_name,
                    workspace_dir,
                    base,
                );
            }
            Ok(Some(conn)) => {
                tracing::warn!(agent = %agent_name, connection = %conn_name, category = %conn.category,
                    "named provider is not type=text, falling back to legacy provider");
            }
            Ok(None) => {
                tracing::warn!(agent = %agent_name, connection = %conn_name,
                    "named provider connection not found in DB, falling back to legacy provider");
            }
            Err(e) => {
                tracing::error!(agent = %agent_name, error = %e,
                    "DB error resolving provider connection, falling back to legacy provider");
            }
        }
    }

    // Legacy fallback — kept for backward compatibility during migration window.
    // NOTE: vault-scoped keys (PROVIDER_CREDENTIALS) are not consulted here.
    // This path is only reached when provider_connection is not set and DB lookup fails.
    create_provider(
        &agent.provider,
        &agent.model,
        temperature,
        max_tokens,
        secrets,
        sandbox,
        agent_name,
        workspace_dir,
        base,
    )
}

// ── RoutingProvider ───────────────────────────────────────────────────────────

struct RouteEntry {
    condition: String,
    provider: Arc<dyn LlmProvider>,
    cooldown_duration: std::time::Duration,
}

/// Routing provider: selects the appropriate backend based on message characteristics.
pub struct RoutingProvider {
    routes: Vec<RouteEntry>,
    /// Tracks providers on cooldown (provider name → cooldown expiry).
    cooldowns: std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

impl RoutingProvider {
    /// Choose the best matching provider for the given messages and tools.
    /// Evaluates conditions in order; returns the first match.
    /// Falls back to the last route if nothing else matches.
    fn select_route(
        &self,
        messages: &[hydeclaw_types::Message],
        tools: &[hydeclaw_types::ToolDefinition],
    ) -> &RouteEntry {
        let last_user_msg = messages
            .iter()
            .rev()
            .find(|m| m.role == hydeclaw_types::MessageRole::User)
            .map(|m| m.content.as_str())
            .unwrap_or("");

        let last_user_len = last_user_msg.len();
        let lower = last_user_msg.to_lowercase();

        for entry in &self.routes {
            let matches = match entry.condition.as_str() {
                "short" => last_user_len < 300,
                "long" => last_user_len > 2000,
                "with_tools" => !tools.is_empty(),
                "financial" => contains_any(&lower, FINANCIAL_KEYWORDS),
                "analytical" => contains_any(&lower, ANALYTICAL_KEYWORDS),
                "code" => contains_any(&lower, CODE_KEYWORDS),
                "default" | "always" => true,
                "fallback" => false, // only used via explicit fallback logic below
                _ => false,
            };
            if matches {
                tracing::debug!(condition = %entry.condition, "routing condition matched");
                return entry;
            }
        }

        // Last resort: return last route (or first if routes is empty — shouldn't happen)
        self.routes.last()
            .or_else(|| self.routes.first())
            .expect("RoutingProvider has no routes")
    }

    /// Check if a provider is on cooldown.
    fn is_on_cooldown(&self, name: &str) -> bool {
        let map = self.cooldowns.lock().unwrap_or_else(|e| {
            tracing::warn!("cooldowns Mutex poisoned, recovering");
            e.into_inner()
        });
        map.get(name).map(|exp| std::time::Instant::now() < *exp).unwrap_or(false)
    }

    /// Put a provider on cooldown.
    fn set_cooldown(&self, name: &str, duration: std::time::Duration) {
        let mut map = self.cooldowns.lock().unwrap_or_else(|e| {
            tracing::warn!("cooldowns Mutex poisoned on write, recovering");
            e.into_inner()
        });
        map.insert(name.to_string(), std::time::Instant::now() + duration);
    }

    /// Classify error and apply appropriate cooldown. Returns the computed cooldown duration.
    fn handle_provider_error(&self, e: &anyhow::Error, provider_name: &str, max_cooldown: std::time::Duration) {
        let class = super::error_classify::classify(e);
        let cd = super::error_classify::cooldown_duration(&class).min(max_cooldown);
        tracing::warn!(provider = %provider_name, error = %e, error_class = ?class, cooldown_secs = cd.as_secs(), "provider failed");
        if !cd.is_zero() { self.set_cooldown(provider_name, cd); }
    }

    /// Get all route entries that could serve as fallbacks (not on cooldown, not excluded).
    fn available_fallbacks(&self, exclude_name: &str) -> Vec<&RouteEntry> {
        self.routes
            .iter()
            .filter(|e| e.provider.name() != exclude_name && !self.is_on_cooldown(e.provider.name()))
            .collect()
    }
}

// ── Keyword sets for semantic routing ─────────────────────────────────────────

const FINANCIAL_KEYWORDS: &[&str] = &[
    // Russian
    "портфель", "акции", "бумаги", "дивиденды", "доходность", "прибыль", "убыток",
    "imoex", "ртс", "мосбиржа", "moex", "облигации", "фонд", "etf", "паи",
    "котировки", "инвестиц", "брокер", "позиции", "активы", "тикер",
    // English
    "portfolio", "shares", "dividend", "yield", "return", "profit", "loss",
    "stock", "bond", "equity", "ticker", "market",
];

const ANALYTICAL_KEYWORDS: &[&str] = &[
    // Russian
    "анализируй", "подсчитай", "посчитай", "вычисли", "рассчитай", "сравни",
    "корреляция", "среднее", "медиана", "статистика", "динамика", "тренд",
    "процент", "прогноз", "агрегируй", "сгруппируй",
    // English
    "analyze", "calculate", "compute", "correlation", "average", "median",
    "statistics", "trend", "forecast", "aggregate",
];

const CODE_KEYWORDS: &[&str] = &[
    // Russian
    "скрипт", "код", "запусти", "выполни", "python", "bash",
    "напиши скрипт", "напиши код",
    // English
    "script", "code", "execute", "run script", "run code",
];

fn contains_any(text: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|kw| text.contains(kw))
}

// ── RoutingProvider LlmProvider impl ─────────────────────────────────────────
// NOTE: chat() and chat_stream() have identical routing/fallback logic but
// cannot be unified into a generic helper due to async closure lifetime issues
// with trait objects. The duplication is intentional and kept in sync.

#[async_trait::async_trait]
impl LlmProvider for RoutingProvider {
    async fn chat(
        &self,
        messages: &[hydeclaw_types::Message],
        tools: &[hydeclaw_types::ToolDefinition],
    ) -> Result<hydeclaw_types::LlmResponse> {
        let primary = self.select_route(messages, tools);
        let primary_name = primary.provider.name().to_string();
        let primary_cooldown = primary.cooldown_duration;

        let primary_skipped = self.is_on_cooldown(&primary_name);
        if !primary_skipped {
            match primary.provider.chat(messages, tools).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    self.handle_provider_error(&e, &primary_name, primary_cooldown);
                }
            }
        } else {
            tracing::debug!(provider = %primary_name, "primary on cooldown, skipping");
        }

        for fb in self.available_fallbacks(&primary_name) {
            tracing::info!(provider = %fb.provider.name(), "trying fallback provider");
            match fb.provider.chat(messages, tools).await {
                Ok(mut resp) => {
                    let reason = if primary_skipped { "cooldown" } else { "primary_failed" };
                    resp.fallback_notice = Some(format!("↪️ {} → {} ({})", primary_name, fb.provider.name(), reason));
                    return Ok(resp);
                }
                Err(e) => {
                    self.handle_provider_error(&e, fb.provider.name(), fb.cooldown_duration);
                }
            }
        }
        anyhow::bail!("all providers failed (including fallbacks)")
    }

    async fn chat_stream(
        &self,
        messages: &[hydeclaw_types::Message],
        tools: &[hydeclaw_types::ToolDefinition],
        chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<hydeclaw_types::LlmResponse> {
        let primary = self.select_route(messages, tools);
        let primary_name = primary.provider.name().to_string();
        let primary_cooldown = primary.cooldown_duration;

        let primary_skipped = self.is_on_cooldown(&primary_name);
        if !primary_skipped {
            use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
            let chunks_sent = Arc::new(AtomicBool::new(false));
            let (tracking_tx, mut tracking_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let forwarder = {
                let sentinel = chunks_sent.clone();
                let forward_tx = chunk_tx.clone();
                tokio::spawn(async move {
                    while let Some(chunk) = tracking_rx.recv().await {
                        sentinel.store(true, Ordering::Relaxed);
                        forward_tx.send(chunk).ok();
                    }
                })
            };

            match primary.provider.chat_stream(messages, tools, tracking_tx).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    // tracking_tx is now consumed/dropped by the call above.
                    // Wait for the forwarder to drain any buffered chunks before
                    // reading chunks_sent — this eliminates the race condition.
                    let _ = forwarder.await;

                    if chunks_sent.load(Ordering::Relaxed) {
                        tracing::warn!(provider = %primary_name, error = %e,
                            "streaming: mid-stream failure, partial output already sent — cannot failover");
                        return Err(e);
                    }
                    let class = super::error_classify::classify(&e);
                    let cd = super::error_classify::cooldown_duration(&class).min(primary_cooldown);
                    tracing::warn!(provider = %primary_name, error = %e, error_class = ?class,
                        "streaming: primary failed before first chunk, trying fallback chain");
                    if !cd.is_zero() { self.set_cooldown(&primary_name, cd); }
                }
            }
        } else {
            tracing::debug!(provider = %primary_name, "primary on cooldown, skipping for streaming");
        }

        for fb in self.available_fallbacks(&primary_name) {
            tracing::info!(provider = %fb.provider.name(), "trying streaming fallback provider");
            match fb.provider.chat_stream(messages, tools, chunk_tx.clone()).await {
                Ok(mut resp) => {
                    let reason = if primary_skipped { "cooldown" } else { "primary_failed" };
                    resp.fallback_notice = Some(format!("↪️ {} → {} ({})", primary_name, fb.provider.name(), reason));
                    return Ok(resp);
                }
                Err(e) => {
                    let class = super::error_classify::classify(&e);
                    let cd = super::error_classify::cooldown_duration(&class).min(fb.cooldown_duration);
                    tracing::warn!(provider = %fb.provider.name(), error = %e, error_class = ?class, "streaming fallback also failed");
                    if !cd.is_zero() { self.set_cooldown(fb.provider.name(), cd); }
                }
            }
        }
        anyhow::bail!("all streaming providers failed (including fallbacks)")
    }

    fn name(&self) -> &str {
        "routing"
    }

    fn set_model_override(&self, model: Option<String>) {
        for entry in &self.routes {
            entry.provider.set_model_override(model.clone());
        }
    }

    fn current_model(&self) -> String {
        self.routes
            .first()
            .map(|e| e.provider.current_model())
            .unwrap_or_else(|| "unknown".to_string())
    }
}





// ── OpenAI wire format helpers ──────────────────────────────────────────────

/// Transform internal Message structs to OpenAI API wire format.
/// Key differences from serde default:
///
/// - tool_calls: wrapped in `{type: "function", function: {name, arguments_as_string}}`
/// - Remove tool messages whose `tool_call_id` has no preceding assistant message with a
///   matching tool call. This prevents MiniMax/OpenAI "tool result does not follow tool call"
///   errors caused by history truncation cutting off the assistant message while keeping the
///   tool result.
pub(super) fn strip_orphaned_tool_messages(messages: &[Message]) -> Vec<Message> {
    // Pass 1: collect all tool_call_ids that have a saved tool result.
    let mut result_ids = std::collections::HashSet::<String>::new();
    for msg in messages {
        if msg.role == MessageRole::Tool
            && let Some(ref id) = msg.tool_call_id {
                result_ids.insert(id.clone());
            }
    }

    // Pass 2: rebuild messages, skipping incomplete assistant+tool_calls groups
    // (where some tool results are missing — e.g. process crashed after saving assistant msg).
    let mut valid_call_ids = std::collections::HashSet::<String>::new();
    let mut result = Vec::with_capacity(messages.len());

    for msg in messages {
        match msg.role {
            MessageRole::Assistant => {
                if let Some(ref tcs) = msg.tool_calls
                    && !tcs.is_empty() {
                        let complete = tcs.iter().all(|tc| result_ids.contains(&tc.id));
                        if !complete {
                            tracing::warn!(
                                "dropping assistant+tool_calls message: \
                                 some tool results missing from history (incomplete save)"
                            );
                            continue;
                        }
                        for tc in tcs {
                            valid_call_ids.insert(tc.id.clone());
                        }
                    }
                result.push(msg.clone());
            }
            MessageRole::Tool => {
                let id = msg.tool_call_id.as_deref().unwrap_or("");
                if valid_call_ids.contains(id) {
                    result.push(msg.clone());
                } else {
                    tracing::warn!(
                        tool_call_id = id,
                        "dropping orphaned tool message (no preceding tool_call in context)"
                    );
                }
            }
            _ => result.push(msg.clone()),
        }
    }

    result
}

/// - tool messages: include `tool_call_id` at top level
/// - assistant content must be null (not empty string) when only tool_calls present
pub(super) fn messages_to_openai_format(messages: &[Message]) -> Vec<serde_json::Value> {
    let messages = strip_orphaned_tool_messages(messages);
    messages
        .iter()
        .map(|msg| {
            let mut m = serde_json::Map::new();
            m.insert(
                "role".to_string(),
                serde_json::to_value(&msg.role).unwrap_or_default(),
            );

            // Assistant with tool_calls: content can be null
            if msg.role == MessageRole::Assistant
                && let Some(ref tool_calls) = msg.tool_calls
                    && !tool_calls.is_empty() {
                        if msg.content.is_empty() {
                            m.insert("content".to_string(), serde_json::Value::Null);
                        } else {
                            m.insert(
                                "content".to_string(),
                                serde_json::Value::String(msg.content.clone()),
                            );
                        }

                        let tc_json: Vec<serde_json::Value> = tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": serde_json::to_string(&tc.arguments)
                                            .unwrap_or_else(|_| "{}".to_string())
                                    }
                                })
                            })
                            .collect();
                        m.insert(
                            "tool_calls".to_string(),
                            serde_json::Value::Array(tc_json),
                        );

                        return serde_json::Value::Object(m);
                    }

            m.insert(
                "content".to_string(),
                serde_json::Value::String(msg.content.clone()),
            );

            if let Some(ref tool_call_id) = msg.tool_call_id {
                m.insert(
                    "tool_call_id".to_string(),
                    serde_json::Value::String(tool_call_id.clone()),
                );
            }

            serde_json::Value::Object(m)
        })
        .collect()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use hydeclaw_types::{Message, MessageRole, ToolCall};

    // ── helpers ──────────────────────────────────────────────────────────────

    fn user_msg(content: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        }
    }

    fn assistant_msg(content: &str) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        }
    }

    fn assistant_with_calls(content: &str, calls: Vec<(&str, &str)>) -> Message {
        let tool_calls = calls
            .into_iter()
            .map(|(id, name)| ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: serde_json::json!({}),
            })
            .collect();
        Message {
            role: MessageRole::Assistant,
            content: content.to_string(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            thinking_blocks: vec![],
        }
    }

    fn tool_msg(call_id: &str, content: &str) -> Message {
        Message {
            role: MessageRole::Tool,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: Some(call_id.to_string()),
            thinking_blocks: vec![],
        }
    }

    fn system_msg(content: &str) -> Message {
        Message {
            role: MessageRole::System,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        }
    }

    // ── ModelOverride tests ───────────────────────────────────────────────────

    #[test]
    fn model_override_new_returns_default() {
        let mo = ModelOverride::new("gpt-4".to_string());
        assert_eq!(mo.effective(), "gpt-4");
    }

    #[test]
    fn model_override_set_some_overrides_default() {
        let mo = ModelOverride::new("gpt-4".to_string());
        mo.set(Some("claude-3".to_string()));
        assert_eq!(mo.effective(), "claude-3");
    }

    #[test]
    fn model_override_set_none_reverts_to_default() {
        let mo = ModelOverride::new("gpt-4".to_string());
        mo.set(Some("claude-3".to_string()));
        mo.set(None);
        assert_eq!(mo.effective(), "gpt-4");
    }

    #[test]
    fn model_override_display_returns_effective() {
        let mo = ModelOverride::new("gpt-4".to_string());
        assert_eq!(format!("{mo}"), "gpt-4");
        mo.set(Some("claude-3".to_string()));
        assert_eq!(format!("{mo}"), "claude-3");
    }

    #[test]
    fn model_override_multiple_sets() {
        let mo = ModelOverride::new("base".to_string());
        mo.set(Some("first".to_string()));
        mo.set(Some("second".to_string()));
        assert_eq!(mo.effective(), "second");
    }

    // ── parse_anthropic_response tests ───────────────────────────────────────

    fn text_block(text: &str) -> AnthropicContentBlock {
        AnthropicContentBlock::Text { text: text.to_string() }
    }

    fn tool_block(id: &str, name: &str, input: serde_json::Value) -> AnthropicContentBlock {
        AnthropicContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input,
        }
    }

    #[test]
    fn parse_anthropic_text_only_no_usage() {
        let resp = AnthropicResponse {
            content: vec![text_block("hello")],
            usage: None,
        };
        let result = parse_anthropic_response(resp, "claude-3");
        assert_eq!(result.content, "hello");
        assert!(result.tool_calls.is_empty());
        assert!(result.usage.is_none());
        assert_eq!(result.model.as_deref(), Some("claude-3"));
        assert_eq!(result.provider.as_deref(), Some("anthropic"));
    }

    #[test]
    fn parse_anthropic_tool_use_only() {
        let resp = AnthropicResponse {
            content: vec![tool_block("call-1", "search", serde_json::json!({"q": "rust"}))],
            usage: None,
        };
        let result = parse_anthropic_response(resp, "claude-3");
        assert_eq!(result.content, "");
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].id, "call-1");
        assert_eq!(result.tool_calls[0].name, "search");
        assert_eq!(result.tool_calls[0].arguments, serde_json::json!({"q": "rust"}));
    }

    #[test]
    fn parse_anthropic_mixed_text_and_tool() {
        let resp = AnthropicResponse {
            content: vec![
                text_block("a"),
                tool_block("c1", "do_thing", serde_json::json!({})),
                text_block("b"),
            ],
            usage: None,
        };
        let result = parse_anthropic_response(resp, "model");
        // texts are joined with \n
        assert_eq!(result.content, "a\nb");
        assert_eq!(result.tool_calls.len(), 1);
    }

    #[test]
    fn parse_anthropic_with_usage() {
        let resp = AnthropicResponse {
            content: vec![text_block("hi")],
            usage: Some(AnthropicUsage { input_tokens: 10, output_tokens: 20, cache_creation_input_tokens: None, cache_read_input_tokens: None }),
        };
        let result = parse_anthropic_response(resp, "model");
        let usage = result.usage.expect("usage should be Some");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 20);
    }

    #[test]
    fn parse_anthropic_other_block_ignored() {
        let resp = AnthropicResponse {
            content: vec![AnthropicContentBlock::Other],
            usage: None,
        };
        let result = parse_anthropic_response(resp, "model");
        assert_eq!(result.content, "");
        assert!(result.tool_calls.is_empty());
    }

    #[test]
    fn parse_anthropic_empty_content() {
        let resp = AnthropicResponse {
            content: vec![],
            usage: None,
        };
        let result = parse_anthropic_response(resp, "model");
        assert_eq!(result.content, "");
        assert!(result.tool_calls.is_empty());
        assert!(result.usage.is_none());
    }

    #[test]
    fn parse_anthropic_multiple_tool_calls() {
        let resp = AnthropicResponse {
            content: vec![
                tool_block("c1", "tool_a", serde_json::json!({"x": 1})),
                tool_block("c2", "tool_b", serde_json::json!({"y": 2})),
            ],
            usage: Some(AnthropicUsage { input_tokens: 5, output_tokens: 15, cache_creation_input_tokens: None, cache_read_input_tokens: None }),
        };
        let result = parse_anthropic_response(resp, "m");
        assert_eq!(result.tool_calls.len(), 2);
        assert_eq!(result.tool_calls[0].id, "c1");
        assert_eq!(result.tool_calls[1].id, "c2");
    }

    // ── strip_orphaned_tool_messages tests ───────────────────────────────────

    #[test]
    fn strip_no_tool_messages_unchanged() {
        let msgs = vec![user_msg("hi"), assistant_msg("hello")];
        let result = strip_orphaned_tool_messages(&msgs);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "hi");
        assert_eq!(result[1].content, "hello");
    }

    #[test]
    fn strip_complete_pair_kept() {
        let msgs = vec![
            user_msg("go"),
            assistant_with_calls("", vec![("tc1", "tool_x")]),
            tool_msg("tc1", "result"),
        ];
        let result = strip_orphaned_tool_messages(&msgs);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn strip_orphaned_tool_message_dropped() {
        // Tool message with no matching assistant tool_call
        let msgs = vec![user_msg("hi"), tool_msg("tc1", "orphan result")];
        let result = strip_orphaned_tool_messages(&msgs);
        // orphaned tool dropped, user kept
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, MessageRole::User);
    }

    #[test]
    fn strip_incomplete_assistant_dropped_with_tool() {
        // assistant requested tc1 and tc2, but only tc1 result exists
        let msgs = vec![
            user_msg("go"),
            assistant_with_calls("", vec![("tc1", "a"), ("tc2", "b")]),
            tool_msg("tc1", "res1"),
        ];
        let result = strip_orphaned_tool_messages(&msgs);
        // assistant dropped (tc2 missing), tc1 tool also dropped (no valid call)
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, MessageRole::User);
    }

    #[test]
    fn strip_empty_input_returns_empty() {
        let result = strip_orphaned_tool_messages(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn strip_system_and_user_always_kept() {
        let msgs = vec![system_msg("sys"), user_msg("usr")];
        let result = strip_orphaned_tool_messages(&msgs);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, MessageRole::System);
        assert_eq!(result[1].role, MessageRole::User);
    }

    #[test]
    fn strip_assistant_no_tool_calls_kept() {
        // Assistant message without tool_calls is always kept
        let msgs = vec![user_msg("hi"), assistant_msg("plain reply")];
        let result = strip_orphaned_tool_messages(&msgs);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn strip_two_complete_pairs_both_kept() {
        let msgs = vec![
            user_msg("q1"),
            assistant_with_calls("", vec![("tc1", "tool_a")]),
            tool_msg("tc1", "res1"),
            user_msg("q2"),
            assistant_with_calls("", vec![("tc2", "tool_b")]),
            tool_msg("tc2", "res2"),
        ];
        let result = strip_orphaned_tool_messages(&msgs);
        assert_eq!(result.len(), 6);
    }

    // ── contains_any tests ────────────────────────────────────────────────────

    #[test]
    fn contains_any_match_found_returns_true() {
        assert!(contains_any("write a script", &["script", "code"]));
    }

    #[test]
    fn contains_any_no_match_returns_false() {
        assert!(!contains_any("hello world", &["script", "code", "execute"]));
    }

    #[test]
    fn contains_any_empty_keywords_returns_false() {
        assert!(!contains_any("anything goes here", &[]));
    }

    // ── messages_to_gemini_format tests ───────────────────────────────────────

    #[test]
    fn gemini_system_extracted_user_and_assistant_mapped() {
        let msgs = vec![
            system_msg("You are helpful."),
            user_msg("Hello"),
            assistant_msg("Hi there!"),
        ];
        let (system, contents) = messages_to_gemini_format(&msgs);
        assert_eq!(system.as_deref(), Some("You are helpful."));
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["text"], "Hi there!");
    }

    #[test]
    fn gemini_tool_message_becomes_function_response() {
        let msgs = vec![
            assistant_with_calls("", vec![("tc1", "get_weather")]),
            tool_msg("tc1", "Sunny, 25°C"),
        ];
        let (_system, contents) = messages_to_gemini_format(&msgs);
        // second item is the tool result
        let tool_content = &contents[1];
        assert_eq!(tool_content["role"], "user");
        let fr = &tool_content["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], "tc1");
        assert_eq!(fr["response"]["result"], "Sunny, 25°C");
    }

    #[test]
    fn gemini_assistant_with_tool_calls_becomes_function_call_parts() {
        let msgs = vec![assistant_with_calls("Thinking...", vec![("tc1", "search")])];
        let (_system, contents) = messages_to_gemini_format(&msgs);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "model");
        // non-empty content produces a text part first, then functionCall
        let parts = contents[0]["parts"].as_array().expect("parts array");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "Thinking...");
        assert!(parts[1].get("functionCall").is_some());
        assert_eq!(parts[1]["functionCall"]["name"], "search");
    }

    #[test]
    fn gemini_empty_messages_returns_none_and_empty_vec() {
        let (system, contents) = messages_to_gemini_format(&[]);
        assert!(system.is_none());
        assert!(contents.is_empty());
    }

    // ── messages_to_openai_format tests ──────────────────────────────────────

    #[test]
    fn openai_basic_user_and_assistant_messages() {
        let msgs = vec![user_msg("Hello"), assistant_msg("Hi!")];
        let result = messages_to_openai_format(&msgs);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["role"], "user");
        assert_eq!(result[0]["content"], "Hello");
        assert_eq!(result[1]["role"], "assistant");
        assert_eq!(result[1]["content"], "Hi!");
    }

    #[test]
    fn openai_assistant_with_tool_calls_has_tool_calls_array_and_null_content() {
        let msgs = vec![
            user_msg("go"),
            assistant_with_calls("", vec![("tc1", "search")]),
            tool_msg("tc1", "result"),
        ];
        let result = messages_to_openai_format(&msgs);
        let asst = &result[1];
        assert_eq!(asst["content"], serde_json::Value::Null);
        let tc_arr = asst["tool_calls"].as_array().expect("tool_calls array");
        assert_eq!(tc_arr.len(), 1);
        assert_eq!(tc_arr[0]["id"], "tc1");
        assert_eq!(tc_arr[0]["type"], "function");
        assert_eq!(tc_arr[0]["function"]["name"], "search");
    }

    #[test]
    fn openai_tool_message_includes_tool_call_id() {
        let msgs = vec![
            user_msg("go"),
            assistant_with_calls("", vec![("call-42", "my_tool")]),
            tool_msg("call-42", "tool output"),
        ];
        let result = messages_to_openai_format(&msgs);
        let tool = &result[2];
        assert_eq!(tool["role"], "tool");
        assert_eq!(tool["content"], "tool output");
        assert_eq!(tool["tool_call_id"], "call-42");
    }

    #[test]
    fn openai_system_message_preserved() {
        let msgs = vec![system_msg("You are an AI."), user_msg("Hi")];
        let result = messages_to_openai_format(&msgs);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["role"], "system");
        assert_eq!(result[0]["content"], "You are an AI.");
    }

    #[test]
    fn openai_assistant_with_content_and_tool_calls_preserves_content() {
        let msgs = vec![
            user_msg("go"),
            assistant_with_calls("Let me search for that.", vec![("tc1", "search")]),
            tool_msg("tc1", "found it"),
        ];
        let result = messages_to_openai_format(&msgs);
        let asst = &result[1];
        // non-empty content should be preserved (not null)
        assert_eq!(asst["content"], "Let me search for that.");
        assert!(asst.get("tool_calls").is_some());
    }
}
