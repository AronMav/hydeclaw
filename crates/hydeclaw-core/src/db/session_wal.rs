//! Session WAL (Write-Ahead Log) — journal table for session lifecycle events.
//!
//! During normal operation, session state transitions (running, tool_start, tool_end,
//! done, failed) are logged to `session_events`. On crash recovery, this WAL is read
//! to identify what was in-flight and reconstruct state cleanly — no synthetic
//! "[interrupted]" messages are injected.

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

/// A tool call that was started but never completed (no matching tool_end).
#[derive(Debug)]
pub struct PendingToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
}

/// Reconstruct LoopDetector state for a session from the WAL.
pub async fn warm_up_detector(
    db: &PgPool,
    session_id: Uuid,
    detector: &mut crate::agent::tool_loop::LoopDetector,
) -> Result<()> {
    // Get last 64 tool_start events (buffer size of detector)
    let rows = sqlx::query(
        "SELECT payload FROM session_events \
         WHERE session_id = $1 AND event_type = 'tool_start' \
         ORDER BY created_at DESC LIMIT 64",
    )
    .bind(session_id)
    .fetch_all(db)
    .await?;

    // Apply in chronological order (reverse of the query result)
    for row in rows.into_iter().rev() {
        let payload: Option<serde_json::Value> = sqlx::Row::try_get(&row, "payload").ok();
        if let Some(payload) = payload {
            let name = payload.get("tool_name").and_then(|v| v.as_str());
            let hash_hex = payload.get("args_hash").and_then(|v| v.as_str());
            
            if let (Some(n), Some(h_str)) = (name, hash_hex) {
                if let Ok(h) = u64::from_str_radix(h_str, 16) {
                    detector.warm_up(h, n);
                }
            }
        }
    }
    Ok(())
}

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

/// Find tool_start events without a matching tool_end for the same tool_call_id.
pub async fn get_pending_tool_calls(
    db: &PgPool,
    session_id: Uuid,
) -> Result<Vec<PendingToolCall>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT
            s.payload->>'tool_call_id' AS tool_call_id,
            s.payload->>'tool_name' AS tool_name
         FROM session_events s
         WHERE s.session_id = $1
           AND s.event_type = 'tool_start'
           AND NOT EXISTS (
               SELECT 1 FROM session_events e
               WHERE e.session_id = s.session_id
                 AND e.event_type = 'tool_end'
                 AND e.payload->>'tool_call_id' = s.payload->>'tool_call_id'
           )",
    )
    .bind(session_id)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(tool_call_id, tool_name)| PendingToolCall {
            tool_call_id,
            tool_name,
        })
        .collect())
}

/// WAL-based crash recovery: find sessions that were running when the process died,
/// reconstruct their state from the WAL, and mark them as 'interrupted'.
///
/// Returns the number of sessions recovered.
pub async fn cleanup_recovered_sessions(db: &PgPool) -> Result<usize> {
    // Find sessions with a 'running' WAL event but no subsequent 'done'/'failed'/'interrupted'.
    // Also include sessions with run_status='running' (legacy path, pre-WAL).
    let interrupted = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT s.id FROM sessions s
         WHERE s.run_status = 'running'
            OR (
                EXISTS (
                    SELECT 1 FROM session_events se
                    WHERE se.session_id = s.id AND se.event_type = 'running'
                )
                AND NOT EXISTS (
                    SELECT 1 FROM session_events se2
                    WHERE se2.session_id = s.id
                      AND se2.event_type IN ('done', 'failed', 'interrupted')
                      AND se2.created_at > (
                          SELECT MAX(se3.created_at) FROM session_events se3
                          WHERE se3.session_id = s.id AND se3.event_type = 'running'
                      )
                )
            )
            OR EXISTS (
                SELECT 1 FROM messages m
                WHERE m.session_id = s.id AND m.status = 'streaming'
            )
         LIMIT 100",
    )
    .fetch_all(db)
    .await?;

    let count = interrupted.len();
    if count > 0 {
        tracing::info!(count, "WAL recovery: cleaning up interrupted sessions");
    }

    for session_id in &interrupted {
        // 1. Get pending tool calls from WAL
        let pending = match get_pending_tool_calls(db, *session_id).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, session_id = %session_id, "failed to get pending tool calls from WAL");
                vec![]
            }
        };

        // 2. Store interrupted tools in session metadata
        if !pending.is_empty() {
            let tools_json: Vec<serde_json::Value> = pending
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "tool_call_id": p.tool_call_id,
                        "tool_name": p.tool_name,
                    })
                })
                .collect();
            let tools_value = serde_json::Value::Array(tools_json);

            if let Err(e) = sqlx::query(
                "UPDATE sessions SET metadata = COALESCE(metadata, '{}'::jsonb) || jsonb_build_object('interrupted_tools', $2::jsonb) WHERE id = $1",
            )
            .bind(session_id)
            .bind(&tools_value)
            .execute(db)
            .await
            {
                tracing::warn!(error = %e, session_id = %session_id, "failed to store interrupted_tools metadata");
            }
        }

        // 3. Delete orphaned streaming messages
        if let Err(e) =
            sqlx::query("DELETE FROM messages WHERE session_id = $1 AND status = 'streaming'")
                .bind(session_id)
                .execute(db)
                .await
        {
            tracing::warn!(error = %e, session_id = %session_id, "failed to delete orphaned streaming message");
        }

        // 4. Log 'interrupted' event to WAL
        if let Err(e) = log_event(
            db,
            *session_id,
            "interrupted",
            Some(&serde_json::json!({
                "pending_tool_calls": pending.len(),
            })),
        )
        .await
        {
            tracing::warn!(error = %e, session_id = %session_id, "failed to log interrupted WAL event");
        }

        // 5. Mark session as interrupted
        if let Err(e) =
            crate::db::sessions::set_session_run_status(db, *session_id, "interrupted").await
        {
            tracing::warn!(error = %e, session_id = %session_id, "failed to mark session interrupted");
        }
    }

    // 6. Clear stale streamStatus from UI metadata
    if let Err(e) = sqlx::query(
        "UPDATE sessions
         SET metadata = jsonb_set(metadata, '{ui_state,streamStatus}', '\"idle\"')
         WHERE metadata->'ui_state'->>'streamStatus' = 'streaming'",
    )
    .execute(db)
    .await
    {
        tracing::warn!(error = %e, "failed to clear stale streamStatus from UI metadata");
    }

    Ok(count)
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
