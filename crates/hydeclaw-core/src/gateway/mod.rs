use axum::{
    Router,
    middleware as axum_mw,
    routing::{delete, get, patch, post, put},
};
use tower_http::services::{ServeDir, ServeFile};
use serde::{Deserialize, Serialize};

pub(crate) mod middleware;
pub mod stream_registry;
pub mod stream_jobs;
pub mod state;
mod handlers;
pub use state::*;
use middleware::*;
use handlers::*;
// Re-export for use by main.rs
pub use handlers::agents::start_agent_from_config;
pub use handlers::email_triggers::renew_expiring_gmail_watches;
pub use handlers::channels::migrate_credentials_to_vault;
pub use handlers::providers::migrate_provider_keys_to_vault;
pub(crate) use handlers::backup::create_backup_internal;
pub(crate) use handlers::notifications::notify;

/// SSE event type constants for Vercel AI SDK v3 compatibility.
mod sse_types {
    pub const DATA_SESSION_ID: &str = "data-session-id";
    pub const START: &str = "start";
    pub const TEXT_START: &str = "text-start";
    pub const TEXT_DELTA: &str = "text-delta";
    pub const TEXT_END: &str = "text-end";
    pub const TOOL_INPUT_START: &str = "tool-input-start";
    pub const TOOL_INPUT_DELTA: &str = "tool-input-delta";
    pub const TOOL_INPUT_AVAILABLE: &str = "tool-input-available";
    pub const TOOL_OUTPUT_AVAILABLE: &str = "tool-output-available";
    pub const RICH_CARD: &str = "rich-card";
    pub const FILE: &str = "file";
    pub const SYNC: &str = "sync";
    pub const FINISH: &str = "finish";
    pub const ERROR: &str = "error";
}

/// Public OpenAI-format message — used by gateway AND referenced from engine::handle_openai.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    /// Vercel AI SDK 3.x format: array of message parts (text, reasoning, tool calls)
    #[serde(default)]
    pub parts: Option<Vec<MessagePart>>,
}

/// Part of a message in Vercel AI SDK 3.x format
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessagePart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(default)]
    pub text: Option<String>,
}

pub fn router(state: AppState) -> anyhow::Result<Router> {
    let auth_token = state
        .config
        .gateway
        .auth_token_env
        .as_ref()
        .and_then(|env_name| std::env::var(env_name).ok());

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/mcp/callback", post(mcp_callback))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/v1/embeddings", post(embeddings_proxy))
        .route("/api/chat", post(api_chat_sse))
        .route("/api/chat/{id}/stream", get(api_chat_resume_stream))
        .route("/api/chat/{id}/abort", post(api_chat_abort))
        .route("/ws", get(ws_handler))
        // Channel connector WebSocket (external adapters: Telegram, Discord, etc.)
        .route("/ws/channel/{agent_name}", get(channel_ws_handler))
        // WS ticket-based auth (one-time ticket to avoid exposing static token in WS URL)
        .route("/api/auth/ws-ticket", post(api_create_ws_ticket))
        // Secrets vault CRUD
        .route("/api/secrets", get(list_secrets))
        .route("/api/secrets", post(set_secret))
        .route("/api/secrets/{name}", get(get_secret))
        .route("/api/secrets/{name}", delete(delete_secret))
        // Setup / onboarding
        .route("/api/setup/status", get(api_setup_status))
        .route("/api/setup/requirements", get(api_setup_requirements))
        .merge(
            Router::new()
                .route("/api/setup/complete", post(api_setup_complete))
                .layer(axum_mw::from_fn_with_state(state.clone(), setup_guard_middleware))
        )
        // UI monitoring endpoints
        .route("/api/status", get(api_status))
        .route("/api/agents", get(api_agents))
        .route("/api/agents", post(api_create_agent))
        .route("/api/agents/{name}", get(api_get_agent))
        .route("/api/agents/{name}", put(api_update_agent))
        .route("/api/agents/{name}", delete(api_delete_agent))
        .route("/api/agents/{name}/model-override", post(set_model_override))
        // Unified providers
        .route("/api/provider-types", get(handlers::providers::api_list_provider_types))
        .route("/api/media-drivers", get(handlers::providers::api_list_media_drivers))
        .route("/api/media-config", get(handlers::providers::api_media_config_export))
        .route("/api/providers", get(handlers::providers::api_list_providers).post(handlers::providers::api_create_provider))
        .route("/api/providers/{id}", get(handlers::providers::api_get_provider).put(handlers::providers::api_update_provider).delete(handlers::providers::api_delete_provider))
        .route("/api/providers/{id}/models", get(handlers::providers::api_unified_provider_models))
        .route("/api/providers/{id}/resolve", get(handlers::providers::api_provider_resolve))
        .route("/api/providers/{id}/test-cli", post(handlers::providers::api_test_cli))
        .route("/api/provider-active", get(handlers::providers::api_list_provider_active).put(handlers::providers::api_set_provider_active))
        .route("/api/watchdog/status", get(api_watchdog_status))
        .route("/api/watchdog/config", get(api_watchdog_config).put(api_watchdog_config_update))
        .route("/api/watchdog/settings", get(api_watchdog_settings).put(api_watchdog_settings_update))
        .route("/api/watchdog/restart/{name}", post(api_watchdog_restart_check))
        .route("/api/stats", get(api_stats))
        .route("/api/usage", get(api_usage))
        .route("/api/usage/daily", get(api_usage_daily))
        .route("/api/usage/sessions", get(api_usage_sessions))
        .route("/api/doctor", get(api_doctor))
        .route("/api/network/addresses", get(api_network_addresses))
        // Sessions & messages
        .route("/api/sessions", get(api_list_sessions))
        .route("/api/sessions", delete(api_delete_all_sessions))
        .route("/api/sessions/latest", get(api_latest_session))
        .route("/api/sessions/search", get(api_search_sessions))
        .route("/api/sessions/{id}", delete(api_delete_session).patch(api_patch_session))
        .route("/api/sessions/{id}/compact", post(api_compact_session))
        .route("/api/sessions/{id}/export", get(api_export_session))
        .route("/api/sessions/{id}/invite", post(handlers::sessions::api_invite_to_session))
        .route("/api/sessions/{id}/documents", post(handlers::sessions::api_session_upload_document))
        .route("/api/sessions/{id}/messages", get(api_session_messages))
        .route("/api/messages/{id}", delete(api_delete_message).patch(api_patch_message))
        .route("/api/messages/{id}/feedback", post(api_message_feedback))
        // Audit
        .route("/api/audit", get(api_audit_events))
        .route("/api/audit/tools", get(api_tool_audit))
        // Tasks (multi-step execution pipeline)
        .route("/api/tasks", get(api_list_tasks).post(api_create_task_endpoint))
        .route("/api/tasks/audit", get(api_task_audit))
        .route("/api/tasks/{id}", get(api_get_task).delete(api_delete_task))
        .route("/api/tasks/{id}/steps", get(api_task_steps))
        // Approvals
        .route("/api/approvals", get(api_list_approvals))
        .route("/api/approvals/{id}/resolve", post(api_resolve_approval))
        .route("/api/approvals/allowlist", get(api_list_allowlist).post(api_add_to_allowlist))
        .route("/api/approvals/allowlist/{id}", delete(api_delete_from_allowlist))
        // Notifications
        // Note: /read-all must be registered BEFORE /{id} to prevent "read-all" matching as UUID path param
        .route("/api/notifications", get(api_list_notifications))
        .route("/api/notifications/read-all", post(api_mark_all_notifications_read))
        .route("/api/notifications/clear", delete(api_clear_all_notifications))
        .route("/api/notifications/{id}", patch(api_mark_notification_read))
        // Cron jobs CRUD
        .route("/api/cron", get(api_list_cron))
        .route("/api/cron", post(api_create_cron))
        .route("/api/cron/{id}", put(api_update_cron))
        .route("/api/cron/{id}", delete(api_delete_cron))
        .route("/api/cron/{id}/run", post(api_run_cron))
        .route("/api/cron/{id}/runs", get(api_cron_runs))
        .route("/api/cron/runs", get(api_cron_runs_all))
        // Memory
        .route("/api/memory", get(api_list_memory).post(api_create_memory))
        .route("/api/memory/stats", get(api_memory_stats))
        .route("/api/memory/graph", get(api_memory_graph))
        .route("/api/memory/export", get(api_export_memory))
        .route("/api/memory/fts-language", get(api_get_fts_language).put(api_set_fts_language))
        .route("/api/memory/{id}", delete(api_delete_memory))
        .route("/api/memory/{id}", patch(api_patch_memory))
        // Memory documents (document-level view)
        .route("/api/memory/tasks", get(api_memory_tasks))
        .route("/api/memory/extraction-queue", get(api_extraction_queue))
        .route("/api/memory/documents", get(api_list_documents))
        .route("/api/memory/documents/{id}", get(api_get_document).patch(api_patch_document).delete(api_delete_memory))
        // Tools & MCP
        .route("/api/tool-definitions", get(api_tool_definitions))
        .route("/api/tools", get(api_list_tools).post(api_tool_service_create))
        .route("/api/tools/{name}", put(api_tool_service_update).delete(api_tool_service_delete))
        .route("/api/mcp", get(api_list_mcp).post(api_mcp_create))
        .route("/api/mcp/{name}", put(api_mcp_update).delete(api_mcp_delete))
        .route("/api/mcp/{name}/reload", post(api_mcp_reload))
        .route("/api/mcp/{name}/toggle", post(api_mcp_toggle))
        // YAML tool management (global shared)
        .route("/api/yaml-tools", get(api_yaml_tools_list_global).post(api_yaml_tool_create_global))
        .route("/api/yaml-tools/{tool}/verify", post(api_yaml_tool_verify_global))
        .route("/api/yaml-tools/{tool}/disable", post(api_yaml_tool_disable_global))
        .route("/api/yaml-tools/{tool}/enable", post(api_yaml_tool_enable_global))
        .route("/api/yaml-tools/{tool}", get(api_yaml_tool_get_global).put(api_yaml_tool_update_global).delete(api_yaml_tool_delete_global))
        // YAML tool management (per-agent, kept for compat)
        .route("/api/agents/{name}/yaml-tools", get(api_yaml_tools_list))
        .route("/api/agents/{name}/yaml-tools/{tool}/verify", post(api_yaml_tool_verify))
        .route("/api/agents/{name}/yaml-tools/{tool}/disable", post(api_yaml_tool_disable))
        // Skills management (global — skills are shared across all agents)
        .route("/api/skills", get(api_skills_list_global))
        .route("/api/skills/{skill}", get(api_skill_get_global).put(api_skill_upsert_global).delete(api_skill_delete_global))
        // Skills management (per-agent, kept for compat)
        .route("/api/agents/{name}/skills", get(api_skills_list))
        .route("/api/agents/{name}/skills/{skill}", get(api_skill_get).put(api_skill_upsert).delete(api_skill_delete))
        // Workspace file browser (flat: workspace/ is root, agents are subdirs)
        .route("/api/workspace", get(api_workspace_browse))
        .route("/api/workspace/{*path}", get(api_workspace_browse))
        .route("/api/workspace/{*path}", put(api_workspace_write))
        .route("/api/workspace/{*path}", delete(api_workspace_delete))
        // TTS
        .route("/api/tts/voices", get(api_tts_voices))
        .route("/api/tts/synthesize", post(api_tts_synthesize))
        // Config
        .route("/api/config/schema", get(api_get_config_schema))
        .route("/api/config", get(api_get_config))
        .route("/api/config", put(api_update_config))
        .route("/api/config/export", get(api_export_config))
        .route("/api/config/import", post(api_import_config))
        // Backup & restore
        .route("/api/backup", post(api_create_backup).get(api_list_backups))
        .route("/api/backup/{filename}", get(api_download_backup).delete(api_delete_backup))
        .merge(
            Router::new()
                .route("/api/restore", post(api_restore))
                .layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)) // 100 MB — backups can be large
        )
        // Restart core process (systemd will restart it)
        .route("/api/restart", post(api_restart))
        // Docker service management (rebuild/restart whitelisted services)
        .route("/api/services", get(api_list_services))
        .route("/api/services/{name}/{action}", post(api_service_action))
        .route("/api/containers/{name}/restart", post(api_container_restart))
        // Access control (pairing + allowlist)
        .route("/api/access/{agent}/pending", get(api_access_pending))
        .route("/api/access/{agent}/approve/{code}", post(api_access_approve))
        .route("/api/access/{agent}/reject/{code}", post(api_access_reject))
        .route("/api/access/{agent}/users", get(api_access_list_users))
        .route("/api/access/{agent}/users/{user_id}", delete(api_access_remove_user))
        // Webhooks — CRUD + POST /webhook/{name} dispatches payload to configured agent
        .route("/api/webhooks", get(api_list_webhooks).post(api_create_webhook))
        .route("/api/webhooks/{id}", put(api_update_webhook).delete(api_delete_webhook))
        .route("/api/webhooks/{id}/regenerate-secret", post(api_regenerate_webhook_secret))
        .route("/webhook/{name}", post(webhook_handler))
        // Channel management (per-agent + global)
        .route("/api/channels", get(api_list_all_channels))
        .route("/api/channels/active", get(api_channels_active))
        .route("/api/channels/notify", post(api_channel_notify))
        .route("/api/agents/{name}/hooks", get(api_agent_hooks))
        .route("/api/agents/{name}/channels", get(api_channels_list).post(api_channel_create))
        .route("/api/agents/{name}/channels/{id}", delete(api_channel_delete).put(api_channel_update))
        .route("/api/agents/{name}/channels/{id}/restart", post(api_channel_restart))
        .route("/api/agents/{name}/channels/{id}/ack", post(api_channel_ack))
        .route("/api/agents/{name}/channels/{id}/status", get(api_channel_status))
        // Canvas state (current content for a given agent)
        .route("/api/canvas/{agent}", get(api_canvas_state).delete(api_canvas_clear))
        // Media upload (channel adapters download media → upload here for stable URLs)
        .route("/uploads/{filename}", get(api_media_serve))
        .merge(
            Router::new()
                .route("/api/media/upload", post(api_media_upload))
                .layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024)) // 20 MB — only for upload
        )
        // OAuth 2.0 — callback is public (exempted in auth_middleware)
        .route("/api/oauth/callback", get(api_oauth_callback))
        // OAuth accounts CRUD (must come BEFORE any /api/oauth/{provider} to avoid path conflicts)
        .route("/api/oauth/accounts", get(api_oauth_accounts_list).post(api_oauth_account_create))
        .route("/api/oauth/accounts/{id}", delete(api_oauth_account_delete))
        .route("/api/oauth/accounts/{id}/connect", post(api_oauth_account_connect))
        .route("/api/oauth/accounts/{id}/revoke", post(api_oauth_account_revoke))
        // OAuth — backward compat
        .route("/api/oauth/providers", get(api_oauth_providers))
        // Agent OAuth bindings
        .route("/api/agents/{name}/oauth/bindings", get(api_oauth_bindings_list).post(api_oauth_binding_create))
        .route("/api/agents/{name}/oauth/bindings/{provider}", delete(api_oauth_binding_delete))
        // Gmail Pub/Sub triggers — push is public (Google calls without our Bearer token)
        // Note: /push must be registered BEFORE /{id} to avoid "push" matching as UUID path param
        .route("/api/triggers/email/push", post(gmail_push_handler))
        // Gmail trigger CRUD — authenticated
        .route("/api/triggers/email", get(api_list_gmail_triggers).post(api_create_gmail_trigger))
        .route("/api/triggers/email/{id}", delete(api_delete_gmail_trigger))
        // GitHub repo allowlist (per-agent)
        .route("/api/agents/{name}/github/repos", get(handlers::github_repos::api_list_github_repos).post(handlers::github_repos::api_add_github_repo))
        .route("/api/agents/{name}/github/repos/{id}", delete(handlers::github_repos::api_delete_github_repo));

    // Auth middleware — REQUIRED. Refuse to start without a token.
    let app = if let Some(token) = auth_token {
        let shared_token: &'static str = Box::leak(token.into_boxed_str());
        let rate_limiter: &'static AuthRateLimiter = Box::leak(Box::new(AuthRateLimiter::new(50, 60)));
        let ws_tickets = state.ws_tickets.clone();
        app.layer(axum_mw::from_fn(move |req, next| {
            let ws_tickets = ws_tickets.clone();
            async move { auth_middleware(req, next, shared_token, rate_limiter, ws_tickets).await }
        }))
    } else {
        tracing::error!("FATAL: no auth token configured — refusing to start unauthenticated gateway");
        tracing::error!("set gateway.auth_token_env in config and provide the env var");
        anyhow::bail!("gateway requires auth token — set gateway.auth_token_env in hydeclaw.toml");
    };

    // Request rate limiting (per-IP, from config limits.max_requests_per_minute)
    let max_rpm = state.config.limits.max_requests_per_minute;
    let req_limiter: &'static RequestRateLimiter =
        Box::leak(Box::new(RequestRateLimiter::new(max_rpm)));
    let ws_budget: &'static WsConnectionBudget =
        Box::leak(Box::new(WsConnectionBudget::new(32)));
    let app = app.layer(axum_mw::from_fn(move |req, next| {
        request_rate_limit_middleware(req, next, req_limiter, ws_budget)
    }));

    // CORS: restrict to configured origins or derive from listen address.
    let cors_origins: Vec<axum::http::HeaderValue> = if state.config.gateway.cors_origins.is_empty() {
        // Derive from listen address: allow UI on same host (:5173) + API port
        let host = state.config.gateway.listen.split(':').next().unwrap_or("0.0.0.0");
        let port = state.config.gateway.listen.rsplit(':').next().unwrap_or("18789");
        let mut origins = vec![
            format!("http://{}:{}", host, port).parse().expect("valid CORS origin"),
            format!("http://{}:5173", host).parse().expect("valid CORS origin"),
        ];
        // For 0.0.0.0: also allow localhost + all local network interfaces
        if host == "0.0.0.0" {
            origins.push("http://localhost:5173".parse().expect("valid CORS origin"));
            origins.push(format!("http://localhost:{}", port).parse().expect("valid CORS origin"));
            // Add all non-loopback IPv4 addresses (for LAN access)
            for iface in get_local_ipv4_addrs() {
                if let Ok(v) = format!("http://{}:{}", iface, port).parse() { origins.push(v); }
                if let Ok(v) = format!("http://{}:5173", iface).parse() { origins.push(v); }
            }
        }
        // Also add public_url origin if configured
        if let Some(ref pu) = state.config.gateway.public_url
            && let Ok(v) = pu.trim_end_matches('/').parse() { origins.push(v); }
        origins
    } else {
        state.config.gateway.cors_origins.iter()
            .filter_map(|o| o.parse().ok())
            .collect()
    };
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(cors_origins)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
        ]);
    // Serve static UI files from ui/out/ with SPA fallback to index.html.
    // API routes have priority (registered above); unmatched paths serve static files.
    // _next/ assets served WITHOUT fallback (404 if missing — prevents stale cache getting HTML).
    // All other paths fall back to index.html for SPA routing.
    let ui_dir = std::path::Path::new("ui/out");
    let app = if ui_dir.is_dir() {
        // _next/ assets are content-hashed → cache forever (immutable)
        let next_service = ServeDir::new(ui_dir.join("_next"));
        let app = app.nest_service(
            "/_next",
            tower_http::set_header::SetResponseHeader::overriding(
                next_service,
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
        );
        // HTML/other files → always revalidate
        let serve = tower_http::set_header::SetResponseHeader::overriding(
            ServeDir::new(ui_dir).fallback(ServeFile::new(ui_dir.join("index.html"))),
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        );
        app.fallback_service(serve)
    } else {
        app
    };

    let app = app.layer(cors);

    // Security headers: prevent MIME sniffing, clickjacking, XSS reflection
    let app = app.layer(axum_mw::from_fn(|req: axum::http::Request<axum::body::Body>, next: axum_mw::Next| async move {
        let mut response = next.run(req).await;
        let headers = response.headers_mut();
        headers.insert("X-Content-Type-Options", "nosniff".parse().expect("valid header value"));
        headers.insert("X-Frame-Options", "DENY".parse().expect("valid header value"));
        headers.insert("X-XSS-Protection", "1; mode=block".parse().expect("valid header value"));
        headers.insert("Referrer-Policy", "strict-origin-when-cross-origin".parse().expect("valid header value"));
        response
    }));

    Ok(app.with_state(state))
}

/// Get the primary non-loopback IPv4 address of the host (for CORS auto-derivation).
fn get_local_ipv4_addrs() -> Vec<String> {
    // UDP connect trick: connect to external IP (no actual traffic sent),
    // then read local_addr to get the outbound interface IP.
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0")
        && sock.connect("8.8.8.8:80").is_ok()
            && let Ok(local) = sock.local_addr()
                && !local.ip().is_loopback() {
                    return vec![local.ip().to_string()];
                }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::handlers::agents::{validate_agent_name, agent_config_path};
    use super::handlers::secrets::mask_secret_value;
    use super::handlers::workspace::format_workspace_size;
    use super::handlers::skills::skill_safe_name;

    // ── mask_secret_value ────────────────────────────────────────────────────

    #[test]
    fn mask_empty_string() {
        assert_eq!(mask_secret_value(""), "");
    }

    #[test]
    fn mask_short_3_chars() {
        assert_eq!(mask_secret_value("abc"), "***");
    }

    #[test]
    fn mask_exactly_8_chars() {
        assert_eq!(mask_secret_value("12345678"), "********");
    }

    #[test]
    fn mask_9_chars() {
        assert_eq!(mask_secret_value("123456789"), "1234...6789");
    }

    #[test]
    fn mask_12_chars() {
        assert_eq!(mask_secret_value("abcdefghijkl"), "abcd...ijkl");
    }

    // ── validate_agent_name ──────────────────────────────────────────────────

    #[test]
    fn validate_agent_name_valid_compound() {
        assert!(validate_agent_name("my-agent_1").is_ok());
    }

    #[test]
    fn validate_agent_name_single_char() {
        assert!(validate_agent_name("a").is_ok());
    }

    #[test]
    fn validate_agent_name_empty() {
        assert!(validate_agent_name("").is_err());
    }

    #[test]
    fn validate_agent_name_too_long() {
        let name = "a".repeat(33);
        assert!(validate_agent_name(&name).is_err());
    }

    #[test]
    fn validate_agent_name_special_chars() {
        assert!(validate_agent_name("my agent!").is_err());
    }

    #[test]
    fn validate_agent_name_dash_underscore() {
        assert!(validate_agent_name("my_agent-1").is_ok());
    }

    #[test]
    fn validate_agent_name_exactly_32_chars() {
        let name = "a".repeat(32);
        assert!(validate_agent_name(&name).is_ok());
    }

    // ── agent_config_path ────────────────────────────────────────────────────

    #[test]
    fn agent_config_path_main() {
        let path = agent_config_path("main");
        assert_eq!(path, std::path::Path::new("config/agents/main.toml"));
    }

    // ── format_workspace_size ────────────────────────────────────────────────

    #[test]
    fn format_workspace_size_zero() {
        assert_eq!(format_workspace_size(0), "0 B");
    }

    #[test]
    fn format_workspace_size_bytes() {
        assert_eq!(format_workspace_size(500), "500 B");
    }

    #[test]
    fn format_workspace_size_exactly_1_kb() {
        assert_eq!(format_workspace_size(1024), "1.0 KB");
    }

    #[test]
    fn format_workspace_size_1_5_kb() {
        assert_eq!(format_workspace_size(1536), "1.5 KB");
    }

    #[test]
    fn format_workspace_size_exactly_1_mb() {
        assert_eq!(format_workspace_size(1_048_576), "1.0 MB");
    }

    // ── skill_safe_name ──────────────────────────────────────────────────────

    #[test]
    fn skill_safe_name_unchanged() {
        assert_eq!(skill_safe_name("simple-name"), "simple-name");
    }

    #[test]
    fn skill_safe_name_slashes() {
        assert_eq!(skill_safe_name("path/to\\file"), "path-to-file");
    }

    #[test]
    fn skill_safe_name_spaces() {
        assert_eq!(skill_safe_name("name with spaces"), "name-with-spaces");
    }

    #[test]
    fn skill_safe_name_all_special_chars() {
        // : * ? " < > | all replaced with -
        assert_eq!(
            skill_safe_name("file:name*bad?\"<>|"),
            "file-name-bad-----"
        );
    }
}
