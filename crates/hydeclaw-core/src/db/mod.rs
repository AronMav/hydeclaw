pub mod access;
pub mod approvals;
pub mod audit_queue;
pub mod github;
pub mod audit;
pub mod tool_audit;
pub mod tool_quality;
pub mod memory_queries;
pub mod notifications;
pub mod providers;
pub mod outbound;
pub mod pending;
pub mod session_wal;
pub mod sessions;
pub mod skill_metrics;
pub mod skill_versions;
pub mod usage;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn create_pool(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(30)
        .min_connections(3)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await?;
    Ok(pool)
}
