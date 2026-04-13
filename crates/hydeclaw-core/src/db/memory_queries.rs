use anyhow::{Context, Result};
use sqlx::PgPool;

use crate::memory::{MemoryChunk, MemoryResult};

/// Validate FTS language is a safe identifier (letters only, no SQL injection).
fn validate_fts_lang(lang: &str) -> Result<()> {
    if lang.is_empty() || !lang.chars().all(|c| c.is_ascii_lowercase()) {
        anyhow::bail!("invalid FTS language: {lang}");
    }
    Ok(())
}

// ── Helper ───────────────────────────────────────────────────────────────────

/// Map a sqlx Row to `MemoryResult`.
fn row_to_memory_result(r: &sqlx::postgres::PgRow) -> MemoryResult {
    use sqlx::Row;
    MemoryResult {
        id: r.get("id"),
        content: r.get("content"),
        source: r.get("source"),
        pinned: r.get("pinned"),
        relevance_score: r.get("relevance_score"),
        similarity: r.get("similarity"),
        parent_id: r.try_get::<Option<String>, _>("parent_id").ok().flatten(),
        chunk_index: r.try_get::<i32, _>("chunk_index").unwrap_or(0),
        category: r.try_get::<Option<String>, _>("category").ok().flatten(),
        topic: r.try_get::<Option<String>, _>("topic").ok().flatten(),
    }
}

/// Map a sqlx Row to `MemoryChunk`.
fn row_to_memory_chunk(r: &sqlx::postgres::PgRow) -> MemoryChunk {
    use sqlx::Row;
    MemoryChunk {
        id: r.get("id"),
        content: r.get("content"),
        source: r.get("source"),
        pinned: r.get("pinned"),
        relevance_score: r.get("relevance_score"),
        created_at: r.get("created_at"),
        accessed_at: r.get("accessed_at"),
        category: r.try_get::<Option<String>, _>("category").ok().flatten(),
        topic: r.try_get::<Option<String>, _>("topic").ok().flatten(),
    }
}

// ── Initialize ───────────────────────────────────────────────────────────────

/// Check the dimension of existing embeddings in the database.
pub async fn get_existing_embedding_dim(db: &PgPool) -> Option<i32> {
    sqlx::query_scalar(
        "SELECT vector_dims(embedding)::int FROM memory_chunks WHERE embedding IS NOT NULL LIMIT 1",
    )
    .fetch_optional(db)
    .await
    .unwrap_or(None)
}

/// Delete all memory chunks that have embeddings (dimension mismatch cleanup).
pub async fn clear_embeddings(db: &PgPool) -> Result<()> {
    sqlx::query("DELETE FROM memory_chunks WHERE embedding IS NOT NULL")
        .execute(db)
        .await
        .context("failed to clear memory_chunks after dimension change")?;
    Ok(())
}

/// Drop the HNSW embedding index.
pub async fn drop_hnsw_index(db: &PgPool) -> Result<()> {
    sqlx::query("DROP INDEX IF EXISTS idx_memory_embedding_hnsw")
        .execute(db)
        .await?;
    Ok(())
}

/// Create HNSW index if it doesn't exist.
pub async fn ensure_hnsw_index(db: &PgPool, dim: u32) -> Result<()> {
    // SAFETY: `dim` is u32 from embed_dim config, not user input.
    let sql = format!(
        "CREATE INDEX IF NOT EXISTS idx_memory_embedding_hnsw \
         ON memory_chunks USING hnsw ((embedding::halfvec({dim})) halfvec_cosine_ops) \
         WITH (m = 16, ef_construction = 64)"
    );
    sqlx::query(&sql)
        .execute(db)
        .await
        .context("failed to create HNSW index")?;
    Ok(())
}

// ── Search ───────────────────────────────────────────────────────────────────

/// Fetch all pinned chunks for a given agent, ordered oldest first.
/// Includes shared chunks (scope = 'shared') visible to all agents.
/// Used by L0 context loading — no embedding or search query needed.
pub async fn fetch_pinned(db: &PgPool, agent_id: &str) -> Result<Vec<MemoryChunk>> {
    let rows = sqlx::query(
        r"SELECT id::text, content, COALESCE(source,'') AS source, pinned,
                  COALESCE(relevance_score, 1.0)::float8 AS relevance_score,
                  created_at, accessed_at,
                  category, topic
           FROM memory_chunks
           WHERE ($1 = '' OR agent_id = $1 OR scope = 'shared') AND pinned = true
           ORDER BY created_at ASC",
    )
    .bind(agent_id)
    .fetch_all(db)
    .await
    .context("failed to fetch pinned memory chunks")?;

    Ok(rows.iter().map(row_to_memory_chunk).collect())
}

/// Semantic similarity search: find nearest chunks by embedding cosine distance.
/// Filters by agent_id so that only the agent's own chunks (or shared chunks) are returned.
pub async fn search_semantic(
    db: &PgPool,
    vec_str: &str,
    candidate_limit: i64,
    agent_id: &str,
) -> Result<Vec<MemoryResult>> {
    let rows = sqlx::query(
        r"SELECT id::text,
                  content,
                  COALESCE(source, '') AS source,
                  pinned,
                  COALESCE(relevance_score, 1.0)::float8 AS relevance_score,
                  (1.0 - (embedding <=> $1::halfvec))::float8 AS similarity,
                  parent_id::text,
                  chunk_index,
                  category,
                  topic
           FROM memory_chunks
           WHERE embedding IS NOT NULL
             AND ($3 = '' OR agent_id = $3 OR scope = 'shared')
           ORDER BY embedding <=> $1::halfvec
           LIMIT $2",
    )
    .bind(vec_str)
    .bind(candidate_limit)
    .bind(agent_id)
    .fetch_all(db)
    .await
    .context("memory search query failed")?;

    Ok(rows.iter().map(row_to_memory_result).collect())
}

/// Full-text search using `PostgreSQL` tsvector/tsquery.
/// Filters by agent_id so that only the agent's own chunks (or shared chunks) are returned.
pub async fn search_fts(
    db: &PgPool,
    query: &str,
    limit: i64,
    lang: &str,
    agent_id: &str,
) -> Result<Vec<MemoryResult>> {
    validate_fts_lang(lang)?;
    // SAFETY: `lang` is validated by validate_fts_lang() which only allows lowercase ASCII
    // letters. Not user input -- comes from server config.
    let sql = format!(
        r"SELECT id::text,
                  content,
                  COALESCE(source, '') AS source,
                  pinned,
                  COALESCE(relevance_score, 1.0)::float8 AS relevance_score,
                  ts_rank_cd(tsv, plainto_tsquery('{lang}', $1))::float8 AS similarity,
                  parent_id::text,
                  chunk_index,
                  category,
                  topic
           FROM memory_chunks
           WHERE tsv @@ plainto_tsquery('{lang}', $1)
             AND ($3 = '' OR agent_id = $3 OR scope = 'shared')
           ORDER BY ts_rank_cd(tsv, plainto_tsquery('{lang}', $1)) DESC,
                    relevance_score DESC
           LIMIT $2",
    );

    let rows = sqlx::query(&sql)
        .bind(query)
        .bind(limit)
        .bind(agent_id)
        .fetch_all(db)
        .await
        .context("FTS search query failed")?;

    Ok(rows.iter().map(row_to_memory_result).collect())
}

/// Update `accessed_at` timestamp for the given chunk IDs.
pub async fn touch_accessed(db: &PgPool, ids: &[uuid::Uuid]) {
    if ids.is_empty() {
        return;
    }
    let _ = sqlx::query(
        "UPDATE memory_chunks SET accessed_at = now() WHERE id = ANY($1)",
    )
    .bind(ids)
    .execute(db)
    .await;
}

/// Return the most-recently-accessed memory chunks (pinned first).
pub async fn fetch_recent(db: &PgPool, limit: i64) -> Result<Vec<MemoryResult>> {
    let rows = sqlx::query(
        r"SELECT id::text,
                  content,
                  COALESCE(source, '') AS source,
                  pinned,
                  COALESCE(relevance_score, 1.0)::float8 AS relevance_score,
                  1.0::float8 AS similarity,
                  parent_id::text,
                  chunk_index,
                  category,
                  topic
           FROM memory_chunks
           ORDER BY pinned DESC, COALESCE(accessed_at, created_at) DESC
           LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
    .context("recent memory query failed")?;

    Ok(rows.iter().map(row_to_memory_result).collect())
}

// ── Index ────────────────────────────────────────────────────────────────────

/// Insert a new memory chunk with embedding and FTS tsvector.
#[allow(clippy::too_many_arguments)]
pub async fn insert_chunk(
    db: &PgPool,
    id: &str,
    content: &str,
    vec_str: &str,
    source: &str,
    pinned: bool,
    lang: &str,
    parent_id: Option<&str>,
    chunk_index: i32,
    category: Option<&str>,
    topic: Option<&str>,
    scope: &str,
    agent_id: &str,
) -> Result<()> {
    validate_fts_lang(lang)?;
    // SAFETY: `lang` is validated by validate_fts_lang() which only allows lowercase ASCII
    // letters. Not user input -- comes from server config.
    let sql = format!(
        r"INSERT INTO memory_chunks (id, agent_id, content, embedding, source, pinned, relevance_score, tsv, parent_id, chunk_index, category, topic, scope)
           VALUES ($1::uuid, $11, $2, $3::halfvec, $4, $5, 1.0, to_tsvector('{lang}', $2), $6::uuid, $7, $8, $9, $10)",
    );

    sqlx::query(&sql)
        .bind(id)
        .bind(content)
        .bind(vec_str)
        .bind(source)
        .bind(pinned)
        .bind(parent_id)
        .bind(chunk_index)
        .bind(category)
        .bind(topic)
        .bind(scope)
        .bind(agent_id)
        .execute(db)
        .await
        .context("failed to insert memory chunk")?;

    Ok(())
}

// ── Get ──────────────────────────────────────────────────────────────────────

/// Retrieve a single chunk by ID.
pub async fn get_chunk_by_id(db: &PgPool, id: &str) -> Result<Vec<MemoryChunk>> {
    let rows = sqlx::query(
        r"SELECT id::text, content, COALESCE(source,'') AS source, pinned,
                  COALESCE(relevance_score,1.0)::float8 AS relevance_score,
                  created_at, accessed_at,
                  category, topic
           FROM memory_chunks WHERE id = $1::uuid",
    )
    .bind(id)
    .fetch_all(db)
    .await?;

    Ok(rows.iter().map(row_to_memory_chunk).collect())
}

/// Retrieve chunks by source, ordered by creation date.
pub async fn get_chunks_by_source(
    db: &PgPool,
    source: &str,
    limit: i64,
) -> Result<Vec<MemoryChunk>> {
    let rows = sqlx::query(
        r"SELECT id::text, content, COALESCE(source,'') AS source, pinned,
                  COALESCE(relevance_score,1.0)::float8 AS relevance_score,
                  created_at, accessed_at,
                  category, topic
           FROM memory_chunks WHERE source = $1
           ORDER BY created_at DESC LIMIT $2",
    )
    .bind(source)
    .bind(limit)
    .fetch_all(db)
    .await?;

    Ok(rows.iter().map(row_to_memory_chunk).collect())
}

/// Retrieve most recently accessed chunks.
pub async fn get_chunks_recent(db: &PgPool, limit: i64) -> Result<Vec<MemoryChunk>> {
    let rows = sqlx::query(
        r"SELECT id::text, content, COALESCE(source,'') AS source, pinned,
                  COALESCE(relevance_score,1.0)::float8 AS relevance_score,
                  created_at, accessed_at,
                  category, topic
           FROM memory_chunks
           ORDER BY accessed_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await?;

    Ok(rows.iter().map(row_to_memory_chunk).collect())
}

// ── Delete / Rebuild ─────────────────────────────────────────────────────────

/// Rebuild all tsv columns with the given FTS language.
pub async fn rebuild_fts(db: &PgPool, lang: &str) -> Result<u64> {
    validate_fts_lang(lang)?;
    // SAFETY: `lang` is validated by validate_fts_lang() which only allows lowercase ASCII
    // letters. Not user input -- comes from server config.
    let sql = format!(
        "UPDATE memory_chunks SET tsv = to_tsvector('{lang}', content)"
    );
    let res = sqlx::query(&sql)
        .execute(db)
        .await
        .context("failed to rebuild FTS index")?;
    Ok(res.rows_affected())
}

/// Delete a memory chunk and its children (if it's a parent of a chunked document).
pub async fn delete_chunk(db: &PgPool, chunk_id: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM memory_chunks WHERE id = $1::uuid OR parent_id = $1::uuid")
        .bind(chunk_id)
        .execute(db)
        .await
        .context("failed to delete memory chunk")?;
    Ok(res.rows_affected() > 0)
}

