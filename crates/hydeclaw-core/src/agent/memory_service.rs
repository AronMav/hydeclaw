/// `MemoryService` trait — abstraction over the concrete `MemoryStore` for testability.
///
/// Engine holds `Arc<dyn MemoryService>` instead of `Arc<MemoryStore>` so unit
/// tests can inject a `MockMemoryService` without needing a live `PostgreSQL` + pgvector stack.
use anyhow::Result;
use async_trait::async_trait;

/// Abstraction over the native memory store.
///
/// All async methods mirror the public API of `crate::memory::MemoryStore`.
/// The `search` method uses `String` for the mode (instead of `&'static str`) to
/// allow object-safe trait dispatch via `Arc<dyn MemoryService>`.
#[async_trait]
pub trait MemoryService: Send + Sync {
    /// Returns true when embedding is enabled and endpoint is configured.
    fn is_available(&self) -> bool;

    /// Generate an embedding vector for `text`.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Hybrid search (semantic + FTS). Returns results and search mode string.
    async fn search(
        &self,
        query: &str,
        limit: usize,
        exclude_ids: &[String],
        category: Option<&str>,
        topic: Option<&str>,
    ) -> Result<(Vec<crate::memory::MemoryResult>, String)>;

    /// Index a new memory chunk. Returns the new chunk UUID.
    async fn index(
        &self,
        content: &str,
        source: &str,
        pinned: bool,
        category: Option<&str>,
        topic: Option<&str>,
    ) -> Result<String>;

    /// Batch-index memory chunks. Returns a vec of new chunk UUIDs.
    async fn index_batch(&self, items: &[(String, String, bool)]) -> Result<Vec<String>>;

    /// Load pinned memory chunks formatted for context injection.
    /// Returns (formatted text, list of chunk IDs).
    async fn load_pinned(
        &self,
        agent_id: &str,
        budget_tokens: u32,
    ) -> Result<(String, Vec<String>)>;

    /// Fetch memory chunks by id or source. Returns raw chunk records.
    async fn get(
        &self,
        chunk_id: Option<&str>,
        source: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::memory::MemoryChunk>>;

    /// Delete a memory chunk by UUID. Returns true if a row was deleted.
    async fn delete(&self, chunk_id: &str) -> Result<bool>;

    /// Return the N most recently created chunks.
    async fn recent(&self, limit: i64) -> Result<Vec<crate::memory::MemoryResult>>;

    /// Wipe all memory for an agent: graph episodes, orphaned edges/entities, then memory chunks.
    /// Returns the number of memory chunks deleted.
    async fn wipe_agent_memory(&self, agent_id: &str) -> Result<u64>;

    /// Insert a reindex task into the memory worker queue.
    /// Returns the task UUID.
    async fn enqueue_reindex_task(&self, params: serde_json::Value) -> Result<uuid::Uuid>;
}

// ── MemoryStore impl ─────────────────────────────────────────────────────────

#[async_trait]
impl MemoryService for crate::memory::MemoryStore {
    fn is_available(&self) -> bool {
        self.is_available()
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.embed(text).await
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
        exclude_ids: &[String],
        category: Option<&str>,
        topic: Option<&str>,
    ) -> Result<(Vec<crate::memory::MemoryResult>, String)> {
        let (results, mode) = self.search(query, limit, exclude_ids, category, topic).await?;
        Ok((results, mode.to_string()))
    }

    async fn index(
        &self,
        content: &str,
        source: &str,
        pinned: bool,
        category: Option<&str>,
        topic: Option<&str>,
    ) -> Result<String> {
        self.index(content, source, pinned, category, topic).await
    }

    async fn index_batch(&self, items: &[(String, String, bool)]) -> Result<Vec<String>> {
        self.index_batch(items).await
    }

    async fn load_pinned(
        &self,
        agent_id: &str,
        budget_tokens: u32,
    ) -> Result<(String, Vec<String>)> {
        self.load_pinned(agent_id, budget_tokens).await
    }

    async fn get(
        &self,
        chunk_id: Option<&str>,
        source: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::memory::MemoryChunk>> {
        self.get(chunk_id, source, limit).await
    }

    async fn delete(&self, chunk_id: &str) -> Result<bool> {
        self.delete(chunk_id).await
    }

    async fn recent(&self, limit: i64) -> Result<Vec<crate::memory::MemoryResult>> {
        self.recent(limit).await
    }

    async fn wipe_agent_memory(&self, agent_id: &str) -> Result<u64> {
        self.wipe_agent_memory(agent_id).await
    }

    async fn enqueue_reindex_task(&self, params: serde_json::Value) -> Result<uuid::Uuid> {
        self.enqueue_reindex_task(params).await
    }
}

// ── Mock (test only) ─────────────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::*;

    /// Stub MemoryService for unit tests. No database or network required.
    pub struct MockMemoryService {
        pub available: bool,
    }

    impl MockMemoryService {
        pub fn available() -> Self {
            Self { available: true }
        }

        pub fn unavailable() -> Self {
            Self { available: false }
        }
    }

    #[async_trait]
    impl MemoryService for MockMemoryService {
        fn is_available(&self) -> bool {
            self.available
        }

        async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            // Fixed-dimension stub vector (dim=4)
            Ok(vec![0.1, 0.2, 0.3, 0.4])
        }

        async fn search(
            &self,
            _query: &str,
            _limit: usize,
            _exclude_ids: &[String],
            _category: Option<&str>,
            _topic: Option<&str>,
        ) -> Result<(Vec<crate::memory::MemoryResult>, String)> {
            Ok((vec![], "mock".to_string()))
        }

        async fn index(
            &self,
            _content: &str,
            _source: &str,
            _pinned: bool,
            _category: Option<&str>,
            _topic: Option<&str>,
        ) -> Result<String> {
            Ok("mock-chunk-id".to_string())
        }

        async fn index_batch(
            &self,
            items: &[(String, String, bool)],
        ) -> Result<Vec<String>> {
            Ok(items.iter().map(|_| "mock-chunk-id".to_string()).collect())
        }

        async fn load_pinned(
            &self,
            _agent_id: &str,
            _budget_tokens: u32,
        ) -> Result<(String, Vec<String>)> {
            Ok((String::new(), vec![]))
        }

        async fn get(
            &self,
            _chunk_id: Option<&str>,
            _source: Option<&str>,
            _limit: usize,
        ) -> Result<Vec<crate::memory::MemoryChunk>> {
            Ok(vec![])
        }

        async fn delete(&self, _chunk_id: &str) -> Result<bool> {
            Ok(false)
        }

        async fn recent(&self, _limit: i64) -> Result<Vec<crate::memory::MemoryResult>> {
            Ok(vec![])
        }

        async fn wipe_agent_memory(&self, _agent_id: &str) -> Result<u64> {
            Ok(0)
        }

        async fn enqueue_reindex_task(&self, _params: serde_json::Value) -> Result<uuid::Uuid> {
            Ok(uuid::Uuid::nil())
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::mock::MockMemoryService;
    use super::MemoryService;
    use std::sync::Arc;

    #[test]
    fn mock_is_available_true() {
        let mock = MockMemoryService::available();
        assert!(mock.is_available());
    }

    #[test]
    fn mock_is_available_false() {
        let mock = MockMemoryService::unavailable();
        assert!(!mock.is_available());
    }

    #[tokio::test]
    async fn mock_search_returns_empty_without_db() {
        let mock = MockMemoryService::available();
        let (results, mode) = mock.search("test query", 5, &[], None, None).await.unwrap();
        assert!(results.is_empty());
        assert_eq!(mode, "mock");
    }

    #[tokio::test]
    async fn mock_embed_returns_fixed_vector_without_db() {
        let mock = MockMemoryService::available();
        let v = mock.embed("some text").await.unwrap();
        assert_eq!(v.len(), 4);
        assert!((v[0] - 0.1).abs() < 1e-6);
    }

    #[tokio::test]
    async fn mock_recent_returns_empty_without_db() {
        let mock = MockMemoryService::available();
        let results = mock.recent(10).await.unwrap();
        assert!(results.is_empty());
    }

    /// Verify that Arc<dyn MemoryService> dispatch works (trait is object-safe).
    #[tokio::test]
    async fn trait_object_dispatch_works() {
        let svc: Arc<dyn MemoryService> = Arc::new(MockMemoryService::available());
        assert!(svc.is_available());
        let v = svc.embed("hello").await.unwrap();
        assert!(!v.is_empty());
    }
}
