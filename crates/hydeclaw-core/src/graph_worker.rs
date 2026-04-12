//! Background graph extraction worker.
//! Processes `graph_extraction_queue` one document at a time
//! with adaptive rate limiting. Pauses during active chats.
//! Auto-restarts on panic.

use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::agent::providers::LlmProvider;

/// Active chat counter. Worker pauses when > 0.
pub static ACTIVE_CHATS: AtomicUsize = AtomicUsize::new(0);

/// RAII guard that increments `ACTIVE_CHATS` on creation, decrements on drop.
pub struct ChatActiveGuard;

impl ChatActiveGuard {
    pub fn new() -> Self {
        ACTIVE_CHATS.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for ChatActiveGuard {
    fn drop(&mut self) {
        ACTIVE_CHATS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Reset any graph queue items stuck in 'processing' from a previous crash.
/// On worker startup, anything still in 'processing' is from a previous run and should be retried.
async fn reset_stale_processing(db: &PgPool) {
    let result = sqlx::query(
        "UPDATE graph_extraction_queue SET status = 'pending' \
         WHERE status = 'processing'"
    )
    .execute(db)
    .await;
    match result {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(count = r.rows_affected(), "reset stale 'processing' graph queue items to 'pending'");
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to reset stale graph queue items");
        }
        _ => {}
    }
}

/// Spawn the extraction worker. Auto-restarts on error.
/// Accepts a `CancellationToken` for cooperative shutdown and returns a `JoinHandle`
/// so the caller can await clean termination.
pub fn spawn_worker(db: PgPool, provider: Arc<dyn LlmProvider>, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Reset items stuck in 'processing' from a previous crash
        reset_stale_processing(&db).await;

        loop {
            if cancel.is_cancelled() {
                tracing::info!("graph extraction worker shutting down (cancelled)");
                break;
            }
            tracing::info!("graph extraction worker started");
            if let Err(e) = worker_loop(&db, &provider, &cancel).await {
                tracing::error!(error = %e, "graph extraction worker error, restarting in 30s");
                tokio::select! {
                    () = cancel.cancelled() => {
                        tracing::info!("graph extraction worker shutting down (cancelled during restart backoff)");
                        break;
                    }
                    () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                }
            } else {
                // worker_loop returned Ok — cancelled
                break;
            }
        }
    })
}

async fn worker_loop(db: &PgPool, provider: &Arc<dyn LlmProvider>, cancel: &CancellationToken) -> anyhow::Result<()> {
    let mut consecutive_errors = 0u32;
    loop {
        if cancel.is_cancelled() {
            tracing::info!("graph extraction worker cancelled");
            return Ok(());
        }

        // Pause while chats are active
        if ACTIVE_CHATS.load(Ordering::Relaxed) > 0 {
            tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
            }
            continue;
        }

        // Fetch next item (SELECT FOR UPDATE SKIP LOCKED — concurrent-safe)
        let item: Option<(uuid::Uuid, String)> = match sqlx::query_as(
            "UPDATE graph_extraction_queue
             SET status = 'processing', attempts = attempts + 1
             WHERE chunk_id = (
                 SELECT chunk_id FROM graph_extraction_queue
                 WHERE status = 'pending' OR (status = 'failed' AND attempts < 3)
                 ORDER BY created_at LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING chunk_id, (SELECT content FROM memory_chunks WHERE id = chunk_id)",
        )
        .fetch_optional(db)
        .await
        {
            Ok(item) => item,
            Err(e) => {
                tracing::warn!(error = %e, "graph worker: DB query failed");
                consecutive_errors += 1;
                tokio::select! {
                    () = cancel.cancelled() => return Ok(()),
                    () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                }
                continue;
            }
        };

        let Some((chunk_id, content)) = item else {
            // Queue empty — sleep and check again
            tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
            }
            consecutive_errors = 0;
            continue;
        };

        if content.len() < 100 {
            if let Err(e) = sqlx::query(
                "UPDATE graph_extraction_queue SET status = 'done', processed_at = now() WHERE chunk_id = $1",
            )
            .bind(chunk_id)
            .execute(db)
            .await
            {
                tracing::error!(error = %e, "graph_worker: failed to update extraction status");
            }
            continue;
        }

        let chunk_id_str = chunk_id.to_string();
        let mut end = content.len().min(1500);
        while end > 0 && !content.is_char_boundary(end) { end -= 1; }
        let truncated = &content[..end];
        tokio::select! {
            () = cancel.cancelled() => {
                tracing::info!("graph worker cancelled during extraction");
                return Ok(());
            }
            result = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                crate::memory_graph::extract_entities_for_chunk(db, provider, truncated, &chunk_id_str),
            ) => {
                match result {
                    Ok(Ok(count)) => {
                        if let Err(e) = sqlx::query(
                            "UPDATE graph_extraction_queue SET status = 'done', processed_at = now() WHERE chunk_id = $1",
                        )
                        .bind(chunk_id)
                        .execute(db)
                        .await
                        {
                            tracing::error!(error = %e, "graph_worker: failed to update extraction status");
                        }
                        consecutive_errors = 0;
                        tracing::debug!(chunk = %chunk_id, entities = count, "extraction ok");
                        tokio::select! {
                            () = cancel.cancelled() => return Ok(()),
                            () = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
                        }
                    }
                    Ok(Err(e)) => {
                        if let Err(db_err) = sqlx::query(
                            "UPDATE graph_extraction_queue SET status = 'failed', last_error = $2 WHERE chunk_id = $1",
                        )
                        .bind(chunk_id)
                        .bind(e.to_string())
                        .execute(db)
                        .await
                        {
                            tracing::error!(error = %db_err, "graph_worker: failed to update extraction status");
                        }
                        consecutive_errors += 1;
                        let delay = std::cmp::min(10 * u64::from(consecutive_errors), 120);
                        tracing::warn!(chunk = %chunk_id, error = %e, delay, "extraction failed");
                        tokio::select! {
                            () = cancel.cancelled() => return Ok(()),
                            () = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {}
                        }
                    }
                    Err(_) => {
                        if let Err(e) = sqlx::query(
                            "UPDATE graph_extraction_queue SET status = 'failed', last_error = 'timeout 60s' WHERE chunk_id = $1",
                        )
                        .bind(chunk_id)
                        .execute(db)
                        .await
                        {
                            tracing::error!(error = %e, "graph_worker: failed to update extraction status");
                        }
                        consecutive_errors += 1;
                        tracing::warn!(chunk = %chunk_id, "extraction timed out (60s)");
                        tokio::select! {
                            () = cancel.cancelled() => return Ok(()),
                            () = tokio::time::sleep(std::time::Duration::from_secs(10)) => {}
                        }
                    }
                }
            }
        }
    }
}

/// Get queue status counts: (pending, processing, done, failed).
pub async fn queue_status(db: &PgPool) -> anyhow::Result<(i64, i64, i64, i64)> {
    let row: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            COUNT(*) FILTER (WHERE status = 'pending'),
            COUNT(*) FILTER (WHERE status = 'processing'),
            COUNT(*) FILTER (WHERE status = 'done'),
            COUNT(*) FILTER (WHERE status = 'failed' AND attempts >= 3)
         FROM graph_extraction_queue",
    )
    .fetch_one(db)
    .await?;
    Ok(row)
}
