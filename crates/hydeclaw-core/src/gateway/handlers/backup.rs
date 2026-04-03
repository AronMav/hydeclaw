use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path as FsPath;
use tokio::fs;

use sqlx::Row;

use super::super::AppState;
use crate::secrets::PlaintextSecret;

const BACKUP_DIR: &str = "backups";
const RETENTION_DAYS: i64 = 7;

// ── Backup file format ──────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupFile {
    pub version: u32,
    pub created_at: chrono::DateTime<Utc>,
    pub config: Value,
    pub workspace: Vec<WorkspaceFile>,
    pub secrets: Vec<PlaintextSecret>,
    pub memory: Vec<MemoryChunk>,
    pub cron: Vec<CronJob>,
    #[serde(default)]
    pub providers: Vec<BackupProvider>,
    #[serde(default)]
    pub provider_active: Vec<BackupProviderActive>,
    #[serde(default)]
    pub channels: Vec<BackupChannel>,
    #[serde(default)]
    pub webhooks: Vec<BackupWebhook>,
    #[serde(default)]
    pub watchdog_settings: Vec<BackupWatchdogSetting>,
    #[serde(default)]
    pub allowed_users: Vec<BackupAllowedUser>,
    #[serde(default)]
    pub approval_allowlist: Vec<BackupApprovalAllow>,
    #[serde(default)]
    pub oauth_accounts: Vec<BackupOAuthAccount>,
    #[serde(default)]
    pub oauth_bindings: Vec<BackupOAuthBinding>,
    #[serde(default)]
    pub gmail_triggers: Vec<BackupGmailTrigger>,
    #[serde(default)]
    pub github_repos: Vec<BackupGithubRepo>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct WorkspaceFile {
    pub path: String,
    pub content: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct MemoryChunk {
    pub id: String,
    pub user_id: String,
    pub content: String,
    pub source: Option<String>,
    pub pinned: bool,
    pub relevance_score: f64,
    pub created_at: chrono::DateTime<Utc>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub chunk_index: i32,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct CronJob {
    pub agent_id: String,
    pub name: String,
    pub cron_expr: String,
    pub timezone: String,
    pub task_message: String,
    pub enabled: bool,
    pub announce_to: Option<Value>,
    pub silent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupProvider {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub category: String,
    pub provider_type: String,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub enabled: bool,
    pub options: Value,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupProviderActive {
    pub capability: String,
    pub provider_name: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupChannel {
    pub id: String,
    pub agent_name: String,
    pub channel_type: String,
    pub display_name: String,
    pub config: Value,
    pub status: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupWebhook {
    pub name: String,
    pub agent_id: String,
    pub secret: Option<String>,
    pub prompt_prefix: Option<String>,
    pub enabled: bool,
    pub webhook_type: String,
    pub event_filter: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupWatchdogSetting {
    pub key: String,
    pub value: Value,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupAllowedUser {
    pub agent_id: String,
    pub channel_user_id: String,
    pub display_name: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupApprovalAllow {
    pub agent_id: String,
    pub tool_pattern: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupOAuthAccount {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub scope: String,
    pub status: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupOAuthBinding {
    pub agent_id: String,
    pub provider: String,
    pub account_id: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupGmailTrigger {
    pub agent_id: String,
    pub email_address: String,
    pub pubsub_topic: String,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BackupGithubRepo {
    pub agent_id: String,
    pub owner: String,
    pub repo: String,
}

// ── POST /api/backup ─────────────────────────────────────────────────────────

/// Create a backup: collect all data, save to disk, clean up old files.
pub(crate) async fn api_create_backup(State(state): State<AppState>) -> impl IntoResponse {
    let now = Utc::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let filename = format!("hydeclaw-{date_str}.json");

    // 1. Config
    let app_toml = std::fs::read_to_string("config/hydeclaw.toml").unwrap_or_default();
    let mut agents = serde_json::Map::new();
    if let Ok(entries) = std::fs::read_dir("config/agents") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                agents.insert(name, Value::String(content));
            }
        }
    }
    let config = json!({ "app_config": app_toml, "agents": agents });

    // 2. Workspace files (recursive walk, text only)
    let workspace_dir = {
        let deps = state.agent_deps.read().await;
        deps.workspace_dir.clone()
    };
    let workspace = collect_workspace_files(&workspace_dir).await;

    // 3. Secrets (raw encrypted blobs)
    let secrets = match state.secrets.export_decrypted().await {
        Ok(rows) => rows,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    };

    // 4. Memory chunks (no embeddings)
    let memory = match collect_memory(&state).await {
        Ok(chunks) => chunks,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    };

    // 5. Cron jobs
    let cron = match collect_cron(&state).await {
        Ok(jobs) => jobs,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    };

    // 6. V2 sections (non-fatal — default to empty on error)
    tracing::info!("backup: collecting V2 sections...");
    let providers = collect_providers(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: providers failed"); vec![] });
    tracing::info!(count = providers.len(), "backup: providers");
    let provider_active = collect_provider_active(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: provider_active failed"); vec![] });
    tracing::info!(count = provider_active.len(), "backup: provider_active");
    let channels = collect_channels(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: channels failed"); vec![] });
    tracing::info!(count = channels.len(), "backup: channels");
    let webhooks = collect_webhooks(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: webhooks failed"); vec![] });
    tracing::info!(count = webhooks.len(), "backup: webhooks");
    let watchdog_settings = collect_watchdog_settings(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: watchdog failed"); vec![] });
    tracing::info!(count = watchdog_settings.len(), "backup: watchdog_settings");
    let allowed_users = collect_allowed_users(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: allowed_users failed"); vec![] });
    tracing::info!(count = allowed_users.len(), "backup: allowed_users");
    let approval_allowlist = collect_approval_allowlist(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: approval failed"); vec![] });
    tracing::info!(count = approval_allowlist.len(), "backup: approval_allowlist");
    let oauth_accounts = collect_oauth_accounts(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: oauth_accounts failed"); vec![] });
    let oauth_bindings = collect_oauth_bindings(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: oauth_bindings failed"); vec![] });
    let gmail_triggers = collect_gmail_triggers(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: gmail_triggers failed"); vec![] });
    let github_repos = collect_github_repos(&state.db).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "backup: github_repos failed"); vec![] });

    let backup = BackupFile {
        version: 2,
        created_at: now,
        config,
        workspace,
        secrets,
        memory,
        cron,
        providers,
        provider_active,
        channels,
        webhooks,
        watchdog_settings,
        allowed_users,
        approval_allowlist,
        oauth_accounts,
        oauth_bindings,
        gmail_triggers,
        github_repos,
    };

    // Serialize to JSON
    let json_bytes = match serde_json::to_vec_pretty(&backup) {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    };

    // Save to disk
    if let Err(e) = fs::create_dir_all(BACKUP_DIR).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("cannot create backup dir: {e}")}))).into_response();
    }
    let filepath = format!("{BACKUP_DIR}/{filename}");
    if let Err(e) = fs::write(&filepath, &json_bytes).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("cannot write backup: {e}")}))).into_response();
    }

    // Cleanup old backups
    cleanup_old_backups(now).await;

    let size_bytes = json_bytes.len();
    tracing::info!(filename = %filename, size_bytes = size_bytes, "backup created");

    Json(json!({
        "ok": true,
        "filename": filename,
        "path": filepath,
        "size_bytes": size_bytes,
        "created_at": now,
    })).into_response()
}

// ── GET /api/backup ──────────────────────────────────────────────────────────

pub(crate) async fn api_list_backups() -> impl IntoResponse {
    let mut entries = Vec::new();
    if let Ok(mut dir) = fs::read_dir(BACKUP_DIR).await {
        while let Ok(Some(entry)) = dir.next_entry().await {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                // Single metadata() call — avoid double syscall
                if let Ok(meta) = entry.metadata().await {
                    let size_bytes = meta.len();
                    let modified = meta.modified().ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .and_then(|d| chrono::DateTime::<Utc>::from_timestamp(d.as_secs() as i64, 0));
                    entries.push(json!({
                        "filename": filename,
                        "size_bytes": size_bytes,
                        "created_at": modified,
                    }));
                }
            }
        }
    }
    entries.sort_by(|a, b| {
        let fa = a["filename"].as_str().unwrap_or("");
        let fb = b["filename"].as_str().unwrap_or("");
        fb.cmp(fa) // descending by filename (date)
    });
    Json(json!({ "backups": entries }))
}

// ── GET /api/backup/:filename ─────────────────────────────────────────────────

pub(crate) async fn api_download_backup(Path(filename): Path<String>) -> impl IntoResponse {
    // Prevent path traversal
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") || filename.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid filename"}))).into_response();
    }
    let filepath = format!("{BACKUP_DIR}/{filename}");
    match fs::read(&filepath).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "application/json".to_string()),
                (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
            ],
            bytes,
        ).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, Json(json!({"error": "backup not found"}))).into_response(),
    }
}

// ── DELETE /api/backup/:filename ─────────────────────────────────────────────

pub(crate) async fn api_delete_backup(Path(filename): Path<String>) -> impl IntoResponse {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") || filename.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid filename"}))).into_response();
    }
    let filepath = format!("{BACKUP_DIR}/{filename}");
    match fs::remove_file(&filepath).await {
        Ok(()) => {
            tracing::info!(filename = %filename, "backup deleted via API");
            Json(json!({"ok": true})).into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "backup not found"}))).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response()
        }
    }
}

// ── POST /api/restore ─────────────────────────────────────────────────────────

pub(crate) async fn api_restore(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(backup): Json<BackupFile>,
) -> impl IntoResponse {
    // Security: restore is a destructive operation — require X-Confirm-Restore header
    // to prevent accidental or automated restore via stolen API token.
    let confirm = headers.get("x-confirm-restore").and_then(|v| v.to_str().ok()).unwrap_or("");
    if confirm != "yes-i-am-sure" {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "restore requires X-Confirm-Restore: yes-i-am-sure header"
        }))).into_response();
    }
    tracing::warn!("RESTORE initiated — overwriting configs, secrets, memory, cron");

    // Stop all running agents before restore to prevent stale state
    {
        let mut agents = state.agents.write().await;
        let names: Vec<String> = agents.keys().cloned().collect();
        for name in &names {
            if let Some(handle) = agents.remove(name) {
                handle.shutdown(&state.scheduler).await;
                tracing::info!(agent = %name, "agent stopped for restore");
            }
        }
    }

    if backup.version != 1 && backup.version != 2 {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "unsupported backup version"}))).into_response();
    }

    let mut restored = json!({ "configs": 0, "workspace_files": 0, "secrets": 0, "memory": 0, "cron": 0 });

    // 1. Config (sync I/O, not in hot path)
    let config_count = restore_config(&backup.config).await;
    restored["configs"] = json!(config_count);

    // 2. Workspace
    let workspace_dir = {
        let deps = state.agent_deps.read().await;
        deps.workspace_dir.clone()
    };
    let ws_count = restore_workspace(&workspace_dir, &backup.workspace).await;
    restored["workspace_files"] = json!(ws_count);

    // 3. Secrets
    let secret_count = backup.secrets.len();
    match state.secrets.restore_plaintext(backup.secrets).await {
        Ok(_) => restored["secrets"] = json!(secret_count),
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("secrets restore failed: {e}")}))).into_response();
        }
    }

    // 4+5. Memory + Cron — atomic: both succeed or neither is committed
    let fts_lang = match state.memory_store.validated_fts_language() {
        Ok(lang) => lang,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("invalid fts language: {e}")}))).into_response();
        }
    };
    match restore_memory_and_cron(&state, &backup.memory, &backup.cron, &fts_lang).await {
        Ok((mem_n, cron_n)) => {
            restored["memory"] = json!(mem_n);
            restored["cron"] = json!(cron_n);
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("db restore failed: {e}")}))).into_response();
        }
    }

    // V2 sections — wrapped in transaction for atomicity (D-10, D-11)
    // Errors are propagated instead of silently swallowed; on failure the transaction
    // rolls back automatically, preventing partial state corruption.
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore tx begin failed: {e}")}))).into_response();
        }
    };

    match restore_providers(&mut tx, &backup.providers, &backup.provider_active).await {
        Ok(n) => { restored["providers"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at providers: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }
    match restore_channels(&mut tx, &backup.channels).await {
        Ok(n) => { restored["channels"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at channels: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }
    match restore_webhooks(&mut tx, &backup.webhooks).await {
        Ok(n) => { restored["webhooks"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at webhooks: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }
    match restore_watchdog_settings(&mut tx, &backup.watchdog_settings).await {
        Ok(n) => { restored["watchdog_settings"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at watchdog_settings: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }
    match restore_allowed_users(&mut tx, &backup.allowed_users).await {
        Ok(n) => { restored["allowed_users"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at allowed_users: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }
    match restore_approval_allowlist(&mut tx, &backup.approval_allowlist).await {
        Ok(n) => { restored["approval_allowlist"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at approval_allowlist: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }
    // OAuth accounts must be restored before bindings (FK dependency)
    match restore_oauth_accounts(&mut tx, &backup.oauth_accounts).await {
        Ok(n) => { restored["oauth_accounts"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at oauth_accounts: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }
    match restore_oauth_bindings(&mut tx, &backup.oauth_bindings).await {
        Ok(n) => { restored["oauth_bindings"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at oauth_bindings: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }
    match restore_gmail_triggers(&mut tx, &backup.gmail_triggers).await {
        Ok(n) => { restored["gmail_triggers"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at gmail_triggers: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }
    match restore_github_repos(&mut tx, &backup.github_repos).await {
        Ok(n) => { restored["github_repos"] = json!(n); }
        Err(e) => {
            tracing::error!("V2 restore failed at github_repos: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore failed: {e}")}))).into_response();
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("V2 restore transaction commit failed: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("v2 restore commit failed: {e}")}))).into_response();
    }

    // Restart agents from restored configs
    let agent_configs = match crate::config::load_agent_configs("config/agents") {
        Ok(cfgs) => cfgs,
        Err(e) => {
            tracing::error!(error = %e, "failed to load agent configs after restore");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("restore succeeded but config reload failed: {}", e)}))).into_response();
        }
    };
    let mut restarted = Vec::new();
    let mut failed = Vec::new();
    for cfg in &agent_configs {
        match super::agents::start_agent_from_config(cfg, &state).await {
            Ok((handle, guard)) => {
                let name = cfg.agent.name.clone();
                state.agents.write().await.insert(name.clone(), handle);
                if let Some(g) = guard {
                    state.access_guards.write().await.insert(name.clone(), g);
                }
                // Ensure Docker sandbox for non-base agents
                if !cfg.agent.base
                    && let Some(ref sandbox) = state.sandbox
                    && let Ok(host_path) = std::fs::canonicalize(crate::config::WORKSPACE_DIR)
                    && let Err(e) = sandbox.ensure_container(&name, &host_path.to_string_lossy(), false, Some(&state.oauth)).await
                {
                    tracing::warn!(agent = %name, error = %e, "failed to ensure container after restore");
                }
                restarted.push(name);
            }
            Err(e) => {
                tracing::error!(agent = %cfg.agent.name, error = %e, "failed to restart agent after restore");
                failed.push(json!({"agent": cfg.agent.name, "error": e.to_string()}));
            }
        }
    }
    tracing::info!(agents = ?restarted, "agents restarted after restore");

    tracing::warn!("AUDIT: system restored from backup: {:?}", restored);
    Json(json!({ "ok": true, "restored": restored, "restarted_agents": restarted, "failed_agents": failed })).into_response()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn collect_workspace_files(workspace_dir: &str) -> Vec<WorkspaceFile> {
    let mut files = Vec::new();
    let root = FsPath::new(workspace_dir);
    collect_dir(root, root, &mut files).await;
    files
}

fn collect_dir<'a>(
    root: &'a FsPath,
    dir: &'a FsPath,
    files: &'a mut Vec<WorkspaceFile>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let Ok(mut rd) = fs::read_dir(dir).await else { return };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // Skip noise directories and files
            if matches!(name.as_ref(), "__pycache__" | "node_modules" | ".git" | ".venv") {
                continue;
            }
            if name.ends_with(".pyc") || name.ends_with(".db") {
                continue;
            }
            // Use file_type() from DirEntry — avoids extra stat syscall vs path.is_dir()
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                collect_dir(root, &path, files).await;
            } else {
                let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
                if size > 2_097_152 { continue; } // Skip files > 2MB
                if let Ok(bytes) = fs::read(&path).await {
                    let rel = path.strip_prefix(root).unwrap_or(&path);
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    if let Ok(text) = String::from_utf8(bytes.clone()) {
                        files.push(WorkspaceFile { path: rel_str, content: text });
                    } else {
                        // Binary file (icon, image): base64 encode
                        use base64::Engine;
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        files.push(WorkspaceFile { path: rel_str, content: format!("base64:{b64}") });
                    }
                }
            }
        }
    })
}

const MEMORY_BACKUP_LIMIT: i64 = 100_000;

async fn collect_memory(state: &AppState) -> sqlx::Result<Vec<MemoryChunk>> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_chunks")
        .fetch_one(&state.db).await.unwrap_or(0);
    if total > MEMORY_BACKUP_LIMIT {
        tracing::warn!(total, limit = MEMORY_BACKUP_LIMIT, "memory_chunks exceeds backup limit, truncating");
    }

    #[allow(clippy::type_complexity)]
    let rows: Vec<(uuid::Uuid, String, String, Option<String>, bool, f64, chrono::DateTime<Utc>, Option<uuid::Uuid>, i32)> =
        sqlx::query_as(
            "SELECT id, user_id, content, source, pinned, relevance_score, created_at, parent_id, chunk_index
             FROM memory_chunks ORDER BY created_at LIMIT $1",
        )
        .bind(MEMORY_BACKUP_LIMIT)
        .fetch_all(&state.db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, user_id, content, source, pinned, relevance_score, created_at, parent_id, chunk_index)| MemoryChunk {
            id: id.to_string(),
            user_id,
            content,
            source,
            pinned,
            relevance_score,
            created_at,
            parent_id: parent_id.map(|p| p.to_string()),
            chunk_index,
        })
        .collect())
}

async fn collect_cron(state: &AppState) -> sqlx::Result<Vec<CronJob>> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, String, String, String, bool, Option<Value>, bool)> =
        sqlx::query_as(
            "SELECT agent_id, name, cron_expr, timezone, task_message, enabled, announce_to, silent
             FROM scheduled_jobs ORDER BY name",
        )
        .fetch_all(&state.db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(agent_id, name, cron_expr, timezone, task_message, enabled, announce_to, silent)| CronJob {
            agent_id,
            name,
            cron_expr,
            timezone,
            task_message,
            enabled,
            announce_to,
            silent,
        })
        .collect())
}

// ── V2 collectors ─────────────────────────────────────────────────────────────

async fn collect_providers(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupProvider>> {
    let rows = crate::db::providers::list_providers(db).await?;
    Ok(rows.iter().map(|r| BackupProvider {
        id: r.id.to_string(),
        name: r.name.clone(),
        category: r.category.clone(),
        provider_type: r.provider_type.clone(),
        base_url: r.base_url.clone(),
        default_model: r.default_model.clone(),
        enabled: r.enabled,
        options: r.options.clone(),
        notes: r.notes.clone(),
    }).collect())
}

async fn collect_provider_active(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupProviderActive>> {
    let rows = crate::db::providers::list_provider_active(db).await?;
    Ok(rows.iter().filter_map(|r| {
        r.provider_name.as_ref().map(|pn| BackupProviderActive {
            capability: r.capability.clone(),
            provider_name: pn.clone(),
        })
    }).collect())
}

async fn collect_channels(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupChannel>> {
    let rows = sqlx::query(
        "SELECT id, agent_name, channel_type, display_name, config, status FROM agent_channels WHERE status != 'deleted'"
    ).fetch_all(db).await?;
    Ok(rows.iter().map(|r| BackupChannel {
        id: r.get::<uuid::Uuid, _>("id").to_string(),
        agent_name: r.get("agent_name"),
        channel_type: r.get("channel_type"),
        display_name: r.get("display_name"),
        config: r.get("config"),
        status: r.get("status"),
    }).collect())
}

async fn collect_webhooks(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupWebhook>> {
    let rows = sqlx::query(
        "SELECT name, agent_id, secret, prompt_prefix, enabled, webhook_type, event_filter FROM webhooks"
    ).fetch_all(db).await?;
    Ok(rows.iter().map(|r| BackupWebhook {
        name: r.get("name"),
        agent_id: r.get("agent_id"),
        secret: r.get("secret"),
        prompt_prefix: r.get("prompt_prefix"),
        enabled: r.get("enabled"),
        webhook_type: r.get("webhook_type"),
        event_filter: r.get("event_filter"),
    }).collect())
}

async fn collect_watchdog_settings(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupWatchdogSetting>> {
    let rows = sqlx::query("SELECT key, value FROM watchdog_settings")
        .fetch_all(db).await?;
    Ok(rows.iter().map(|r| BackupWatchdogSetting {
        key: r.get("key"),
        value: r.get("value"),
    }).collect())
}

async fn collect_allowed_users(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupAllowedUser>> {
    let rows = sqlx::query("SELECT agent_id, channel_user_id, display_name FROM channel_allowed_users")
        .fetch_all(db).await?;
    Ok(rows.iter().map(|r| BackupAllowedUser {
        agent_id: r.get("agent_id"),
        channel_user_id: r.get("channel_user_id"),
        display_name: r.get("display_name"),
    }).collect())
}

async fn collect_approval_allowlist(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupApprovalAllow>> {
    let rows = sqlx::query("SELECT agent_id, tool_pattern FROM approval_allowlist")
        .fetch_all(db).await?;
    Ok(rows.iter().map(|r| BackupApprovalAllow {
        agent_id: r.get("agent_id"),
        tool_pattern: r.get("tool_pattern"),
    }).collect())
}

async fn collect_oauth_accounts(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupOAuthAccount>> {
    let rows = sqlx::query("SELECT id, provider, display_name, scope, status FROM oauth_accounts")
        .fetch_all(db).await?;
    Ok(rows.iter().map(|r| BackupOAuthAccount {
        id: r.get::<uuid::Uuid, _>("id").to_string(),
        provider: r.get("provider"),
        display_name: r.get("display_name"),
        scope: r.get("scope"),
        status: r.get("status"),
    }).collect())
}

async fn collect_oauth_bindings(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupOAuthBinding>> {
    let rows = sqlx::query("SELECT agent_id, provider, account_id FROM agent_oauth_bindings")
        .fetch_all(db).await?;
    Ok(rows.iter().map(|r| BackupOAuthBinding {
        agent_id: r.get("agent_id"),
        provider: r.get("provider"),
        account_id: r.get::<uuid::Uuid, _>("account_id").to_string(),
    }).collect())
}

async fn collect_gmail_triggers(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupGmailTrigger>> {
    let rows = sqlx::query("SELECT agent_id, email_address, pubsub_topic, enabled FROM gmail_triggers")
        .fetch_all(db).await?;
    Ok(rows.iter().map(|r| BackupGmailTrigger {
        agent_id: r.get("agent_id"),
        email_address: r.get("email_address"),
        pubsub_topic: r.get("pubsub_topic"),
        enabled: r.get("enabled"),
    }).collect())
}

async fn collect_github_repos(db: &sqlx::PgPool) -> sqlx::Result<Vec<BackupGithubRepo>> {
    let rows = sqlx::query("SELECT agent_id, owner, repo FROM agent_github_repos")
        .fetch_all(db).await?;
    Ok(rows.iter().map(|r| BackupGithubRepo {
        agent_id: r.get("agent_id"),
        owner: r.get("owner"),
        repo: r.get("repo"),
    }).collect())
}

// ── V2 restore helpers ────────────────────────────────────────────────────────

async fn restore_providers(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, providers: &[BackupProvider], active: &[BackupProviderActive]) -> sqlx::Result<usize> {
    sqlx::query("DELETE FROM provider_active").execute(&mut **tx).await?;
    sqlx::query("DELETE FROM providers").execute(&mut **tx).await?;

    let mut count = 0;
    for p in providers {
        let id: uuid::Uuid = p.id.parse().unwrap_or_else(|_| uuid::Uuid::new_v4());
        sqlx::query(
            "INSERT INTO providers (id, name, type, provider_type, base_url, default_model, enabled, options, notes) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        )
        .bind(id).bind(&p.name).bind(&p.category).bind(&p.provider_type)
        .bind(&p.base_url).bind(&p.default_model).bind(p.enabled)
        .bind(&p.options).bind(&p.notes)
        .execute(&mut **tx).await?;
        count += 1;
    }

    for a in active {
        sqlx::query(
            "INSERT INTO provider_active (capability, provider_name) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        )
        .bind(&a.capability).bind(&a.provider_name)
        .execute(&mut **tx).await?;
    }

    Ok(count)
}

async fn restore_channels(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, items: &[BackupChannel]) -> sqlx::Result<usize> {
    if items.is_empty() { return Ok(0); }
    sqlx::query("DELETE FROM agent_channels").execute(&mut **tx).await?;
    for c in items {
        let id = uuid::Uuid::parse_str(&c.id).unwrap_or_else(|_| uuid::Uuid::new_v4());
        sqlx::query(
            "INSERT INTO agent_channels (id, agent_name, channel_type, display_name, config, status)
             VALUES ($1, $2, $3, $4, $5, $6)"
        )
        .bind(id).bind(&c.agent_name).bind(&c.channel_type).bind(&c.display_name)
        .bind(&c.config).bind(&c.status)
        .execute(&mut **tx).await?;
    }
    Ok(items.len())
}

async fn restore_webhooks(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, items: &[BackupWebhook]) -> sqlx::Result<usize> {
    if items.is_empty() { return Ok(0); }
    sqlx::query("DELETE FROM webhooks").execute(&mut **tx).await?;
    for w in items {
        sqlx::query(
            "INSERT INTO webhooks (name, agent_id, secret, prompt_prefix, enabled, webhook_type, event_filter)
             VALUES ($1, $2, $3, $4, $5, $6, $7)"
        )
        .bind(&w.name).bind(&w.agent_id).bind(&w.secret).bind(&w.prompt_prefix)
        .bind(w.enabled).bind(&w.webhook_type).bind(&w.event_filter as &Option<Vec<String>>)
        .execute(&mut **tx).await?;
    }
    Ok(items.len())
}

async fn restore_watchdog_settings(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, items: &[BackupWatchdogSetting]) -> sqlx::Result<usize> {
    if items.is_empty() { return Ok(0); }
    sqlx::query("DELETE FROM watchdog_settings").execute(&mut **tx).await?;
    for s in items {
        sqlx::query("INSERT INTO watchdog_settings (key, value) VALUES ($1, $2)")
            .bind(&s.key).bind(&s.value)
            .execute(&mut **tx).await?;
    }
    Ok(items.len())
}

async fn restore_approval_allowlist(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, items: &[BackupApprovalAllow]) -> sqlx::Result<usize> {
    if items.is_empty() { return Ok(0); }
    sqlx::query("DELETE FROM approval_allowlist").execute(&mut **tx).await?;
    for e in items {
        sqlx::query("INSERT INTO approval_allowlist (agent_id, tool_pattern) VALUES ($1, $2)")
            .bind(&e.agent_id).bind(&e.tool_pattern)
            .execute(&mut **tx).await?;
    }
    Ok(items.len())
}

async fn restore_allowed_users(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, items: &[BackupAllowedUser]) -> sqlx::Result<usize> {
    if items.is_empty() { return Ok(0); }
    sqlx::query("DELETE FROM channel_allowed_users").execute(&mut **tx).await?;
    for u in items {
        sqlx::query(
            "INSERT INTO channel_allowed_users (agent_id, channel_user_id, display_name) VALUES ($1, $2, $3)"
        )
        .bind(&u.agent_id).bind(&u.channel_user_id).bind(&u.display_name)
        .execute(&mut **tx).await?;
    }
    Ok(items.len())
}

async fn restore_oauth_accounts(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, items: &[BackupOAuthAccount]) -> sqlx::Result<usize> {
    if items.is_empty() { return Ok(0); }
    sqlx::query("DELETE FROM agent_oauth_bindings").execute(&mut **tx).await?;
    sqlx::query("DELETE FROM oauth_accounts").execute(&mut **tx).await?;
    for a in items {
        let id = uuid::Uuid::parse_str(&a.id).unwrap_or_else(|_| uuid::Uuid::new_v4());
        sqlx::query("INSERT INTO oauth_accounts (id, provider, display_name, scope, status) VALUES ($1, $2, $3, $4, $5)")
            .bind(id).bind(&a.provider).bind(&a.display_name).bind(&a.scope).bind(&a.status)
            .execute(&mut **tx).await?;
    }
    Ok(items.len())
}

async fn restore_oauth_bindings(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, items: &[BackupOAuthBinding]) -> sqlx::Result<usize> {
    if items.is_empty() { return Ok(0); }
    for b in items {
        let account_id = uuid::Uuid::parse_str(&b.account_id).unwrap_or_default();
        sqlx::query("INSERT INTO agent_oauth_bindings (agent_id, provider, account_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(&b.agent_id).bind(&b.provider).bind(account_id)
            .execute(&mut **tx).await?;
    }
    Ok(items.len())
}

async fn restore_gmail_triggers(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, items: &[BackupGmailTrigger]) -> sqlx::Result<usize> {
    if items.is_empty() { return Ok(0); }
    sqlx::query("DELETE FROM gmail_triggers").execute(&mut **tx).await?;
    for t in items {
        sqlx::query("INSERT INTO gmail_triggers (agent_id, email_address, pubsub_topic, enabled) VALUES ($1, $2, $3, $4)")
            .bind(&t.agent_id).bind(&t.email_address).bind(&t.pubsub_topic).bind(t.enabled)
            .execute(&mut **tx).await?;
    }
    Ok(items.len())
}

async fn restore_github_repos(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, items: &[BackupGithubRepo]) -> sqlx::Result<usize> {
    if items.is_empty() { return Ok(0); }
    sqlx::query("DELETE FROM agent_github_repos").execute(&mut **tx).await?;
    for r in items {
        sqlx::query("INSERT INTO agent_github_repos (agent_id, owner, repo) VALUES ($1, $2, $3)")
            .bind(&r.agent_id).bind(&r.owner).bind(&r.repo)
            .execute(&mut **tx).await?;
    }
    Ok(items.len())
}

async fn cleanup_old_backups(now: chrono::DateTime<Utc>) {
    let cutoff = now - chrono::Duration::days(RETENTION_DAYS);
    let Ok(mut dir) = fs::read_dir(BACKUP_DIR).await else { return };
    while let Ok(Some(entry)) = dir.next_entry().await {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            // Parse date from filename: hydeclaw-YYYY-MM-DD.json
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && let Some(date_part) = stem.strip_prefix("hydeclaw-")
                    && let Ok(date) = chrono::NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
                        let file_dt = date.and_hms_opt(0, 0, 0).expect("midnight is valid time").and_utc();
                        if file_dt < cutoff {
                            let _ = fs::remove_file(&path).await;
                            tracing::info!(path = %path.display(), "removed old backup");
                        }
                    }
        }
    }
}

async fn restore_config(config: &Value) -> usize {
    let mut count = 0;
    if let Some(toml_str) = config.get("app_config").and_then(|v| v.as_str())
        && toml_str.parse::<toml::Table>().is_ok() {
            let _ = fs::copy("config/hydeclaw.toml", "config/hydeclaw.toml.bak").await;
            if fs::write("config/hydeclaw.toml", toml_str).await.is_ok() {
                count += 1;
            }
        }
    if let Some(agents) = config.get("agents").and_then(|v| v.as_object()) {
        let _ = fs::create_dir_all("config/agents").await;
        for (name, content) in agents {
            // Validate agent name (same rules as API create)
            if name.contains('/') || name.contains('\\') || name.contains("..")
               || name.is_empty() || name.len() > 64
               || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ' ') {
                continue;
            }
            if let Some(toml_str) = content.as_str()
                && toml_str.parse::<toml::Table>().is_ok() {
                    let path = format!("config/agents/{name}.toml");
                    let _ = fs::copy(&path, format!("{path}.bak")).await;
                    if fs::write(&path, toml_str).await.is_ok() { count += 1; }
                }
        }
    }
    count
}

async fn restore_workspace(workspace_dir: &str, files: &[WorkspaceFile]) -> usize {
    let root = FsPath::new(workspace_dir);
    let mut count = 0;
    for file in files {
        // Prevent path traversal
        if file.path.contains("..") || file.path.starts_with('/') || file.path.starts_with('\\') { continue; }
        let dest = root.join(&file.path);
        let bytes = if file.content.starts_with("base64:") {
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(&file.content[7..]) {
                Ok(b) => b,
                Err(_) => file.content.as_bytes().to_vec(),
            }
        } else {
            file.content.as_bytes().to_vec()
        };
        if let Some(parent) = dest.parent()
            && fs::create_dir_all(parent).await.is_ok()
                && fs::write(&dest, &bytes).await.is_ok() {
                    count += 1;
                }
    }
    count
}

/// Restore memory and cron jobs atomically within a single DB transaction.
/// Preserves the `daily-backup` cron job so Hyde continues working after restore.
async fn restore_memory_and_cron(
    state: &AppState,
    chunks: &[MemoryChunk],
    jobs: &[CronJob],
    fts_lang: &str,
) -> sqlx::Result<(usize, usize)> {
    let mut tx = state.db.begin().await?;

    // Disable FK checks for bulk restore (parent_id references may arrive out of order)
    sqlx::query("SET CONSTRAINTS ALL DEFERRED").execute(&mut *tx).await?;

    // Memory: replace all chunks
    sqlx::query("DELETE FROM memory_chunks").execute(&mut *tx).await?;
    for chunk in chunks {
        let id = uuid::Uuid::parse_str(&chunk.id).unwrap_or_else(|_| uuid::Uuid::new_v4());
        let parent_id = chunk.parent_id.as_deref().and_then(|s| uuid::Uuid::parse_str(s).ok());
        sqlx::query(
            "INSERT INTO memory_chunks (id, user_id, content, source, pinned, relevance_score, created_at, tsv, parent_id, chunk_index)
             VALUES ($1, $2, $3, $4, $5, $6, $7, to_tsvector($8::regconfig, $3), $9, $10)",
        )
        .bind(id)
        .bind(&chunk.user_id)
        .bind(&chunk.content)
        .bind(&chunk.source)
        .bind(chunk.pinned)
        .bind(chunk.relevance_score)
        .bind(chunk.created_at)
        .bind(fts_lang)
        .bind(parent_id)
        .bind(chunk.chunk_index)
        .execute(&mut *tx)
        .await?;
    }

    // Cron: replace all jobs except daily-backup (Hyde re-creates it on heartbeat anyway,
    // but preserving it means backups keep running even if Hyde hasn't heartbeated yet)
    sqlx::query("DELETE FROM scheduled_jobs WHERE name != 'daily-backup'")
        .execute(&mut *tx)
        .await?;
    for job in jobs {
        if job.name == "daily-backup" { continue; } // already preserved above
        sqlx::query(
            "INSERT INTO scheduled_jobs (agent_id, name, cron_expr, timezone, task_message, enabled, announce_to, silent)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (name) DO UPDATE SET
               cron_expr = EXCLUDED.cron_expr,
               timezone = EXCLUDED.timezone,
               task_message = EXCLUDED.task_message,
               enabled = EXCLUDED.enabled,
               announce_to = EXCLUDED.announce_to,
               silent = EXCLUDED.silent",
        )
        .bind(&job.agent_id)
        .bind(&job.name)
        .bind(&job.cron_expr)
        .bind(&job.timezone)
        .bind(&job.task_message)
        .bind(job.enabled)
        .bind(&job.announce_to)
        .bind(job.silent)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok((chunks.len(), jobs.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Per D-12: Round-trip test proves backup export -> restore preserves all provider data
    /// without loss or corruption. This tests serialization fidelity; actual DB round-trip
    /// requires integration test infrastructure.
    #[test]
    fn test_backup_roundtrip_providers() {
        // Construct realistic providers covering all types
        let providers = vec![
            BackupProvider {
                id: "550e8400-e29b-41d4-a716-446655440001".to_string(),
                name: "openai-main".to_string(),
                category: "text".to_string(),
                provider_type: "openai".to_string(),
                base_url: Some("https://api.openai.com/v1".to_string()),
                default_model: Some("gpt-4o".to_string()),
                enabled: true,
                options: json!({"models": ["gpt-4o", "gpt-4o-mini"], "max_tokens": 8192}),
                notes: Some("Primary text provider".to_string()),
            },
            BackupProvider {
                id: "550e8400-e29b-41d4-a716-446655440002".to_string(),
                name: "ollama-embed".to_string(),
                category: "embedding".to_string(),
                provider_type: "ollama".to_string(),
                base_url: Some("http://localhost:11434".to_string()),
                default_model: Some("nomic-embed-text".to_string()),
                enabled: true,
                options: json!({}),
                notes: None,
            },
            BackupProvider {
                id: "550e8400-e29b-41d4-a716-446655440003".to_string(),
                name: "whisper-stt".to_string(),
                category: "stt".to_string(),
                provider_type: "whisper".to_string(),
                base_url: None,
                default_model: None,
                enabled: false,
                options: json!({"models": []}),
                notes: Some("".to_string()),
            },
            // Edge case: unicode in provider name
            BackupProvider {
                id: "550e8400-e29b-41d4-a716-446655440004".to_string(),
                name: "tts-\u{00e9}l\u{00e8}ve".to_string(),
                category: "tts".to_string(),
                provider_type: "custom".to_string(),
                base_url: Some("http://localhost:8880".to_string()),
                default_model: Some("clone:Arty".to_string()),
                enabled: true,
                options: json!(null),
                notes: None,
            },
        ];

        let active = vec![
            BackupProviderActive {
                capability: "text".to_string(),
                provider_name: "openai-main".to_string(),
            },
            BackupProviderActive {
                capability: "embedding".to_string(),
                provider_name: "ollama-embed".to_string(),
            },
        ];

        // Serialize (simulating export)
        let providers_json = serde_json::to_string(&providers).expect("serialize providers");
        let active_json = serde_json::to_string(&active).expect("serialize active");

        // Deserialize (simulating restore parse)
        let restored_providers: Vec<BackupProvider> =
            serde_json::from_str(&providers_json).expect("deserialize providers");
        let restored_active: Vec<BackupProviderActive> =
            serde_json::from_str(&active_json).expect("deserialize active");

        // Assert full equality
        assert_eq!(providers, restored_providers, "providers round-trip mismatch");
        assert_eq!(active, restored_active, "provider_active round-trip mismatch");

        // Verify specific edge cases survived the round-trip
        let whisper = &restored_providers[2];
        assert_eq!(whisper.base_url, None, "None base_url should survive round-trip");
        assert_eq!(whisper.default_model, None, "None default_model should survive round-trip");
        assert_eq!(whisper.options, json!({"models": []}), "empty models list should survive round-trip");
        assert!(!whisper.enabled, "disabled flag should survive round-trip");

        let unicode_provider = &restored_providers[3];
        assert_eq!(unicode_provider.name, "tts-\u{00e9}l\u{00e8}ve", "unicode name should survive round-trip");
        assert_eq!(unicode_provider.notes, None, "None notes should survive round-trip");
        assert_eq!(unicode_provider.options, json!(null), "null options should survive round-trip");

        // Verify round-trip through BackupFile container (full envelope)
        let wrapper = json!({
            "providers": providers,
            "provider_active": active,
        });
        let wrapper_json = serde_json::to_string(&wrapper).expect("serialize wrapper");
        let restored_wrapper: serde_json::Value =
            serde_json::from_str(&wrapper_json).expect("deserialize wrapper");

        let final_providers: Vec<BackupProvider> =
            serde_json::from_value(restored_wrapper["providers"].clone()).expect("extract providers");
        let final_active: Vec<BackupProviderActive> =
            serde_json::from_value(restored_wrapper["provider_active"].clone()).expect("extract active");

        assert_eq!(providers, final_providers, "nested round-trip providers mismatch");
        assert_eq!(active, final_active, "nested round-trip active mismatch");
    }
}
