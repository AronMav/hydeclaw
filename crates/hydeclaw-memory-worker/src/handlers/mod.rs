pub mod reindex;

use crate::tasks::MemoryTask;
#[cfg(feature = "test-noop")]
use serde_json::json;
use sqlx::PgPool;

pub struct DispatchCtx<'a> {
    pub toolgate_url: &'a str,
    pub workspace_dir: &'a str,
    pub fts_language: &'a str,
}

pub async fn dispatch(
    task: &MemoryTask,
    db: &PgPool,
    ctx: &DispatchCtx<'_>,
) -> anyhow::Result<serde_json::Value> {
    match task.task_type.as_str() {
        "reindex" => reindex::handle(task, db, ctx.toolgate_url, ctx.workspace_dir, ctx.fts_language).await,
        // Phase 66 REF-04: test-only no-op dispatch arm. Gated behind `test-noop`
        // feature so production builds cannot accept `test_noop` tasks. The
        // integration test in tests/integration_memory_worker_notify.rs builds
        // the worker with --features test-noop and enqueues `test_noop` rows.
        #[cfg(feature = "test-noop")]
        "test_noop" => Ok(json!({ "ok": true, "test_noop": true })),
        other => anyhow::bail!("unknown task type: {other}"),
    }
}
