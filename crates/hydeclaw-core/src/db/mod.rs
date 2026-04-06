pub mod access;
pub mod approvals;
pub mod github;
pub mod audit;
pub mod tool_audit;
pub mod tool_quality;
pub mod memory_queries;
pub mod notifications;
pub mod providers;
pub mod outbound;
pub mod pending;
pub mod session_documents;
pub mod sessions;
pub mod skill_metrics;
pub mod skill_versions;
pub mod usage;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn create_pool(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .min_connections(2)
        .connect(url)
        .await?;
    Ok(pool)
}
