# MemoryStore Decomposition — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose the 1258-line `memory.rs` god-object into 4 focused modules: `EmbeddingService` trait + `ToolgateEmbedder`, slimmer `MemoryStore`, `MemoryAdmin`, and `workspace watcher`.

**Architecture:** Extract embedding HTTP client into `memory/embedding.rs` behind an `EmbeddingService` trait. Move search/index logic into `memory/store.rs` which takes `Arc<dyn EmbeddingService>`. Move admin ops (FTS, wipe, reindex) into `memory/admin.rs`. Move workspace watcher into `memory/watcher.rs`. Update `MemoryService` trait (remove embed methods), add `embedder` field to `InfraServices`, update all consumers.

**Tech Stack:** Rust 2024, async-trait, sqlx 0.8, reqwest (rustls-tls), tokio.

---

## File Map

**Create:**
- `crates/hydeclaw-core/src/memory/mod.rs`
- `crates/hydeclaw-core/src/memory/embedding.rs`
- `crates/hydeclaw-core/src/memory/store.rs`
- `crates/hydeclaw-core/src/memory/admin.rs`
- `crates/hydeclaw-core/src/memory/watcher.rs`

**Modify:**
- `crates/hydeclaw-core/src/agent/memory_service.rs` — update trait, mock
- `crates/hydeclaw-core/src/gateway/clusters/infra_services.rs` — add `embedder` field
- `crates/hydeclaw-core/src/main.rs` — new construction
- `crates/hydeclaw-core/src/db/memory_queries.rs` — add 3 moved SQL functions
- `crates/hydeclaw-core/src/agent/engine_subagent.rs` — `embed` → `embedder.embed`
- `crates/hydeclaw-core/src/agent/engine_parallel.rs` — `embed` → `embedder.embed`
- `crates/hydeclaw-core/src/gateway/handlers/memory.rs` — use `embedder` + `MemoryAdmin`
- `crates/hydeclaw-core/src/gateway/handlers/chat.rs` — use `embedder`
- `crates/hydeclaw-core/src/gateway/handlers/config.rs` — use `embedder`

**Delete:**
- `crates/hydeclaw-core/src/memory.rs` (replaced by `memory/` directory)

---

## Task 1: Create memory/ directory scaffold

**Files:**
- Create: `crates/hydeclaw-core/src/memory/mod.rs`

- [ ] **Step 1: Create memory directory**

```bash
mkdir -p crates/hydeclaw-core/src/memory
```

- [ ] **Step 2: Create mod.rs with re-exports from old memory.rs**

Create `crates/hydeclaw-core/src/memory/mod.rs`:

```rust
pub mod embedding;
pub mod store;
pub mod admin;
pub mod watcher;

// Re-export types that consumers import from crate::memory::*
pub use store::MemoryStore;
pub use admin::MemoryAdmin;
pub use embedding::{EmbeddingService, ToolgateEmbedder, fmt_vec};

// Types stay here (moved from old memory.rs)
use chrono::{DateTime, Utc};

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default, schemars::JsonSchema)]
pub struct MemoryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub embed_dim: Option<u32>,
    pub embed_dimensions: Option<u32>,
    pub fts_language: Option<String>,
    #[serde(default = "default_pinned_budget")]
    pub pinned_budget_tokens: u32,
    #[serde(default = "default_compression_age_days")]
    pub compression_age_days: u32,
}

use crate::config::default_true;

fn default_pinned_budget() -> u32 { 2000 }
fn default_compression_age_days() -> u32 { 30 }

// ── Types ─────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct MemoryResult {
    pub id: String,
    pub content: String,
    pub source: String,
    pub pinned: bool,
    pub relevance_score: f64,
    pub similarity: f64,
    pub parent_id: Option<String>,
    pub chunk_index: i32,
    pub category: Option<String>,
    pub topic: Option<String>,
}

#[allow(dead_code)]
pub struct MemoryChunk {
    pub id: String,
    pub content: String,
    pub source: String,
    pub pinned: bool,
    pub relevance_score: f64,
    pub created_at: DateTime<Utc>,
    pub accessed_at: DateTime<Utc>,
    pub category: Option<String>,
    pub topic: Option<String>,
}
```

- [ ] **Step 3: Create placeholder files for each submodule**

Create `crates/hydeclaw-core/src/memory/embedding.rs`:
```rust
// populated in Task 2
```

Create `crates/hydeclaw-core/src/memory/store.rs`:
```rust
// populated in Task 3
```

Create `crates/hydeclaw-core/src/memory/admin.rs`:
```rust
// populated in Task 4
```

Create `crates/hydeclaw-core/src/memory/watcher.rs`:
```rust
// populated in Task 5
```

- [ ] **Step 4: Delete old memory.rs and verify module swap**

Delete `crates/hydeclaw-core/src/memory.rs`. The `mod memory;` in `lib.rs` or `main.rs` now resolves to the `memory/` directory.

Run:
```bash
cargo check -p hydeclaw-core 2>&1 | head -20
```

Expected: compile errors about missing items (embedding, store, admin functions) — that is expected for now.

- [ ] **Step 5: Commit**

```bash
git add -A crates/hydeclaw-core/src/memory/ && git rm crates/hydeclaw-core/src/memory.rs
git commit -m "refactor: replace memory.rs with memory/ directory scaffold"
```

---

## Task 2: EmbeddingService trait + ToolgateEmbedder

**Files:**
- Create: `crates/hydeclaw-core/src/memory/embedding.rs`

- [ ] **Step 1: Write the failing test**

Add at the bottom of `embedding.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_vec_empty() {
        assert_eq!(fmt_vec(&[]), "[]");
    }

    #[test]
    fn fmt_vec_multiple() {
        assert_eq!(fmt_vec(&[1.0, 2.5, -3.0]), "[1,2.5,-3]");
    }

    #[test]
    fn fmt_vec_no_spaces() {
        let result = fmt_vec(&[0.1, 0.2, 0.3]);
        assert!(!result.contains(' '), "fmt_vec must not contain spaces: {result}");
    }

    #[test]
    fn test_unavailable_when_no_url() {
        let embedder = ToolgateEmbedder::new_disabled();
        assert!(!embedder.is_available());
        assert_eq!(embedder.embed_dim(), 0);
        assert!(embedder.embed_model_name().is_none());
    }
}
```

- [ ] **Step 2: Run test — expect compile error**

```bash
cargo test -p hydeclaw-core embedding 2>&1 | head -20
```

Expected: error — `EmbeddingService`, `ToolgateEmbedder`, `fmt_vec` not defined.

- [ ] **Step 3: Implement EmbeddingService trait and ToolgateEmbedder**

Replace `embedding.rs` content with the full implementation. Copy from old `memory.rs` the following methods and adapt them:

```rust
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use tokio::sync::OnceCell;
use sqlx::PgPool;

// ── Trait ─────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait EmbeddingService: Send + Sync {
    fn is_available(&self) -> bool;
    fn embed_dim(&self) -> u32;
    fn embed_model_name(&self) -> Option<String>;
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

// ── ToolgateEmbedder ─────────────────────────────────────────────────────────

pub struct ToolgateEmbedder {
    db: PgPool,
    http: reqwest::Client,
    embed_url: String,
    embed_model: OnceLock<String>,
    embed_dim: AtomicU32,
    embed_dimensions: u32,
    initialized: OnceCell<()>,
}

impl ToolgateEmbedder {
    pub fn new(db: PgPool, toolgate_url: Option<&str>, embed_dim: u32, embed_dimensions: u32) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        let embed_url = match toolgate_url {
            Some(url) if !url.is_empty() => format!("{}/v1", url.trim_end_matches('/')),
            _ => String::new(),
        };
        Self {
            db,
            http,
            embed_url,
            embed_model: OnceLock::new(),
            embed_dim: AtomicU32::new(embed_dim),
            embed_dimensions,
            initialized: OnceCell::new(),
        }
    }

    /// Create a disabled embedder for tests (is_available() = false).
    pub fn new_disabled() -> Self {
        Self {
            db: PgPool::connect_lazy("postgres://invalid").expect("lazy pool"),
            http: reqwest::Client::new(),
            embed_url: String::new(),
            embed_model: OnceLock::new(),
            embed_dim: AtomicU32::new(0),
            embed_dimensions: 0,
            initialized: OnceCell::new(),
        }
    }

    // Copy from old memory.rs (lines 184-275): do_initialize, ensure_initialized,
    // fetch_embed_model_from_toolgate, ensure_index — adjusting self references.
    // These are private methods, copy them verbatim.

    async fn do_initialize(&self) -> Result<()> {
        // ... (copy from old memory.rs lines 210-266)
    }

    async fn ensure_initialized(&self) {
        // ... (copy from old memory.rs lines 269-275)
    }

    async fn fetch_embed_model_from_toolgate(&self) {
        // ... (copy from old memory.rs lines 184-205)
    }

    async fn ensure_index(&self, dim: u32) -> Result<()> {
        crate::db::memory_queries::ensure_hnsw_index(&self.db, dim).await
    }
}

#[async_trait::async_trait]
impl EmbeddingService for ToolgateEmbedder {
    fn is_available(&self) -> bool {
        !self.embed_url.is_empty()
    }

    fn embed_dim(&self) -> u32 {
        self.embed_dim.load(Ordering::Relaxed)
    }

    fn embed_model_name(&self) -> Option<String> {
        self.embed_model.get().cloned()
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.ensure_initialized().await;
        // ... (copy from old memory.rs lines 287-337, removing self.enabled check)
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        // ... (copy from old memory.rs lines 341-411)
    }
}

// ── Helper ───────────────────────────────────────────────────────────────────

/// Format a float vector as a pgvector literal: "[0.1,0.2,...]"
pub fn fmt_vec(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 10 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 { s.push(','); }
        s.push_str(&x.to_string());
    }
    s.push(']');
    s
}

// ── FakeEmbedder for tests ───────────────────────────────────────────────────

#[cfg(test)]
pub struct FakeEmbedder {
    pub available: bool,
    pub dim: u32,
}

#[cfg(test)]
#[async_trait::async_trait]
impl EmbeddingService for FakeEmbedder {
    fn is_available(&self) -> bool { self.available }
    fn embed_dim(&self) -> u32 { self.dim }
    fn embed_model_name(&self) -> Option<String> { Some("fake".into()) }
    async fn embed(&self, _: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; self.dim as usize])
    }
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0; self.dim as usize]).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_vec_empty() {
        assert_eq!(fmt_vec(&[]), "[]");
    }

    #[test]
    fn fmt_vec_multiple() {
        assert_eq!(fmt_vec(&[1.0, 2.5, -3.0]), "[1,2.5,-3]");
    }

    #[test]
    fn fmt_vec_no_spaces() {
        let result = fmt_vec(&[0.1, 0.2, 0.3]);
        assert!(!result.contains(' '), "fmt_vec must not contain spaces: {result}");
    }

    #[test]
    fn test_unavailable_when_no_url() {
        let embedder = ToolgateEmbedder::new_disabled();
        assert!(!embedder.is_available());
        assert_eq!(embedder.embed_dim(), 0);
        assert!(embedder.embed_model_name().is_none());
    }
}
```

> **IMPORTANT:** Copy the actual method bodies from old `memory.rs` exactly. The plan shows the structure; the engineer must copy lines 184-411 from the old file, adjusting `Self::fmt_vec` → `fmt_vec` and removing references to `self.enabled` (availability is now just `!self.embed_url.is_empty()`).

- [ ] **Step 4: Run tests — expect pass**

```bash
cargo test -p hydeclaw-core embedding 2>&1 | tail -5
```

Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/memory/embedding.rs
git commit -m "refactor: extract EmbeddingService trait + ToolgateEmbedder"
```

---

## Task 3: MemoryStore in store.rs

**Files:**
- Create: `crates/hydeclaw-core/src/memory/store.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::embedding::FakeEmbedder;

    #[test]
    fn test_store_delegates_availability() {
        let embedder = Arc::new(FakeEmbedder { available: true, dim: 4 });
        let store = MemoryStore::test_with_embedder(embedder.clone());
        assert!(store.is_available());

        let embedder2 = Arc::new(FakeEmbedder { available: false, dim: 4 });
        let store2 = MemoryStore::test_with_embedder(embedder2);
        assert!(!store2.is_available());
    }
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test -p hydeclaw-core test_store_delegates 2>&1 | head -20
```

- [ ] **Step 3: Implement MemoryStore**

Copy from old `memory.rs` the search/index/get/delete/load_pinned methods. The struct changes from 9 fields to 3:

```rust
use std::sync::{Arc, RwLock};
use anyhow::{Context, Result};
use sqlx::PgPool;
use crate::memory::embedding::{EmbeddingService, fmt_vec};
use crate::memory::{MemoryResult, MemoryChunk};

pub struct MemoryStore {
    db: PgPool,
    embedder: Arc<dyn EmbeddingService>,
    fts_language: RwLock<String>,
}

impl MemoryStore {
    pub fn new(db: PgPool, embedder: Arc<dyn EmbeddingService>, fts_language: &str) -> Self {
        Self {
            db,
            embedder,
            fts_language: RwLock::new(fts_language.to_string()),
        }
    }

    pub fn is_available(&self) -> bool {
        self.embedder.is_available()
    }

    pub fn fts_language(&self) -> String {
        self.fts_language.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn validated_fts_language(&self) -> Result<String> {
        let lang = self.fts_language();
        anyhow::ensure!(
            !lang.is_empty() && lang.chars().all(|c| c.is_ascii_lowercase()),
            "invalid FTS language: {lang}"
        );
        Ok(lang)
    }

    pub fn set_fts_language(&self, lang: &str) {
        *self.fts_language.write().unwrap_or_else(std::sync::PoisonError::into_inner) = lang.to_ascii_lowercase();
    }

    pub fn db(&self) -> &PgPool { &self.db }
    pub fn embedder(&self) -> &Arc<dyn EmbeddingService> { &self.embedder }

    #[cfg(test)]
    pub fn test_with_embedder(embedder: Arc<dyn EmbeddingService>) -> Self {
        Self {
            db: PgPool::connect_lazy("postgres://invalid").expect("lazy pool"),
            embedder,
            fts_language: RwLock::new("simple".to_string()),
        }
    }

    // Copy search, search_hybrid, search_semantic, search_fts from old memory.rs
    // Change: self.embed(x) → self.embedder.embed(x)
    // Change: self.embed_batch(x) → self.embedder.embed_batch(x)
    // Change: Self::fmt_vec(x) → fmt_vec(x)
    // Change: self.ensure_initialized() → remove (embedder handles its own init)

    // Copy index, index_batch from old memory.rs — same substitutions

    // Copy get, recent, delete, load_pinned from old memory.rs — no embed changes needed

    // Copy dedup_by_parent as a private method

    // rebuild_fts stays here (uses self.db + self.validated_fts_language)
}
```

> **IMPORTANT:** The `ensure_initialized()` calls in `search` and `index` are removed — `ToolgateEmbedder` handles its own lazy init internally when `embed()` is called. The `embed_batch` signature in `EmbeddingService` takes `&[&str]` — match the trait exactly.

- [ ] **Step 4: Run tests — expect pass**

```bash
cargo test -p hydeclaw-core test_store_delegates 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/memory/store.rs
git commit -m "refactor: extract MemoryStore into memory/store.rs"
```

---

## Task 4: MemoryAdmin + move inline SQL to db/memory_queries.rs

**Files:**
- Create: `crates/hydeclaw-core/src/memory/admin.rs`
- Modify: `crates/hydeclaw-core/src/db/memory_queries.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_fts_language_known() {
        assert_eq!(MemoryAdmin::detect_fts_language("ru"), "russian");
        assert_eq!(MemoryAdmin::detect_fts_language("en"), "english");
        assert_eq!(MemoryAdmin::detect_fts_language("de"), "german");
    }

    #[test]
    fn detect_fts_language_unknown_fallback() {
        assert_eq!(MemoryAdmin::detect_fts_language("xx"), "simple");
        assert_eq!(MemoryAdmin::detect_fts_language(""), "simple");
    }

    #[test]
    fn validated_fts_rejects_injection() {
        assert!(MemoryAdmin::validated_fts_language("russian").is_ok());
        assert!(MemoryAdmin::validated_fts_language("Robert'; DROP TABLE--").is_err());
        assert!(MemoryAdmin::validated_fts_language("").is_err());
    }
}
```

- [ ] **Step 2: Run — expect compile error**

```bash
cargo test -p hydeclaw-core detect_fts 2>&1 | head -20
```

- [ ] **Step 3: Add 3 SQL functions to db/memory_queries.rs**

At the end of `crates/hydeclaw-core/src/db/memory_queries.rs`, add:

```rust
/// Delete all memory chunks with a given source path.
pub async fn delete_by_source(db: &PgPool, source: &str) -> anyhow::Result<u64> {
    let result = sqlx::query("DELETE FROM memory_chunks WHERE source = $1")
        .bind(source)
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

/// Delete all memory chunks for an agent.
pub async fn wipe_agent_memory(db: &PgPool, agent_id: &str) -> anyhow::Result<u64> {
    let result = sqlx::query("DELETE FROM memory_chunks WHERE agent_id = $1")
        .bind(agent_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

/// Enqueue a reindex task for the memory worker.
pub async fn enqueue_reindex_task(db: &PgPool, params: serde_json::Value) -> anyhow::Result<uuid::Uuid> {
    sqlx::query_scalar(
        "INSERT INTO memory_tasks (task_type, params) VALUES ('reindex', $1) RETURNING id",
    )
    .bind(params)
    .fetch_one(db)
    .await
    .map_err(Into::into)
}
```

- [ ] **Step 4: Implement MemoryAdmin**

```rust
use anyhow::Result;
use sqlx::PgPool;

pub struct MemoryAdmin {
    db: PgPool,
}

impl MemoryAdmin {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// BCP-47 language code → PostgreSQL FTS dictionary name.
    pub fn detect_fts_language(agent_lang: &str) -> String {
        match agent_lang {
            "ru" => "russian",
            "en" => "english",
            "es" => "spanish",
            "de" => "german",
            "fr" => "french",
            "pt" => "portuguese",
            "it" => "italian",
            "nl" => "dutch",
            "sv" => "swedish",
            "no" | "nb" => "norwegian",
            "da" => "danish",
            "fi" => "finnish",
            "hu" => "hungarian",
            "ro" => "romanian",
            "tr" => "turkish",
            _ => "simple",
        }.to_string()
    }

    /// Validate FTS language is safe for SQL interpolation.
    pub fn validated_fts_language(lang: &str) -> Result<String> {
        anyhow::ensure!(
            !lang.is_empty() && lang.chars().all(|c| c.is_ascii_lowercase()),
            "invalid FTS language: {lang}"
        );
        Ok(lang.to_string())
    }

    pub async fn rebuild_fts(&self, fts_language: &str) -> Result<u64> {
        let lang = Self::validated_fts_language(fts_language)?;
        let rows = crate::db::memory_queries::rebuild_fts(&self.db, &lang).await?;
        tracing::info!(lang = %lang, rows, "FTS index rebuilt");
        Ok(rows)
    }

    pub async fn delete_by_source(&self, source: &str) -> Result<u64> {
        crate::db::memory_queries::delete_by_source(&self.db, source).await
    }

    pub async fn wipe_agent_memory(&self, agent_id: &str) -> Result<u64> {
        crate::db::memory_queries::wipe_agent_memory(&self.db, agent_id).await
    }

    pub async fn enqueue_reindex_task(&self, params: serde_json::Value) -> Result<uuid::Uuid> {
        crate::db::memory_queries::enqueue_reindex_task(&self.db, params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_fts_language_known() {
        assert_eq!(MemoryAdmin::detect_fts_language("ru"), "russian");
        assert_eq!(MemoryAdmin::detect_fts_language("en"), "english");
        assert_eq!(MemoryAdmin::detect_fts_language("de"), "german");
    }

    #[test]
    fn detect_fts_language_unknown_fallback() {
        assert_eq!(MemoryAdmin::detect_fts_language("xx"), "simple");
        assert_eq!(MemoryAdmin::detect_fts_language(""), "simple");
    }

    #[test]
    fn validated_fts_rejects_injection() {
        assert!(MemoryAdmin::validated_fts_language("russian").is_ok());
        assert!(MemoryAdmin::validated_fts_language("Robert'; DROP TABLE--").is_err());
        assert!(MemoryAdmin::validated_fts_language("").is_err());
    }
}
```

- [ ] **Step 5: Run tests — expect pass**

```bash
cargo test -p hydeclaw-core detect_fts validated_fts 2>&1 | tail -5
```

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/memory/admin.rs crates/hydeclaw-core/src/db/memory_queries.rs
git commit -m "refactor: extract MemoryAdmin + move inline SQL to db/memory_queries"
```

---

## Task 5: Workspace Watcher

**Files:**
- Create: `crates/hydeclaw-core/src/memory/watcher.rs`

- [ ] **Step 1: Move spawn_workspace_watcher from old memory.rs**

Copy the `spawn_workspace_watcher` function (lines 865-966 of old memory.rs) into `watcher.rs`. Change:
- `mem.delete_by_source(&source)` → `crate::db::memory_queries::delete_by_source(&mem.db(), &source)` (or keep using `mem.delete_by_source` if MemoryStore still has it via delegation)
- Add `use crate::memory::MemoryStore;`

```rust
use std::sync::Arc;
use crate::memory::MemoryStore;

/// Watch workspace directory for .md/.txt file changes and auto-index into memory.
pub fn spawn_workspace_watcher(
    workspace_dir: String,
    memory: Arc<MemoryStore>,
    handle: tokio::runtime::Handle,
) {
    // ... (copy lines 870-966 from old memory.rs verbatim)
    // Change: mem.delete_by_source(&source) calls db::memory_queries::delete_by_source
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p hydeclaw-core 2>&1 | head -20
```

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/memory/watcher.rs
git commit -m "refactor: move spawn_workspace_watcher to memory/watcher.rs"
```

---

## Task 6: Update MemoryService trait

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/memory_service.rs`

- [ ] **Step 1: Remove embed methods from the trait**

In `memory_service.rs`, remove these methods from the `MemoryService` trait:
- `async fn embed(&self, text: &str) -> Result<Vec<f32>>` (line 19)
- `async fn embed_batch(&self, texts: &[&str]) -> ...` (lines 103-110, defaulted method)
- `fn embed_dim(&self) -> u32` (line 86, defaulted)
- `fn embed_model_name(&self) -> String` (line 89, defaulted)

Keep `is_available()` — MemoryStore delegates to embedder.

FTS methods (`fts_language`, `validated_fts_language`, `set_fts_language`, `rebuild_fts`) remain on the trait for now — handlers use them through `Arc<dyn MemoryService>`.

- [ ] **Step 2: Update impl MemoryService for MemoryStore**

The `impl MemoryService for crate::memory::MemoryStore` block — remove the embed/embed_batch/embed_dim/embed_model_name delegations.

Update the `use` path if needed: `crate::memory::MemoryStore` stays the same since `mod.rs` re-exports it.

- [ ] **Step 3: Update MockMemoryService**

Remove `embed` method from `MockMemoryService`. Remove `embed_dim`, `embed_model_name` if they were overridden.

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p hydeclaw-core 2>&1 | grep "^error" | wc -l
```

Expected: errors in files that call `memory_store.embed()` — those will be fixed in Tasks 8-9.

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/agent/memory_service.rs
git commit -m "refactor: remove embed methods from MemoryService trait"
```

---

## Task 7: Add embedder to InfraServices + main.rs construction

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/clusters/infra_services.rs`
- Modify: `crates/hydeclaw-core/src/main.rs`

- [ ] **Step 1: Add embedder field to InfraServices**

In `infra_services.rs`, add:

```rust
use crate::memory::EmbeddingService;
```

Add field to struct:
```rust
pub embedder: Arc<dyn EmbeddingService>,
```

Update `new()` constructor to accept the new field.

Update `test_with_memory()` to also accept/create a fake embedder:
```rust
#[cfg(test)]
pub fn test_with_memory(memory: impl MemoryService + 'static) -> Self {
    use crate::memory::embedding::FakeEmbedder;
    Self {
        db: PgPool::connect_lazy("postgres://invalid").expect("lazy pool"),
        memory_store: Arc::new(memory),
        embedder: Arc::new(FakeEmbedder { available: false, dim: 0 }),
        container_manager: None,
        sandbox: None,
        process_manager: None,
    }
}
```

- [ ] **Step 2: Update main.rs construction**

In `main.rs`, find where `MemoryStore::new(...)` is constructed. Change to:

```rust
// Create embedder first
let embedder: Arc<dyn memory::EmbeddingService> = Arc::new(
    memory::ToolgateEmbedder::new(
        db_pool.clone(),
        toolgate_url.as_deref(),
        memory_config.embed_dim.unwrap_or(0),
        memory_config.embed_dimensions.unwrap_or(0),
    )
);

// Create memory store with embedder
let memory_store: Arc<memory::MemoryStore> = Arc::new(
    memory::MemoryStore::new(
        db_pool.clone(),
        embedder.clone(),
        &memory_config.fts_language.clone().unwrap_or_else(|| "simple".to_string()),
    )
);
```

Update the `InfraServices::new(...)` call to pass `embedder.clone()`.

- [ ] **Step 3: Verify compilation progress**

```bash
cargo check -p hydeclaw-core 2>&1 | grep "^error" | wc -l
```

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/clusters/infra_services.rs crates/hydeclaw-core/src/main.rs
git commit -m "refactor: add embedder field to InfraServices + update main.rs"
```

---

## Task 8: Migrate engine files (embed → embedder)

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine_subagent.rs`
- Modify: `crates/hydeclaw-core/src/agent/engine_parallel.rs`
- Modify: any other engine file calling `memory_store.embed()`

- [ ] **Step 1: Find all embed calls on memory_store in engine files**

```bash
grep -rn "memory_store\.embed\|memory_store\.is_available\|memory_store\.embed_dim\|memory_store\.embed_model" \
  crates/hydeclaw-core/src/agent/ --include="*.rs"
```

- [ ] **Step 2: Add embedder field to AgentEngine**

If AgentEngine has a `memory_store` field, it needs an `embedder` field too. Check `crates/hydeclaw-core/src/agent/engine.rs` for the struct definition and add:

```rust
pub(crate) embedder: Arc<dyn crate::memory::EmbeddingService>,
```

Update the constructor to accept it. Update all call sites that create AgentEngine (in `lifecycle.rs`).

- [ ] **Step 3: Replace embed calls**

In each engine file:
- `self.memory_store.embed(x)` → `self.embedder.embed(x)`
- `self.memory_store.embed_batch(x)` → `self.embedder.embed_batch(x)`
- `self.memory_store.is_available()` → keep (still on MemoryService trait)
- `self.memory_store.embed_dim()` → `self.embedder.embed_dim()`

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p hydeclaw-core 2>&1 | grep "^error" | wc -l
```

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/agent/
git commit -m "refactor: engine files use embedder for embed operations"
```

---

## Task 9: Migrate handlers

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/memory.rs`
- Modify: `crates/hydeclaw-core/src/gateway/handlers/chat.rs`
- Modify: `crates/hydeclaw-core/src/gateway/handlers/config.rs`

- [ ] **Step 1: Fix memory.rs handler**

Replace:
- `state.memory_store.embed(x)` → `infra.embedder.embed(x)`
- `state.memory_store.embed_dim()` → `infra.embedder.embed_dim()`
- `state.memory_store.embed_model_name()` → `infra.embedder.embed_model_name().unwrap_or_default()`
- Admin calls: use `MemoryAdmin::new(infra.db.clone())` inline, or call `crate::db::memory_queries::*` directly

- [ ] **Step 2: Fix chat.rs handler**

Replace:
- `infra.memory_store.embed_batch(...)` → `infra.embedder.embed_batch(...)`
- `infra.memory_store.embed_model_name()` → `infra.embedder.embed_model_name().unwrap_or_default()`
- `infra.memory_store.is_available()` → `infra.embedder.is_available()`

- [ ] **Step 3: Fix config.rs handler**

Replace:
- `infra.memory_store.embed_dim()` → `infra.embedder.embed_dim()`
- `infra.memory_store.is_available()` → `infra.embedder.is_available()`

- [ ] **Step 4: Fix any other handlers**

```bash
grep -rn "memory_store\.\(embed\|embed_batch\|embed_dim\|embed_model\)" \
  crates/hydeclaw-core/src/gateway/handlers/ --include="*.rs"
```

Fix all remaining references.

- [ ] **Step 5: Verify compilation — expect clean**

```bash
cargo check --all-targets 2>&1 | tail -5
```

Expected: `Finished` with no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/
git commit -m "refactor: handlers use embedder for embed operations"
```

---

## Task 10: Finalize

- [ ] **Step 1: Run full test suite**

```bash
cargo test 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 2: Run clippy**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -10
```

Fix any warnings (unused imports, dead_code on removed methods).

- [ ] **Step 3: Verify no references to old memory.rs remain**

```bash
grep -rn "crate::memory::MemoryStore::fmt_vec\|crate::memory::MemoryStore::detect_fts" \
  crates/hydeclaw-core/src/ --include="*.rs"
```

Should return nothing — these are now `crate::memory::fmt_vec` and `MemoryAdmin::detect_fts_language`.

- [ ] **Step 4: Final checks**

```bash
cargo check --all-targets 2>&1 | tail -3
cargo test 2>&1 | grep "test result"
cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
```

All three should be clean.

- [ ] **Step 5: Commit any remaining fixes**

```bash
git add -A
git commit -m "refactor: finalize MemoryStore decomposition — cleanup"
```
