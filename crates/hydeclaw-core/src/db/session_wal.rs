//! Session WAL (Write-Ahead Log) — journal table for session lifecycle events.
//!
//! During normal operation, session state transitions (running, `tool_start`, `tool_end`,
//! done, failed) are logged to `session_events`. On crash recovery, this WAL is read
//! to identify what was in-flight and reconstruct state cleanly — no synthetic
//! "[interrupted]" messages are injected.

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

/// Log a session lifecycle event to the WAL.
pub async fn log_event(
    db: &PgPool,
    session_id: Uuid,
    event_type: &str,
    payload: Option<&serde_json::Value>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO session_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
    )
    .bind(session_id)
    .bind(event_type)
    .bind(payload)
    .execute(db)
    .await?;
    Ok(())
}

/// Delete WAL events older than `days` to prevent unbounded table growth.
pub async fn prune_old_events(db: &PgPool, days: u32) -> Result<u64> {
    if days == 0 {
        return Ok(0);
    }
    let result = sqlx::query(
        "DELETE FROM session_events WHERE created_at < now() - make_interval(days => $1)",
    )
    .bind(days as i32)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
