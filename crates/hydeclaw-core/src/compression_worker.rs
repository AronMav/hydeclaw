//! Background memory compression worker.
//! Periodically finds old non-pinned memory chunks, groups them by topic,
//! summarizes via LLM, and archives originals.
//! Pauses during active chats. Auto-restarts on panic.

use sqlx::PgPool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::agent::memory_service::MemoryService;
use crate::agent::providers::LlmProvider;
use crate::db::memory_queries::{fetch_compressible_groups, archive_chunks, CompressibleGroup};
use crate::graph_worker::ACTIVE_CHATS;
use crate::memory::MemoryStore;

/// Spawn the compression worker. Auto-restarts on panic.
/// Accepts a `CancellationToken` for cooperative shutdown and returns a `JoinHandle`
/// so the caller can await clean termination.
pub fn spawn_worker(
    db: PgPool,
    provider: Arc<dyn LlmProvider>,
    memory_store: Arc<MemoryStore>,
    compression_age_days: u32,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if cancel.is_cancelled() {
                tracing::info!("compression worker shutting down (cancelled)");
                break;
            }
            tracing::info!("compression worker started");
            if let Err(e) = worker_loop(&db, &provider, &memory_store, compression_age_days, &cancel).await {
                tracing::error!(error = %e, "compression worker error, restarting in 30s");
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!("compression worker shutting down (cancelled during restart backoff)");
                        break;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                }
            } else {
                // worker_loop returned Ok — cancelled
                break;
            }
        }
    })
}

async fn worker_loop(
    db: &PgPool,
    provider: &Arc<dyn LlmProvider>,
    memory_store: &Arc<MemoryStore>,
    compression_age_days: u32,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    loop {
        if cancel.is_cancelled() {
            tracing::info!("compression worker cancelled");
            return Ok(());
        }

        // Pause while chats are active (Pi resource contention)
        if ACTIVE_CHATS.load(Ordering::Relaxed) > 0 {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
            }
            continue;
        }

        // Fetch all compressible groups
        let groups = match fetch_compressible_groups(db, compression_age_days).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = %e, "compression worker: failed to fetch compressible groups");
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
                }
                continue;
            }
        };

        if groups.is_empty() {
            tracing::debug!("compression worker: no compressible groups, sleeping 1h");
        } else {
            tracing::info!(groups = groups.len(), "compression worker: processing groups");
            for group in groups {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                // Pause again between groups if chats become active
                if ACTIVE_CHATS.load(Ordering::Relaxed) > 0 {
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                    }
                }
                match compress_group(db, provider, memory_store.as_ref(), group).await {
                    Ok(archived) => {
                        tracing::info!(archived, "compression worker: group compressed");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "compression worker: group compression failed, skipping");
                    }
                }
            }
        }

        // Sleep 1 hour before next scan
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(3600)) => {}
        }
    }
}

/// Three-step idempotent compression for one (agent_id, topic) group:
/// 1. Concatenate chunk contents
/// 2. LLM summarize (with 120s timeout)
/// 3. Insert summary chunk + archive originals
pub async fn compress_group(
    db: &PgPool,
    provider: &Arc<dyn LlmProvider>,
    memory_store: &dyn MemoryService,
    group: CompressibleGroup,
) -> anyhow::Result<u64> {
    use hydeclaw_types::{Message, MessageRole};

    // Step 1: Concatenate chunk contents
    let concatenated: String = group
        .chunks
        .iter()
        .enumerate()
        .map(|(i, c)| format!("--- Chunk {} ---\n{}\n", i + 1, c.content))
        .collect::<Vec<_>>()
        .join("\n");

    // Determine inherited category (first non-None from originals)
    let category: Option<String> = group.chunks.iter().find_map(|c| c.category.clone());

    // Step 2: LLM summarization with 120s timeout
    let prompt = format!(
        "Summarize the following memory chunks into a single cohesive summary. \
         Preserve all key facts, decisions, and actionable information. \
         Remove redundancy. Keep the summary concise but complete.\n\n{}",
        concatenated
    );

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        provider.chat(
            &[Message {
                role: MessageRole::User,
                content: prompt,
                tool_calls: None,
                tool_call_id: None,
                thinking_blocks: vec![],
            }],
            &[],
        ),
    )
    .await;

    let response = match result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => anyhow::bail!("LLM summarization failed: {}", e),
        Err(_) => anyhow::bail!("LLM summarization timed out (120s)"),
    };

    let summary_text = response.content.trim().to_string();
    if summary_text.is_empty() {
        anyhow::bail!("LLM returned empty summary");
    }

    // Step 3a: Insert summary chunk via MemoryStore (creates embedding for searchability)
    // source="compression", category/topic inherited, pinned=false
    let _summary_id = memory_store
        .index(
            &summary_text,
            "compression",
            false,
            category.as_deref(),
            Some(group.topic.as_str()),
        )
        .await?;

    // Step 3b: Archive original chunks
    let chunk_ids: Vec<uuid::Uuid> = group.chunks.iter().map(|c| c.id).collect();
    let archived = archive_chunks(db, &chunk_ids).await?;

    tracing::info!(
        agent_id = %group.agent_id,
        topic = %group.topic,
        chunks_archived = archived,
        "compressed memory group"
    );

    Ok(archived)
}
