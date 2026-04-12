pub mod reindex;

use crate::tasks::MemoryTask;
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
        other => anyhow::bail!("unknown task type: {other}"),
    }
}
