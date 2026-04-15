# MemoryStore Decomposition Design

**Date:** 2026-04-15
**Status:** Approved
**Scope:** Рефакторинг god-object `MemoryStore` — второй из трёх (AppState done → MemoryStore → AgentEngine)

---

## Проблема

`memory.rs` — 1258 строк, 9 полей, 6 перемешанных ответственностей в одном struct:

1. **HTTP embedding client** — `reqwest::Client`, URL конструкция, OpenAI-format парсинг, timeout-handling
2. **Embedding provider discovery** — lazy dimension probe, `AtomicU32::compare_exchange`, `OnceLock` для model name
3. **Vector index lifecycle** — HNSW creation, dimension mismatch recovery
4. **Semantic search pipeline** — MMR reranking, RRF hybrid merge, FTS fallback, dedup
5. **FTS configuration** — language detection, runtime mutation через `RwLock`, rebuild
6. **Background task queue** — `enqueue_reindex_task` — producer для memory-worker binary

3 SQL-запроса инлайнятся в `memory.rs` вместо `db/memory_queries.rs`. `spawn_workspace_watcher` (free function) замыкается на `Arc<MemoryStore>`.

Единый трейт `MemoryService` (12 методов) смешивает embed-операции с search/index — мокать можно только всё целиком.

---

## Решение: 3 модуля + обновлённые трейты

### Структура файлов

```
src/memory/
├── mod.rs              — pub use реэкспорты
├── embedding.rs        — trait EmbeddingService + struct ToolgateEmbedder
├── store.rs            — struct MemoryStore (search, index, get, delete, load_pinned)
├── admin.rs            — struct MemoryAdmin (FTS, rebuild, wipe, reindex task)
└── watcher.rs          — spawn_workspace_watcher (free function)
```

Файл `src/memory.rs` → директория `src/memory/` с `mod.rs`.

---

## Модуль 1: EmbeddingService + ToolgateEmbedder (embedding.rs)

### Трейт

```rust
#[async_trait::async_trait]
pub trait EmbeddingService: Send + Sync {
    fn is_available(&self) -> bool;
    fn embed_dim(&self) -> u32;
    fn embed_model_name(&self) -> Option<String>;
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>;
}
```

### Struct

```rust
pub struct ToolgateEmbedder {
    http: reqwest::Client,
    embed_url: String,
    embed_model: OnceLock<String>,
    embed_dim: AtomicU32,
    embed_dimensions: u32,
    initialized: OnceCell<()>,
}
```

### Переносимые методы

Из старого `MemoryStore`:
- `embed()`, `embed_batch()` → реализация `EmbeddingService`
- `is_available()` (проверка enabled + URL) → `EmbeddingService::is_available()`
- `embed_dim()`, `embed_model_name()` → `EmbeddingService`
- `do_initialize()`, `ensure_initialized()` → приватные методы `ToolgateEmbedder`
- `fetch_embed_model_from_toolgate()` → приватный
- `ensure_index(dim)` → приватный (вызывает `db::memory_queries::ensure_hnsw_index`)
- `fmt_vec()` → pub(crate) static helper в `embedding.rs`

### Конструктор

```rust
impl ToolgateEmbedder {
    pub fn new(db: PgPool, toolgate_url: Option<&str>, embed_dimensions: u32) -> Self;

    #[cfg(test)]
    pub fn test_unavailable() -> Self;  // is_available() = false
}
```

`db: PgPool` нужен для `ensure_index()` (HNSW index creation при первом embed). Это единственная DB-зависимость embedder — только DDL, не data queries.

---

## Модуль 2: MemoryStore (store.rs)

### Struct

```rust
pub struct MemoryStore {
    db: PgPool,
    embedder: Arc<dyn EmbeddingService>,
    fts_language: RwLock<String>,
}
```

### Публичные методы

- `search(query, limit, exclude_ids, category, topic, agent_id)` → гибридный/FTS поиск
- `search_fts(query, limit, agent_id)` → FTS fallback
- `index(content, source, pinned, category, topic, scope, agent_id)` → embed + insert
- `index_batch(items, agent_id)` → batch embed + insert
- `get(chunk_id, source, limit)` → fetch by id/source/recent
- `recent(limit)` → most-recently-accessed
- `delete(chunk_id)` → delete by UUID
- `load_pinned(agent_id, budget_tokens)` → L0 pinned chunks for context

### Приватные методы

- `search_hybrid()`, `search_semantic()` — internal pipeline
- `dedup_by_parent()` — post-processing

### Конструктор

```rust
impl MemoryStore {
    pub fn new(db: PgPool, embedder: Arc<dyn EmbeddingService>, fts_language: &str) -> Self;

    #[cfg(test)]
    pub fn test_with_embedder(embedder: Arc<dyn EmbeddingService>) -> Self;
}
```

### Реализует обновлённый MemoryService

`MemoryStore` имплементирует `MemoryService` — тонкие делегации к pub-методам. Дополнительно делегирует `wipe_agent_memory` и `enqueue_reindex_task` через внутренний `MemoryAdmin`.

---

## Модуль 3: MemoryAdmin (admin.rs)

### Struct

```rust
pub struct MemoryAdmin {
    db: PgPool,
}
```

### Публичные методы

- `validated_fts_language(lang: &str) -> String` — safe-SQL validation (статическая, pub(crate))
- `detect_fts_language(lang_code: &str) -> &'static str` — BCP-47 → PG dict (статическая)
- `rebuild_fts(fts_language: &str)` → re-stem all chunks
- `delete_by_source(source: &str)` → delete all chunks for a file path
- `wipe_agent_memory(agent_id: &str)` → delete all chunks for agent
- `enqueue_reindex_task(params: serde_json::Value)` → insert into memory_tasks

3 inline SQL из старого `memory.rs` → `db/memory_queries.rs`:
- `DELETE FROM memory_chunks WHERE source = $1`
- `DELETE FROM memory_chunks WHERE agent_id = $1`
- `INSERT INTO memory_tasks ... RETURNING id`

### Конструктор

```rust
impl MemoryAdmin {
    pub fn new(db: PgPool) -> Self;
}
```

Не трейт — мокать не нужно. Handlers создают `MemoryAdmin::new(infra.db.clone())` по месту.

---

## Модуль 4: Workspace Watcher (watcher.rs)

```rust
pub fn spawn_workspace_watcher(
    store: Arc<MemoryStore>,
    workspace_dir: String,
) -> tokio::task::JoinHandle<()>;
```

Переносится из конца `memory.rs` без изменений логики. Единственная правка: `store.delete_by_source()` вызывается на `MemoryStore`, а не напрямую SQL.

Примечание: `delete_by_source` нужен и в MemoryStore (watcher), и в MemoryAdmin (handler). Реализация через общую функцию в `db/memory_queries.rs`, оба модуля вызывают её.

---

## Обновлённый MemoryService трейт

В `agent/memory_service.rs`:

```rust
#[async_trait::async_trait]
pub trait MemoryService: Send + Sync {
    fn is_available(&self) -> bool;
    async fn search(&self, query: &str, limit: usize, exclude_ids: &[String],
                    category: Option<&str>, topic: Option<&str>, agent_id: &str)
        -> anyhow::Result<(Vec<MemoryResult>, String)>;
    async fn index(&self, content: &str, source: &str, pinned: bool,
                   category: Option<&str>, topic: Option<&str>, scope: &str, agent_id: &str)
        -> anyhow::Result<String>;
    async fn index_batch(&self, items: &[(&str, &str, bool, &str)], agent_id: &str)
        -> anyhow::Result<Vec<String>>;
    async fn get(&self, chunk_id: Option<&str>, source: Option<&str>, limit: i64)
        -> anyhow::Result<Vec<MemoryChunk>>;
    async fn delete(&self, chunk_id: &str) -> anyhow::Result<bool>;
    async fn recent(&self, limit: i64) -> anyhow::Result<Vec<MemoryResult>>;
    async fn load_pinned(&self, agent_id: &str, budget_tokens: u32)
        -> anyhow::Result<(String, Vec<String>)>;
    async fn wipe_agent_memory(&self, agent_id: &str) -> anyhow::Result<u64>;
    async fn enqueue_reindex_task(&self, params: serde_json::Value)
        -> anyhow::Result<uuid::Uuid>;
}
```

Убраны: `embed()`, `embed_batch()` (теперь в `EmbeddingService`).
`is_available()` остаётся — MemoryStore делегирует в `self.embedder.is_available()`.

`MockMemoryService` в тестах — обновляется соответственно.

---

## InfraServices: новое поле

```rust
pub struct InfraServices {
    pub db: PgPool,
    pub memory_store: Arc<dyn MemoryService>,
    pub embedder: Arc<dyn EmbeddingService>,    // NEW
    pub container_manager: Option<Arc<ContainerManager>>,
    pub sandbox: Option<Arc<CodeSandbox>>,
    pub process_manager: Option<Arc<ProcessManager>>,
}
```

`FromRef<AppState>` для `InfraServices` — уже реализован, поле добавляется автоматически.

---

## Тестирование (TDD)

### FakeEmbedder для тестов

```rust
#[cfg(test)]
pub struct FakeEmbedder {
    pub available: bool,
    pub dim: u32,
}

#[async_trait::async_trait]
impl EmbeddingService for FakeEmbedder {
    fn is_available(&self) -> bool { self.available }
    fn embed_dim(&self) -> u32 { self.dim }
    fn embed_model_name(&self) -> Option<String> { Some("fake".into()) }
    async fn embed(&self, _: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0; self.dim as usize])
    }
    async fn embed_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0; self.dim as usize]).collect())
    }
}
```

### Тесты по модулям

- `embedding.rs` — `test_unavailable_when_no_url`, `test_fmt_vec`
- `store.rs` — `test_new_with_fake_embedder`, `test_is_available_delegates_to_embedder`
- `admin.rs` — `test_detect_fts_language_ru`, `test_detect_fts_language_en`, `test_validated_fts_language_rejects_injection`
- `memory_service.rs` — обновить `MockMemoryService` (убрать embed-методы)

---

## План миграции

На ветке `refactor/appstate-clusters` (продолжение текущего PR). 10 фаз:

1. Создать `memory/` директорию + `mod.rs`
2. `embedding.rs` — трейт + `ToolgateEmbedder` + тесты
3. `store.rs` — `MemoryStore` с `Arc<dyn EmbeddingService>` + тесты
4. `admin.rs` — `MemoryAdmin` + перенос inline SQL в `db/memory_queries.rs` + тесты
5. `watcher.rs` — перенос `spawn_workspace_watcher`
6. Обновить `MemoryService` трейт + `MockMemoryService`
7. Добавить `embedder` поле в `InfraServices` + `FromRef`
8. Обновить `main.rs` — конструкция `ToolgateEmbedder` + `MemoryStore`
9. Мигрировать engine-файлы и handlers
10. Удалить старый `memory.rs`, финализация (tests + clippy)

---

## Что НЕ входит

- AgentEngine decomposition — отдельный спек
- Изменение DB схемы `memory_chunks` — не нужно
- Новые HTTP endpoints — не добавляем
- Миграция memory-worker binary — он уже работает через `memory_tasks` таблицу, не через MemoryStore напрямую

---

## Критерии готовности

- [ ] `EmbeddingService` трейт + `ToolgateEmbedder` struct в `memory/embedding.rs`
- [ ] `MemoryStore` в `memory/store.rs` получает `Arc<dyn EmbeddingService>`
- [ ] `MemoryAdmin` в `memory/admin.rs`, inline SQL перенесён в `db/memory_queries.rs`
- [ ] `spawn_workspace_watcher` в `memory/watcher.rs`
- [ ] `MemoryService` трейт обновлён (без embed-методов)
- [ ] `InfraServices` имеет поле `embedder: Arc<dyn EmbeddingService>`
- [ ] Все engine-файлы используют `embedder` для embed-операций
- [ ] Все handlers используют правильные модули
- [ ] `cargo check --all-targets` чистый
- [ ] `cargo test` зелёный
- [ ] `cargo clippy --all-targets -- -D warnings` чистый
