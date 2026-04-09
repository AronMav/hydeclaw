pub mod memory_service;
pub mod context_builder;
pub mod tool_executor;
pub mod session_manager;
pub mod channel_actions;
pub mod channel_kind;
pub mod cli_backend;
pub mod hooks;
pub mod engine;
pub use engine::AgentDispatch;
pub(crate) mod error_classify;
pub(crate) mod localization;
pub mod handle;
pub mod history;
pub mod model_discovery;
pub mod providers;
pub(crate) mod providers_http;
pub(crate) mod openapi;
pub(crate) mod pii;
pub(crate) mod json_repair;
pub(crate) mod thinking;
pub mod subagent_state;
pub mod tool_loop;
pub(crate) mod url_tools;
pub mod mention_parser;
pub mod workspace;

/// Delete upload files older than `max_age` from workspace/uploads/.
pub async fn cleanup_stale_uploads(workspace_dir: &str, max_age: std::time::Duration) -> usize {
    let uploads_dir = std::path::PathBuf::from(workspace_dir).join("uploads");
    if !uploads_dir.exists() {
        return 0;
    }
    let mut deleted = 0;
    let cutoff = std::time::SystemTime::now() - max_age;
    let mut entries = match tokio::fs::read_dir(&uploads_dir).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, path = %uploads_dir.display(), "failed to read uploads directory for cleanup");
            return 0;
        }
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_file() { continue; }
        let Ok(meta) = tokio::fs::metadata(&path).await else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if modified >= cutoff { continue; }
        if tokio::fs::remove_file(&path).await.is_ok() {
            deleted += 1;
        }
    }
    if deleted > 0 {
        tracing::info!(deleted, "cleaned up stale uploads");
    }
    deleted
}
