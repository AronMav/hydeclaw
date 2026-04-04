/// VectorStore trait: abstracts pgvector-specific operations behind an interface.
///
/// Implementations handle embedding storage, semantic search, and chunk lifecycle.
/// The default implementation is PgVectorStore (pgvector + PostgreSQL).
use anyhow::Result;
use async_trait::async_trait;

use super::{MemoryChunk, MemoryResult};

#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait VectorStore: Send + Sync {
    /// Insert a new memory chunk with its embedding vector and FTS tsvector.
    async fn insert_chunk(
        &self,
        id: &str,
        content: &str,
        vec_str: &str,
        source: &str,
        pinned: bool,
        lang: &str,
        parent_id: Option<&str>,
        chunk_index: i32,
    ) -> Result<()>;

    /// Semantic similarity search: find nearest chunks by embedding cosine distance.
    async fn search_semantic(
        &self,
        vec_str: &str,
        candidate_limit: i64,
    ) -> Result<Vec<MemoryResult>>;

    /// Full-text search using PostgreSQL tsvector/tsquery.
    async fn search_fts(
        &self,
        query: &str,
        limit: i64,
        lang: &str,
    ) -> Result<Vec<MemoryResult>>;

    /// Update accessed_at timestamp for the given chunk IDs.
    async fn touch_accessed(&self, ids: &[uuid::Uuid]);

    /// Return the most-recently-accessed memory chunks (pinned first).
    async fn fetch_recent(&self, limit: i64) -> Result<Vec<MemoryResult>>;

    /// Retrieve a single chunk by ID.
    async fn get_chunk_by_id(&self, id: &str) -> Result<Vec<MemoryChunk>>;

    /// Retrieve chunks by source, ordered by creation date.
    async fn get_chunks_by_source(&self, source: &str, limit: i64) -> Result<Vec<MemoryChunk>>;

    /// Retrieve most recently accessed chunks.
    async fn get_chunks_recent(&self, limit: i64) -> Result<Vec<MemoryChunk>>;

    /// Rebuild all tsv columns with the given FTS language.
    async fn rebuild_fts(&self, lang: &str) -> Result<u64>;

    /// Delete a memory chunk and its children by UUID.
    async fn delete_chunk(&self, chunk_id: &str) -> Result<bool>;

    /// Delete all chunks matching a given source.
    async fn delete_by_source(&self, source: &str) -> Result<u64>;

    // ── Initialization helpers ────────────────────────────────────────────────

    /// Check the dimension of existing embeddings in the database.
    async fn get_existing_embedding_dim(&self) -> Option<i32>;

    /// Delete all memory chunks that have embeddings (dimension mismatch cleanup).
    async fn clear_embeddings(&self) -> Result<()>;

    /// Drop the HNSW embedding index.
    async fn drop_hnsw_index(&self) -> Result<()>;

    /// Create HNSW index if it doesn't exist.
    async fn ensure_hnsw_index(&self, dim: u32) -> Result<()>;

    // ── Dreaming ──────────────────────────────────────────────────────────────

    /// Promote frequently-recalled raw memories to pinned tier.
    /// Returns count of promoted chunks.
    async fn promote_frequent_memories(
        &self,
        recall_threshold: i32,
        lookback_days: i32,
    ) -> Result<u64>;
}
