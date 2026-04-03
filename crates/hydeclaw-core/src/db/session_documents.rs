use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

/// Insert a document chunk with embedding into session-scoped storage.
pub async fn insert_chunk(
    db: &PgPool,
    session_id: Uuid,
    filename: &str,
    content: &str,
    chunk_index: i32,
    embedding_vec: &str,
) -> Result<Uuid> {
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO session_documents (session_id, filename, content, chunk_index, embedding) \
         VALUES ($1, $2, $3, $4, $5::halfvec) RETURNING id",
    )
    .bind(session_id)
    .bind(filename)
    .bind(content)
    .bind(chunk_index)
    .bind(embedding_vec)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

/// Search session documents by vector similarity.
pub async fn search(
    db: &PgPool,
    session_id: Uuid,
    query_vec: &str,
    limit: i64,
) -> Result<Vec<(String, String, f64)>> {
    let rows = sqlx::query_as::<_, (String, String, f64)>(
        "SELECT filename, content, \
         1 - (embedding <=> $2::halfvec) AS score \
         FROM session_documents \
         WHERE session_id = $1 AND embedding IS NOT NULL \
         ORDER BY embedding <=> $2::halfvec \
         LIMIT $3",
    )
    .bind(session_id)
    .bind(query_vec)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

