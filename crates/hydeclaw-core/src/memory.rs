/// Native pgvector memory store.
///
/// pgvector queries run directly against the local `PostgreSQL` pool.
/// Embedding generation is delegated to Toolgate (`POST /v1/embeddings`), which
/// proxies to the configured embedding backend (Ollama, `OpenAI`, or any other
/// OpenAI-compatible provider). Core never calls Ollama or `OpenAI` directly.
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{OnceLock, RwLock};
use tokio::sync::OnceCell;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default, schemars::JsonSchema)]
pub struct MemoryConfig {
    /// Whether embedding is enabled. Defaults to true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Vector dimension (optional, auto-detected at startup)
    pub embed_dim: Option<u32>,
    /// `PostgreSQL` FTS dictionary name (e.g. "russian", "english", "simple").
    /// Auto-detected from first agent's language if not set.
    pub fts_language: Option<String>,
    /// Maximum tokens for pinned chunks in L0 context. Default: 2000.
    /// Approximation: `content.len()` / 4.
    #[serde(default = "default_pinned_budget")]
    pub pinned_budget_tokens: u32,
    /// Age in days after which non-pinned chunks become eligible for compression. Default: 30.
    #[serde(default = "default_compression_age_days")]
    pub compression_age_days: u32,
}

use crate::config::default_true;

fn default_pinned_budget() -> u32 {
    2000
}

fn default_compression_age_days() -> u32 {
    30
}

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
    pub archived: Option<bool>,
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
    pub archived: Option<bool>,
}

// ── Store ─────────────────────────────────────────────────────────────────────

pub struct MemoryStore {
    db: PgPool,
    http: reqwest::Client,
    enabled: bool,
    embed_url: String,
    embed_model: OnceLock<String>,
    /// 0 = not yet detected
    embed_dim: AtomicU32,
    /// `PostgreSQL` FTS dictionary (e.g. "russian", "english", "simple").
    /// Mutable at runtime via API.
    fts_language: RwLock<String>,
    /// Lazy initialization guard: embedding probe runs on first memory operation.
    initialized: OnceCell<()>,
}

impl MemoryStore {
    pub fn new(db: PgPool, config: &MemoryConfig, toolgate_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        let fts_lang = config.fts_language.clone().unwrap_or_else(|| "simple".to_string());
        let embed_url = if toolgate_url.is_empty() {
            String::new()
        } else {
            format!("{}/v1", toolgate_url.trim_end_matches('/'))
        };
        Self {
            db,
            http,
            enabled: config.enabled,
            embed_url,
            embed_model: OnceLock::new(),
            embed_dim: AtomicU32::new(config.embed_dim.unwrap_or(0)),
            fts_language: RwLock::new(fts_lang),
            initialized: OnceCell::new(),
        }
    }

    /// Returns true when embedding is enabled and endpoint is configured.
    pub fn is_available(&self) -> bool {
        self.enabled && !self.embed_url.is_empty()
    }

    /// Returns the configured embedding model name.
    pub fn embed_model_name(&self) -> String {
        self.embed_model.get().cloned().unwrap_or_default()
    }

    /// Returns the detected embedding dimension (0 if not yet detected).
    pub fn embed_dim(&self) -> u32 {
        self.embed_dim.load(Ordering::Relaxed)
    }

    /// Returns the current FTS language.
    pub fn fts_language(&self) -> String {
        self.fts_language.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Returns the FTS language after validating it is safe for SQL interpolation.
    /// regconfig cannot be parameterized, so we must validate before format!().
    pub fn validated_fts_language(&self) -> anyhow::Result<String> {
        let lang = self.fts_language();
        anyhow::ensure!(
            !lang.is_empty() && lang.chars().all(|c| c.is_ascii_lowercase()),
            "invalid FTS language: {lang}"
        );
        Ok(lang)
    }

    /// Update the FTS language at runtime (normalizes to lowercase).
    pub fn set_fts_language(&self, lang: &str) {
        *self.fts_language.write().unwrap_or_else(std::sync::PoisonError::into_inner) = lang.to_ascii_lowercase();
    }

    /// Auto-detect FTS language from agent language code (e.g. "ru" → "russian").
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
            _ => "simple", // fallback for unsupported languages
        }.to_string()
    }

    /// Query toolgate /health to discover the active embedding provider display name.
    /// Note: the display name (e.g. "`OpenAI` Embedding") is for logging only — it is NOT
    /// passed as the `model` field in embedding requests. Toolgate resolves the actual
    /// model internally from its provider registry.
    async fn fetch_embed_model_from_toolgate(&self) {
        let health_url = format!(
            "{}/health",
            self.embed_url
                .trim_end_matches('/')
                .trim_end_matches("/v1"),
        );
        match self.http.get(&health_url).timeout(std::time::Duration::from_secs(5)).send().await {
            Ok(resp) => {
                if let Ok(body) = resp.json::<serde_json::Value>().await
                    && let Some(name) = body["active_providers"]["embedding"].as_str()
                    {
                        tracing::info!(embed_provider = %name, "discovered embedding provider from toolgate");
                        // Store display name for logging/status only — do NOT use as model param
                        let _ = self.embed_model.set(format!("({name})"));
                    }
            }
            Err(e) => {
                tracing::debug!(error = %e, "could not query toolgate /health for provider name");
            }
        }
    }

    /// Initialize embedding: auto-detect dimension, validate DB, ensure HNSW index.
    /// Graceful: if embedding endpoint is unreachable, logs a warning and continues
    /// (FTS fallback will be used for search).
    async fn do_initialize(&self) -> Result<()> {
        if !self.is_available() {
            tracing::info!("embedding not configured, memory will use FTS only");
            return Ok(());
        }

        // 1. Detect dimension (from config or probe request)
        let current_dim = self.embed_dim.load(Ordering::Relaxed);
        let dim = if current_dim > 0 {
            current_dim
        } else {
            match self.embed("dimension probe").await {
                Ok(probe) => {
                    let d = probe.len() as u32;
                    self.embed_dim.store(d, Ordering::Relaxed);
                    d
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "embedding endpoint unreachable at startup, memory degraded to FTS"
                    );
                    return Ok(());
                }
            }
        };

        // 2. Discover embedding model name from toolgate health endpoint
        self.fetch_embed_model_from_toolgate().await;

        // 3. Check if DB has embeddings with a different dimension
        let existing_dim = crate::db::memory_queries::get_existing_embedding_dim(&self.db).await;

        if let Some(old_dim) = existing_dim
            && old_dim as u32 != dim {
                tracing::warn!(
                    old_dim, new_dim = dim,
                    "embedding dimension changed, clearing memory_chunks"
                );
                crate::db::memory_queries::clear_embeddings(&self.db).await?;
                // Drop old index (wrong dimension)
                crate::db::memory_queries::drop_hnsw_index(&self.db).await?;
            }

        // 4. Ensure HNSW index with correct dimension
        self.ensure_index(dim).await?;

        let model = self.embed_model_name();
        tracing::info!(
            model = %model,
            dim,
            "embedding initialized"
        );
        Ok(())
    }

    /// Lazy initialization: runs embedding probe on first memory operation, not at startup.
    async fn ensure_initialized(&self) {
        self.initialized.get_or_init(|| async {
            if let Err(e) = self.do_initialize().await {
                tracing::warn!(error = %e, "embedding init failed — memory uses FTS only");
            }
        }).await;
    }

    /// Create HNSW index if it doesn't exist.
    async fn ensure_index(&self, dim: u32) -> Result<()> {
        crate::db::memory_queries::ensure_hnsw_index(&self.db, dim).await
    }

    // ── Embedding ─────────────────────────────────────────────────────────────

    /// Call the OpenAI-compatible /v1/embeddings endpoint and return the vector.
    /// On first successful call (dim == 0), performs lazy initialization:
    /// detects dimension and creates HNSW index.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/embeddings", self.embed_url.trim_end_matches('/'));
        let model = self.embed_model_name();
        let mut body = serde_json::json!({ "input": text });
        // Only pass model if it's a real model ID (not a display name from toolgate health)
        if !model.is_empty() && !model.starts_with('(') {
            body["model"] = serde_json::Value::String(model);
        }
        let resp = self.http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("embedding request failed")?;

        resp.error_for_status_ref().context("embedding API error")?;
        let body: serde_json::Value = resp.json().await.context("failed to parse embedding response")?;

        let vec: Vec<f32> = body["data"][0]["embedding"]
            .as_array()
            .context("missing 'data[0].embedding' in response")?
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect();

        anyhow::ensure!(!vec.is_empty(), "embedding returned empty vector");

        // Validate dimension matches expected (if already known)
        let expected = self.embed_dim.load(Ordering::Relaxed);
        if expected > 0 && vec.len() as u32 != expected {
            anyhow::bail!(
                "embedding dimension mismatch: expected {}, got {} — possible model change",
                expected, vec.len()
            );
        }

        // Lazy init: if dim was unknown (embedding was down at startup), set it now.
        // compare_exchange ensures only one thread creates the HNSW index.
        let detected_dim = vec.len() as u32;
        if self.embed_dim.compare_exchange(0, detected_dim, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            let model = self.embed_model_name();
            tracing::info!(dim = detected_dim, model = %model, "embedding came online, lazy-initializing");
            if let Err(e) = self.ensure_index(detected_dim).await {
                tracing::warn!(error = %e, "failed to create HNSW index during lazy init");
            }
        }

        Ok(vec)
    }

    /// Batch embed: sends multiple texts in one request (`OpenAI` API supports arrays).
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        if texts.len() == 1 {
            return Ok(vec![self.embed(texts[0]).await?]);
        }

        let url = format!("{}/embeddings", self.embed_url.trim_end_matches('/'));
        let model = self.embed_model_name();
        let mut body = serde_json::json!({ "input": texts });
        if !model.is_empty() && !model.starts_with('(') {
            body["model"] = serde_json::Value::String(model);
        }
        let resp = self.http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("batch embedding request failed")?;

        resp.error_for_status_ref().context("batch embedding API error")?;
        let body: serde_json::Value = resp.json().await.context("failed to parse batch embedding response")?;

        let data = body["data"]
            .as_array()
            .context("missing 'data' array in batch embedding response")?;

        let mut results = Vec::with_capacity(texts.len());
        for item in data {
            let vec: Vec<f32> = item["embedding"]
                .as_array()
                .context("missing 'embedding' in batch result")?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            anyhow::ensure!(!vec.is_empty(), "batch embedding returned empty vector");
            results.push(vec);
        }

        // Validate dimension matches expected (if already known)
        let expected = self.embed_dim.load(Ordering::Relaxed);
        if expected > 0 {
            for (i, v) in results.iter().enumerate() {
                if v.len() as u32 != expected {
                    anyhow::bail!(
                        "batch embedding dimension mismatch at index {}: expected {}, got {}",
                        i, expected, v.len()
                    );
                }
            }
        }

        // Lazy init if needed.
        // compare_exchange ensures only one thread creates the HNSW index.
        if !results.is_empty() {
            let detected_dim = results[0].len() as u32;
            if self.embed_dim.compare_exchange(0, detected_dim, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                let model = self.embed_model_name();
                tracing::info!(dim = detected_dim, model = %model, "embedding came online via batch, lazy-initializing");
                if let Err(e) = self.ensure_index(detected_dim).await {
                    tracing::warn!(error = %e, "failed to create HNSW index during lazy init");
                }
            }
        }

        Ok(results)
    }

    /// Batch index: embed multiple texts and insert them all. Returns chunk IDs.
    /// Long texts (> `DEFAULT_CHUNK_SIZE`) are delegated to `index()` for auto-chunking.
    /// Short texts are batch-embedded in a single request for efficiency.
    /// Category and topic are not supported in batch index (pass None/None per item).
    /// Tuple: (content, source, pinned, scope).
    pub async fn index_batch(&self, items: &[(String, String, bool, String)]) -> Result<Vec<String>> {
        self.ensure_initialized().await;
        if items.is_empty() {
            return Ok(vec![]);
        }

        let lang = self.validated_fts_language()?;
        let mut ids: Vec<(usize, String)> = Vec::with_capacity(items.len());

        // Split: long items use index() with chunking, short items batch-embed
        let mut short_items: Vec<(usize, &str, &str, bool, &str)> = Vec::new();
        for (idx, (content, source, pinned, scope)) in items.iter().enumerate() {
            if content.len() > crate::chunker::DEFAULT_CHUNK_SIZE {
                let id = self.index(content, source, *pinned, None, None, scope).await
                    .context("failed to index long item in batch")?;
                ids.push((idx, id));
            } else {
                short_items.push((idx, content, source, *pinned, scope));
            }
        }

        if !short_items.is_empty() {
            let texts: Vec<&str> = short_items.iter().map(|(_, c, _, _, _)| *c).collect();
            let embeddings = self.embed_batch(&texts).await?;

            for (i, &(idx, content, source, pinned, scope)) in short_items.iter().enumerate() {
                let vec_str = Self::fmt_vec(&embeddings[i]);
                let id = uuid::Uuid::new_v4().to_string();
                crate::db::memory_queries::insert_chunk(
                    &self.db, &id, content, &vec_str, source, pinned, &lang, None, 0,
                    None, None, scope,
                ).await
                .context("failed to insert memory chunk in batch")?;
                ids.push((idx, id));
            }
        }

        ids.sort_by_key(|(idx, _)| *idx);
        Ok(ids.into_iter().map(|(_, id)| id).collect())
    }

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

    // ── Search ────────────────────────────────────────────────────────────────

    /// Deduplicate results: keep highest-scoring chunk per parent document.
    /// Results are pre-sorted by similarity, so the first occurrence of each
    /// `parent_id` is the best one.
    fn dedup_by_parent(results: Vec<MemoryResult>) -> Vec<MemoryResult> {
        let mut seen = std::collections::HashSet::with_capacity(results.len());
        results.into_iter().filter(|r| {
            seen.insert(r.parent_id.as_deref().unwrap_or(&r.id).to_owned())
        }).collect()
    }

    /// Search memory: hybrid (semantic + FTS via RRF) when embedding available, pure FTS fallback.
    /// Returns (results, `search_mode`) where `search_mode` is "hybrid", "semantic", or "fts".
    /// `exclude_ids`: chunk IDs already loaded via L0 pinned loading — excluded from results (CTX-04).
    /// `category` / `topic`: optional post-query filters; only chunks with matching values are returned.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        exclude_ids: &[String],
        category: Option<&str>,
        topic: Option<&str>,
    ) -> Result<(Vec<MemoryResult>, &'static str)> {
        self.ensure_initialized().await;
        if query.trim().is_empty() {
            return Ok((vec![], "none"));
        }

        let (results, mode) = if self.is_available() {
            // Run semantic + FTS in parallel and merge via RRF
            match self.search_hybrid(query, limit).await {
                Ok(results) if !results.is_empty() => (results, "hybrid"),
                Ok(_) => {
                    let fts = self.search_fts(query, limit).await?;
                    (fts, "fts")
                }
                Err(e) => {
                    tracing::warn!(error = %e, "hybrid search failed, falling back to FTS");
                    let fts = self.search_fts(query, limit).await?;
                    (fts, "fts")
                }
            }
        } else {
            let fts = self.search_fts(query, limit).await?;
            (fts, "fts")
        };

        // Deduplicate: keep only the best chunk per parent document
        let mut results = Self::dedup_by_parent(results);

        // L2 dedup: remove chunks already loaded via L0 pinned loading (CTX-04)
        if !exclude_ids.is_empty() {
            results.retain(|r| !exclude_ids.contains(&r.id));
        }

        // Category/topic post-query filtering (CTX-05)
        if let Some(cat) = category {
            results.retain(|r| r.category.as_deref() == Some(cat));
        }
        if let Some(top) = topic {
            results.retain(|r| r.topic.as_deref() == Some(top));
        }

        Ok((results, mode))
    }

    /// Hybrid search: semantic + FTS merged via Reciprocal Rank Fusion (RRF).
    async fn search_hybrid(&self, query: &str, limit: usize) -> Result<Vec<MemoryResult>> {
        use std::collections::HashMap;

        let (sem_result, fts_result) = tokio::join!(
            self.search_semantic(query, limit * 2),
            self.search_fts(query, limit * 2),
        );

        let sem = match sem_result {
            Ok(v) => v,
            Err(e) => { tracing::warn!(error = %e, "semantic search failed"); vec![] }
        };
        let fts = match fts_result {
            Ok(v) => v,
            Err(e) => { tracing::warn!(error = %e, "FTS search failed"); vec![] }
        };

        if sem.is_empty() { return Ok(fts.into_iter().take(limit).collect()); }
        if fts.is_empty() { return Ok(sem.into_iter().take(limit).collect()); }

        const K: f64 = 60.0;

        // Build rank maps for RRF scoring
        let sem_ranks: HashMap<String, usize> = sem.iter()
            .enumerate().map(|(i, r)| (r.id.clone(), i)).collect();
        let fts_ranks: HashMap<String, usize> = fts.iter()
            .enumerate().map(|(i, r)| (r.id.clone(), i)).collect();

        // Collect all unique results (semantic takes priority for the stored copy)
        let mut all: HashMap<String, MemoryResult> = HashMap::new();
        for r in sem { all.entry(r.id.clone()).or_insert(r); }
        for r in fts { all.entry(r.id.clone()).or_insert(r); }

        // Score each result with RRF: 1/(k + rank_sem) + 1/(k + rank_fts)
        let mut scored: Vec<(f64, MemoryResult)> = all.into_values().map(|r| {
            let sem_rrf = sem_ranks.get(&r.id)
                .map_or(0.0, |&rank| 1.0 / (K + rank as f64 + 1.0));
            let fts_rrf = fts_ranks.get(&r.id)
                .map_or(0.0, |&rank| 1.0 / (K + rank as f64 + 1.0));
            (sem_rrf + fts_rrf, r)
        }).collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored.into_iter().take(limit).map(|(_, r)| r).collect())
    }

    /// Semantic similarity search with MMR reranking (lambda=0.75).
    async fn search_semantic(&self, query: &str, limit: usize) -> Result<Vec<MemoryResult>> {
        let embedding = self.embed(query).await?;
        let vec_str = Self::fmt_vec(&embedding);
        let candidate_limit = (limit * 6) as i64;

        let mut candidates = crate::db::memory_queries::search_semantic(
            &self.db, &vec_str, candidate_limit,
        )
        .await?;

        // MMR reranking (lambda=0.75): balance relevance vs diversity.
        // Penalty = max inter-result similarity, approximated via min(candidate_sim, selected_sim)
        // since we only have query-similarity, not cross-embeddings.
        let mut results: Vec<MemoryResult> = Vec::with_capacity(limit);
        let mut selected_sims: Vec<f64> = Vec::with_capacity(limit);
        let lam = 0.75_f64;

        for _ in 0..limit.min(candidates.len()) {
            let mut best_idx = 0;
            let mut best_score = f64::NEG_INFINITY;

            for (i, c) in candidates.iter().enumerate() {
                let relevance = c.similarity * c.relevance_score;
                let max_sim_to_selected = selected_sims.iter()
                    .map(|&r_sim| c.similarity.min(r_sim))
                    .fold(0.0_f64, f64::max);
                let score = lam * relevance - (1.0 - lam) * max_sim_to_selected;
                if score > best_score {
                    best_score = score;
                    best_idx = i;
                }
            }
            let selected = candidates.remove(best_idx);
            selected_sims.push(selected.similarity);
            results.push(selected);
        }

        // Update accessed_at for returned chunks
        let ids: Vec<uuid::Uuid> = results.iter().filter_map(|r| r.id.parse().ok()).collect();
        crate::db::memory_queries::touch_accessed(&self.db, &ids).await;

        Ok(results)
    }

    /// Full-text search using `PostgreSQL` tsvector/tsquery with morphological stemming.
    /// Used as fallback when embedding endpoint is unavailable.
    pub async fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<MemoryResult>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        let lang = self.validated_fts_language()?;

        let results = crate::db::memory_queries::search_fts(
            &self.db, query, limit as i64, &lang,
        )
        .await?;

        // Update accessed_at
        let ids: Vec<uuid::Uuid> = results.iter().filter_map(|r| r.id.parse().ok()).collect();
        crate::db::memory_queries::touch_accessed(&self.db, &ids).await;

        Ok(results)
    }

    /// Return the most-recently-accessed memory chunks (pinned first).
    pub async fn recent(&self, limit: i64) -> Result<Vec<MemoryResult>> {
        crate::db::memory_queries::fetch_recent(&self.db, limit).await
    }

    /// Load L0 pinned chunks for an agent, respecting token budget.
    /// Returns (formatted text for prompt, list of chunk IDs for L2 dedup).
    /// Budget is in tokens, approximated as `content.len()` / 4.
    /// When total exceeds budget, oldest chunks (FIFO) are dropped first.
    pub async fn load_pinned(
        &self,
        agent_id: &str,
        budget_tokens: u32,
    ) -> Result<(String, Vec<String>)> {
        let chunks = crate::db::memory_queries::fetch_pinned(&self.db, agent_id).await?;
        if chunks.is_empty() {
            return Ok((String::new(), vec![]));
        }

        // Calculate token estimates for all chunks (oldest first from SQL ORDER BY created_at ASC)
        let chunk_tokens: Vec<u32> = chunks.iter()
            .map(|c| (c.content.len() as u32) / 4)
            .collect();
        let total_tokens: u32 = chunk_tokens.iter().sum();

        // Determine how many oldest chunks to skip (FIFO drop: drop oldest first)
        let mut skip_count = 0usize;
        let mut remaining = total_tokens;
        if remaining > budget_tokens {
            for &ct in &chunk_tokens {
                if remaining <= budget_tokens {
                    break;
                }
                remaining -= ct;
                skip_count += 1;
            }
        }

        if skip_count > 0 {
            tracing::warn!(
                dropped = skip_count,
                budget = budget_tokens,
                total = total_tokens,
                "pinned chunks exceed token budget"
            );
        }

        let mut text_parts: Vec<String> = Vec::new();
        let mut ids: Vec<String> = Vec::new();

        for chunk in chunks.iter().skip(skip_count) {
            let source = if chunk.source.is_empty() { "memory" } else { &chunk.source };
            text_parts.push(format!("- [{}] {}", source, chunk.content));
            ids.push(chunk.id.clone());
        }

        let text = if text_parts.is_empty() {
            String::new()
        } else {
            format!("\n\n## Known Facts\n{}", text_parts.join("\n"))
        };

        Ok((text, ids))
    }

    // ── Index ─────────────────────────────────────────────────────────────────

    /// Generate embedding and insert a new memory chunk. Returns the new chunk UUID.
    /// If content exceeds `DEFAULT_CHUNK_SIZE`, splits into overlapping chunks
    /// linked by `parent_id`. Returns the parent chunk's UUID.
    /// `scope`: "private" (agent-only) or "shared" (visible to all agents).
    pub async fn index(
        &self,
        content: &str,
        source: &str,
        pinned: bool,
        category: Option<&str>,
        topic: Option<&str>,
        scope: &str,
    ) -> Result<String> {
        self.ensure_initialized().await;
        let lang = self.validated_fts_language()?;

        let chunks = crate::chunker::split_text(
            content,
            crate::chunker::DEFAULT_CHUNK_SIZE,
            crate::chunker::DEFAULT_CHUNK_OVERLAP,
        );

        if chunks.len() == 1 {
            // Single chunk — original path
            let embedding = self.embed(&chunks[0]).await?;
            let vec_str = Self::fmt_vec(&embedding);
            let id = uuid::Uuid::new_v4().to_string();
            crate::db::memory_queries::insert_chunk(
                &self.db, &id, &chunks[0], &vec_str, source, pinned, &lang, None, 0,
                category, topic, scope,
            ).await?;
            return Ok(id);
        }

        // Multiple chunks — batch embed and link via parent_id
        let texts: Vec<&str> = chunks.iter().map(std::string::String::as_str).collect();
        let embeddings = self.embed_batch(&texts).await?;
        let parent_id = uuid::Uuid::new_v4().to_string();

        for (i, (chunk, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            let vec_str = Self::fmt_vec(embedding);
            let id = if i == 0 {
                parent_id.clone()
            } else {
                uuid::Uuid::new_v4().to_string()
            };
            let parent = if i == 0 { None } else { Some(parent_id.as_str()) };
            crate::db::memory_queries::insert_chunk(
                &self.db, &id, chunk, &vec_str, source, pinned, &lang, parent, i as i32,
                category, topic, scope,
            ).await?;
        }

        tracing::info!(
            parent_id = %parent_id,
            chunks = chunks.len(),
            source = %source,
            "indexed chunked document"
        );
        Ok(parent_id)
    }

    // ── Get ───────────────────────────────────────────────────────────────────

    /// Retrieve chunks by ID, by source, or most-recently-accessed (when both empty).
    pub async fn get(
        &self,
        chunk_id: Option<&str>,
        source: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryChunk>> {
        match (chunk_id, source) {
            (Some(id), _) => {
                crate::db::memory_queries::get_chunk_by_id(&self.db, id).await
            }
            (None, Some(src)) => {
                crate::db::memory_queries::get_chunks_by_source(&self.db, src, limit as i64).await
            }
            (None, None) => {
                crate::db::memory_queries::get_chunks_recent(&self.db, limit as i64).await
            }
        }
    }

    // ── Delete ────────────────────────────────────────────────────────────────

    /// Rebuild all tsv columns with the current FTS language.
    /// Called after changing `fts_language` to re-stem existing content.
    pub async fn rebuild_fts(&self) -> Result<u64> {
        let lang = self.validated_fts_language()?;
        let rows = crate::db::memory_queries::rebuild_fts(&self.db, &lang).await?;
        tracing::info!(lang = %lang, rows, "FTS index rebuilt");
        Ok(rows)
    }

    /// Delete a memory chunk by UUID. Returns true if a row was deleted.
    pub async fn delete(&self, chunk_id: &str) -> Result<bool> {
        crate::db::memory_queries::delete_chunk(&self.db, chunk_id).await
    }

    /// Delete all chunks with a given source (e.g. filename).
    pub async fn delete_by_source(&self, source: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM memory_chunks WHERE source = $1")
            .bind(source)
            .execute(&self.db)
            .await?;
        Ok(result.rows_affected())
    }

    /// Wipe all memory for an agent.
    /// Returns the number of memory chunks deleted.
    pub async fn wipe_agent_memory(&self, agent_id: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM memory_chunks WHERE agent_id = $1")
            .bind(agent_id)
            .execute(&self.db)
            .await?;
        Ok(result.rows_affected())
    }

    /// Insert a reindex task into the memory worker queue.
    pub async fn enqueue_reindex_task(&self, params: serde_json::Value) -> Result<uuid::Uuid> {
        sqlx::query_scalar(
            "INSERT INTO memory_tasks (task_type, params) VALUES ('reindex', $1) RETURNING id",
        )
        .bind(params)
        .fetch_one(&self.db)
        .await
        .map_err(Into::into)
    }
}

// ── Workspace File Watcher ─────────────────────────────────────────────────

/// Watch workspace directory for .md/.txt file changes and auto-index into memory.
/// Uses timer-based debounce: waits for 5s of quiet after last change before re-indexing.
pub fn spawn_workspace_watcher(
    workspace_dir: String,
    memory: std::sync::Arc<MemoryStore>,
    handle: tokio::runtime::Handle,
) {
    use notify::{Event, EventKind, RecursiveMode, Watcher};

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(error = %e, "workspace watcher failed to start");
                return;
            }
        };

        // Watch entire workspace root — exclude system dirs at event time
        let watch_dir_path = std::path::PathBuf::from(&workspace_dir);
        let watch_dir = watch_dir_path.as_path();

        if let Err(e) = watcher.watch(watch_dir, RecursiveMode::Recursive) {
            tracing::error!(error = %e, path = ?watch_dir, "failed to watch workspace dir");
            return;
        }

        tracing::info!(dir = ?watch_dir, "workspace file watcher started");

        let mut pending_files: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
        let mut debounce_deadline: Option<std::time::Instant> = None;

        loop {
            let timeout = debounce_deadline
                .map_or(std::time::Duration::from_secs(3600), |d| d.saturating_duration_since(std::time::Instant::now()));

            match rx.recv_timeout(timeout) {
                Ok(Ok(Event { kind: EventKind::Create(_) | EventKind::Modify(_), paths, .. })) => {
                    let workspace_root = std::path::Path::new(&workspace_dir);
                    let exclude_dirs = crate::agent::workspace::MEMORY_INDEX_EXCLUDE_DIRS;
                    for p in paths {
                        // Skip files in system directories
                        let in_excluded = p.strip_prefix(workspace_root)
                            .ok()
                            .and_then(|rel| rel.components().next())
                            .and_then(|c| c.as_os_str().to_str())
                            .is_some_and(|first| exclude_dirs.contains(&first));
                        if in_excluded {
                            continue;
                        }
                        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if matches!(ext, "md" | "txt") {
                            pending_files.insert(p);
                        }
                    }
                    if !pending_files.is_empty() {
                        debounce_deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                    }
                }
                Ok(_) => {} // other events
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Debounce fired — process pending files
                    if !pending_files.is_empty() {
                        let files: Vec<std::path::PathBuf> = pending_files.drain().collect();
                        let mem = memory.clone();
                        let workspace_dir_clone = workspace_dir.clone();
                        handle.spawn(async move {
                            // Try first file to check if embedding is reachable
                            let mut indexed = 0u32;
                            for path in &files {
                                let content = match tokio::fs::read_to_string(path).await {
                                    Ok(c) if c.len() > 50 => c, // skip tiny files
                                    _ => continue,
                                };
                                let workspace_root = std::path::Path::new(&workspace_dir_clone);
                                let source = path.strip_prefix(workspace_root)
                                    .unwrap_or(path.as_path())
                                    .to_string_lossy()
                                    .to_string();
                                // Delete existing chunks from this source, then re-index
                                if let Err(e) = mem.delete_by_source(&source).await {
                                    tracing::debug!(source = %source, error = %e, "no existing chunks to delete");
                                }
                                match mem.index(&content, &source, false, None, None, "private").await {
                                    Ok(_) => indexed += 1,
                                    Err(e) => {
                                        tracing::debug!(error = %e, "embedding unavailable — skipping workspace indexing");
                                        break;
                                    }
                                }
                            }
                            if indexed > 0 {
                                tracing::info!(count = indexed, "workspace watcher: re-indexed changed files");
                            }
                        });
                        debounce_deadline = None;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── MemoryStore::fmt_vec ─────────────────────────────────────────────────

    #[test]
    fn fmt_vec_empty() {
        assert_eq!(MemoryStore::fmt_vec(&[]), "[]");
    }

    #[test]
    fn fmt_vec_single() {
        assert_eq!(MemoryStore::fmt_vec(&[1.5]), "[1.5]");
    }

    #[test]
    fn fmt_vec_multiple() {
        assert_eq!(MemoryStore::fmt_vec(&[1.0, 2.5, -3.0]), "[1,2.5,-3]");
    }

    #[test]
    fn fmt_vec_no_spaces() {
        // pgvector literal must have no spaces between values
        let result = MemoryStore::fmt_vec(&[0.1, 0.2, 0.3]);
        assert!(!result.contains(' '), "fmt_vec output must not contain spaces: {result}");
    }

    #[test]
    fn fmt_vec_negative_values() {
        assert_eq!(MemoryStore::fmt_vec(&[-1.5, -2.5]), "[-1.5,-2.5]");
    }

    // ── MemoryStore::detect_fts_language ──────────────────────────────────────

    #[test]
    fn detect_fts_language_russian() {
        assert_eq!(MemoryStore::detect_fts_language("ru"), "russian");
    }

    #[test]
    fn detect_fts_language_english() {
        assert_eq!(MemoryStore::detect_fts_language("en"), "english");
    }

    #[test]
    fn detect_fts_language_spanish() {
        assert_eq!(MemoryStore::detect_fts_language("es"), "spanish");
    }

    #[test]
    fn detect_fts_language_german() {
        assert_eq!(MemoryStore::detect_fts_language("de"), "german");
    }

    #[test]
    fn detect_fts_language_french() {
        assert_eq!(MemoryStore::detect_fts_language("fr"), "french");
    }

    #[test]
    fn detect_fts_language_portuguese() {
        assert_eq!(MemoryStore::detect_fts_language("pt"), "portuguese");
    }

    #[test]
    fn detect_fts_language_italian() {
        assert_eq!(MemoryStore::detect_fts_language("it"), "italian");
    }

    #[test]
    fn detect_fts_language_dutch() {
        assert_eq!(MemoryStore::detect_fts_language("nl"), "dutch");
    }

    #[test]
    fn detect_fts_language_swedish() {
        assert_eq!(MemoryStore::detect_fts_language("sv"), "swedish");
    }

    #[test]
    fn detect_fts_language_norwegian_variants() {
        assert_eq!(MemoryStore::detect_fts_language("no"), "norwegian");
        assert_eq!(MemoryStore::detect_fts_language("nb"), "norwegian");
    }

    #[test]
    fn detect_fts_language_danish() {
        assert_eq!(MemoryStore::detect_fts_language("da"), "danish");
    }

    #[test]
    fn detect_fts_language_finnish() {
        assert_eq!(MemoryStore::detect_fts_language("fi"), "finnish");
    }

    #[test]
    fn detect_fts_language_hungarian() {
        assert_eq!(MemoryStore::detect_fts_language("hu"), "hungarian");
    }

    #[test]
    fn detect_fts_language_romanian() {
        assert_eq!(MemoryStore::detect_fts_language("ro"), "romanian");
    }

    #[test]
    fn detect_fts_language_turkish() {
        assert_eq!(MemoryStore::detect_fts_language("tr"), "turkish");
    }

    #[test]
    fn detect_fts_language_unknown_fallback() {
        assert_eq!(MemoryStore::detect_fts_language("xx"), "simple");
    }

    #[test]
    fn detect_fts_language_empty_fallback() {
        assert_eq!(MemoryStore::detect_fts_language(""), "simple");
    }

    // ── MemoryConfig default ─────────────────────────────────────────────────

    #[test]
    fn memory_config_default_enabled() {
        // The serde default_true only applies during deserialization.
        // Test that deserializing an empty object gives enabled=true.
        let config: MemoryConfig = serde_json::from_str("{}").unwrap();
        assert!(config.enabled, "MemoryConfig.enabled should default to true via serde");
    }

    #[test]
    fn memory_config_default_option_fields_none() {
        let config: MemoryConfig = serde_json::from_str("{}").unwrap();
        assert!(config.embed_dim.is_none());
        assert!(config.fts_language.is_none());
    }

    #[test]
    fn dedup_by_parent_keeps_first_occurrence() {
        let results = vec![
            MemoryResult {
                id: "id1".into(), content: "a".into(), source: "s".into(),
                pinned: false, relevance_score: 1.0, similarity: 0.9,
                parent_id: Some("parent1".into()), chunk_index: 0,
                category: None, topic: None, archived: None,
            },
            MemoryResult {
                id: "id2".into(), content: "b".into(), source: "s".into(),
                pinned: false, relevance_score: 1.0, similarity: 0.8,
                parent_id: Some("parent1".into()), chunk_index: 1,
                category: None, topic: None, archived: None,
            },
            MemoryResult {
                id: "id3".into(), content: "c".into(), source: "s2".into(),
                pinned: false, relevance_score: 1.0, similarity: 0.7,
                parent_id: None, chunk_index: 0,
                category: None, topic: None, archived: None,
            },
        ];
        let deduped = MemoryStore::dedup_by_parent(results);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].id, "id1"); // best from parent1
        assert_eq!(deduped[1].id, "id3"); // standalone
    }

    #[test]
    fn needs_chunking_threshold() {
        use crate::chunker::{split_text, DEFAULT_CHUNK_SIZE};
        let short = "Hello";
        let long = "A".repeat(DEFAULT_CHUNK_SIZE + 100);
        assert_eq!(split_text(short, DEFAULT_CHUNK_SIZE, 200).len(), 1);
        assert!(split_text(&long, DEFAULT_CHUNK_SIZE, 200).len() >= 2);
    }

    // ── MemoryStore::is_available ────────────────────────────────────────────

    fn make_config(enabled: bool) -> MemoryConfig {
        MemoryConfig {
            enabled,
            embed_dim: None,
            fts_language: None,
            pinned_budget_tokens: 2000,
            compression_age_days: 30,
        }
    }

    #[test]
    fn config_enabled_flag() {
        let cfg = make_config(true);
        assert!(cfg.enabled);
    }

    #[test]
    fn config_disabled_flag() {
        let cfg = make_config(false);
        assert!(!cfg.enabled);
    }


    // ── dedup_by_parent edge cases ───────────────────────────────────────────

    fn make_result(id: &str, parent_id: Option<&str>, similarity: f64) -> MemoryResult {
        MemoryResult {
            id: id.into(),
            content: String::new(),
            source: String::new(),
            pinned: false,
            relevance_score: 1.0,
            similarity,
            parent_id: parent_id.map(|s| s.to_string()),
            chunk_index: 0,
            category: None,
            topic: None,
            archived: None,
        }
    }

    #[test]
    fn dedup_empty_input() {
        let results = MemoryStore::dedup_by_parent(vec![]);
        assert!(results.is_empty());
    }

    #[test]
    fn dedup_all_standalone() {
        let results = vec![
            make_result("a", None, 0.9),
            make_result("b", None, 0.8),
            make_result("c", None, 0.7),
        ];
        let deduped = MemoryStore::dedup_by_parent(results);
        assert_eq!(deduped.len(), 3);
    }

    #[test]
    fn dedup_three_chunks_same_parent() {
        let results = vec![
            make_result("c1", Some("p1"), 0.9),
            make_result("c2", Some("p1"), 0.8),
            make_result("c3", Some("p1"), 0.7),
        ];
        let deduped = MemoryStore::dedup_by_parent(results);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].id, "c1");
    }

    #[test]
    fn dedup_parent_chunk_itself() {
        // Parent (parent_id=None) and its children (parent_id=parent.id)
        // Parent's dedup key = its own id. Children's dedup key = parent_id.
        // These are DIFFERENT keys unless parent_id == parent.id
        let results = vec![
            make_result("parent", None, 0.95),     // key = "parent"
            make_result("child1", Some("parent"), 0.90), // key = "parent"
        ];
        let deduped = MemoryStore::dedup_by_parent(results);
        assert_eq!(deduped.len(), 1); // both have key "parent"
        assert_eq!(deduped[0].id, "parent");
    }

    #[test]
    fn dedup_preserves_order() {
        let results = vec![
            make_result("a", None, 0.9),
            make_result("b", Some("x"), 0.8),
            make_result("c", None, 0.7),
            make_result("d", Some("x"), 0.6),
        ];
        let deduped = MemoryStore::dedup_by_parent(results);
        assert_eq!(deduped.len(), 3); // a, b (first from x), c
        assert_eq!(deduped[0].id, "a");
        assert_eq!(deduped[1].id, "b");
        assert_eq!(deduped[2].id, "c");
    }

    // ── MemoryConfig serde ───────────────────────────────────────────────────

    #[test]
    fn config_all_fields() {
        let cfg: MemoryConfig = serde_json::from_str(r#"{
            "enabled": false,
            "embed_dim": 768,
            "fts_language": "english"
        }"#).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.embed_dim.unwrap(), 768);
        assert_eq!(cfg.fts_language.unwrap(), "english");
    }
}

