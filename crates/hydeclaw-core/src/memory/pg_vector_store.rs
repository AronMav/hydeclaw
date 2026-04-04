/// PgVectorStore: pgvector-backed implementation of the VectorStore trait.
///
/// All SQL queries are delegated to `crate::db::memory_queries`.
use anyhow::Result;
use async_trait::async_trait;
use sqlx::PgPool;

use super::vector_store::VectorStore;
use super::{MemoryChunk, MemoryResult};

pub struct PgVectorStore {
    db: PgPool,
}

impl PgVectorStore {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

#[async_trait]
#[allow(clippy::too_many_arguments)]
impl VectorStore for PgVectorStore {
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
    ) -> Result<()> {
        crate::db::memory_queries::insert_chunk(
            &self.db, id, content, vec_str, source, pinned, lang, parent_id, chunk_index,
        )
        .await
    }

    async fn search_semantic(
        &self,
        vec_str: &str,
        candidate_limit: i64,
    ) -> Result<Vec<MemoryResult>> {
        crate::db::memory_queries::search_semantic(&self.db, vec_str, candidate_limit).await
    }

    async fn search_fts(
        &self,
        query: &str,
        limit: i64,
        lang: &str,
    ) -> Result<Vec<MemoryResult>> {
        crate::db::memory_queries::search_fts(&self.db, query, limit, lang).await
    }

    async fn touch_accessed(&self, ids: &[uuid::Uuid]) {
        crate::db::memory_queries::touch_accessed(&self.db, ids).await
    }

    async fn fetch_recent(&self, limit: i64) -> Result<Vec<MemoryResult>> {
        crate::db::memory_queries::fetch_recent(&self.db, limit).await
    }

    async fn get_chunk_by_id(&self, id: &str) -> Result<Vec<MemoryChunk>> {
        crate::db::memory_queries::get_chunk_by_id(&self.db, id).await
    }

    async fn get_chunks_by_source(&self, source: &str, limit: i64) -> Result<Vec<MemoryChunk>> {
        crate::db::memory_queries::get_chunks_by_source(&self.db, source, limit).await
    }

    async fn get_chunks_recent(&self, limit: i64) -> Result<Vec<MemoryChunk>> {
        crate::db::memory_queries::get_chunks_recent(&self.db, limit).await
    }

    async fn rebuild_fts(&self, lang: &str) -> Result<u64> {
        crate::db::memory_queries::rebuild_fts(&self.db, lang).await
    }

    async fn delete_chunk(&self, chunk_id: &str) -> Result<bool> {
        crate::db::memory_queries::delete_chunk(&self.db, chunk_id).await
    }

    async fn delete_by_source(&self, source: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM memory_chunks WHERE source = $1")
            .bind(source)
            .execute(&self.db)
            .await?;
        Ok(result.rows_affected())
    }

    async fn get_existing_embedding_dim(&self) -> Option<i32> {
        crate::db::memory_queries::get_existing_embedding_dim(&self.db).await
    }

    async fn clear_embeddings(&self) -> Result<()> {
        crate::db::memory_queries::clear_embeddings(&self.db).await
    }

    async fn drop_hnsw_index(&self) -> Result<()> {
        crate::db::memory_queries::drop_hnsw_index(&self.db).await
    }

    async fn ensure_hnsw_index(&self, dim: u32) -> Result<()> {
        crate::db::memory_queries::ensure_hnsw_index(&self.db, dim).await
    }

    async fn promote_frequent_memories(
        &self,
        recall_threshold: i32,
        lookback_days: i32,
    ) -> Result<u64> {
        crate::db::memory_queries::promote_frequent_memories(&self.db, recall_threshold, lookback_days).await
    }
}
