//! Memory-related internal tools — extracted from engine.rs for readability.

use super::*;

// ── L0 Memory Context ────────────────────────────────────────────────────────

/// Result of L0 pinned chunk loading.
pub(super) struct MemoryContext {
    /// Formatted text to append to system prompt (empty if no pinned chunks).
    pub pinned_text: String,
    /// IDs of pinned chunks already loaded (for L2 dedup).
    pub pinned_ids: Vec<String>,
}

impl AgentEngine {
    /// Build L0 memory context: load pinned chunks for this agent.
    /// Called from build_context() in engine.rs before the system prompt size log.
    pub(super) async fn build_memory_context(&self, budget_tokens: u32) -> MemoryContext {
        if !self.memory_store.is_available() {
            return MemoryContext { pinned_text: String::new(), pinned_ids: vec![] };
        }
        match self.memory_store.load_pinned(&self.agent.name, budget_tokens).await {
            Ok((text, ids)) => MemoryContext { pinned_text: text, pinned_ids: ids },
            Err(e) => {
                tracing::warn!(error = %e, "failed to load pinned memory chunks");
                MemoryContext { pinned_text: String::new(), pinned_ids: vec![] }
            }
        }
    }

    /// Index extracted facts into memory (called after compaction).
    /// Uses batch embedding for efficiency when multiple facts are available.
    pub(super) async fn index_facts_to_memory(&self, facts: &[String]) {
        if !self.memory_store.is_available() {
            return;
        }
        let items: Vec<(String, String, bool)> = facts
            .iter()
            .filter(|f| !f.trim().is_empty())
            .map(|f| (f.clone(), "compaction".to_string(), false))
            .collect();
        if items.is_empty() {
            return;
        }
        match self.memory_store.index_batch(&items).await {
            Ok(ids) => tracing::info!(count = ids.len(), "batch indexed facts to memory"),
            Err(e) => {
                tracing::warn!(error = %e, "batch index failed, falling back to individual inserts");
                let mut ok = 0usize;
                let mut fail = 0usize;
                for (content, source, pinned) in &items {
                    match self.memory_store.index(content, source, *pinned, None, None).await {
                        Ok(_) => ok += 1,
                        Err(ie) => {
                            fail += 1;
                            tracing::warn!(error = %ie, "individual fact index failed");
                        }
                    }
                }
                tracing::info!(ok, fail, "individual fact indexing complete");
            }
        }
    }

    /// Internal tool: search long-term memory.
    pub(super) async fn handle_memory_search(&self, args: &serde_json::Value) -> String {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let category = args.get("category").and_then(|v| v.as_str());
        let topic = args.get("topic").and_then(|v| v.as_str());

        if query.is_empty() {
            return "Error: 'query' is required".to_string();
        }

        let mut parts: Vec<String> = Vec::new();

        // Search session-scoped documents first (per-conversation RAG)
        let session_id = args.get("_context")
            .and_then(|c| c.get("session_id"))
            .and_then(|s| s.as_str())
            .and_then(|s| uuid::Uuid::parse_str(s).ok());

        if let (Some(sid), Ok(embedding)) = (session_id, self.memory_store.embed(query).await) {
            let vec_str = format!("[{}]", embedding.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","));
            if let Ok(docs) = self.memory_store.search_session_documents(sid, &vec_str, 3).await
                && !docs.is_empty() {
                    let doc_body = docs.iter().enumerate()
                        .map(|(i, (filename, content, score))| format!("{}. [{}] {} (score: {:.2})", i + 1, filename, content, score))
                        .collect::<Vec<_>>().join("\n");
                    parts.push(format!("[Session documents]\n{}", doc_body));
                }
        }

        // Search long-term memory (exclude L0 pinned chunks to avoid duplication)
        let exclude = self.tex().pinned_chunk_ids.lock().await.clone();
        match self.memory_store.search(query, limit, &exclude, category, topic).await {
            Ok((results, _)) if results.is_empty() && parts.is_empty() => {
                return "No relevant memories found.".to_string();
            }
            Ok((results, mode)) => {
                let header = if mode == "fts" { "[FTS fallback] " } else { "" };
                let body = results
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let pin = if r.pinned { "📌 " } else { "" };
                        format!("{}. [{}] {}{}  (id: {})", i + 1, r.source, pin, r.content, r.id)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !body.is_empty() {
                    parts.push(format!("{}[Memory]\n{}", header, body));
                }
            }
            Err(e) if parts.is_empty() => return format!("Memory search error: {}", e),
            Err(_) => {} // session docs available, ignore memory error
        }

        parts.join("\n\n")
    }

    /// Internal tool: index content into long-term memory.
    pub(super) async fn handle_memory_index(&self, args: &serde_json::Value) -> String {
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("manual");
        let pinned = args.get("pinned").and_then(|v| v.as_bool()).unwrap_or(false);
        let category = args.get("category").and_then(|v| v.as_str());
        let topic = args.get("topic").and_then(|v| v.as_str());

        if content.is_empty() {
            return "Error: 'content' is required".to_string();
        }
        if !self.memory_store.is_available() {
            return "Memory indexing is not available (embedding endpoint not configured).".to_string();
        }

        // Validate category if provided
        const VALID_CATEGORIES: &[&str] = &["decision", "preference", "event", "discovery", "advice", "general"];
        if let Some(cat) = category {
            if !VALID_CATEGORIES.contains(&cat) {
                return format!(
                    "Error: invalid category '{}'. Valid values: {}",
                    cat,
                    VALID_CATEGORIES.join(", ")
                );
            }
        }

        match self.memory_store.index(content, source, pinned, category, topic).await {
            Ok(id) => {
                // Build (chunk_id, chunk_content) pairs for GraphRAG.
                // One query: parent (id match) + children (parent_id match).
                // For single-chunk docs, returns just the parent row.
                let chunks_for_graph = self.memory_store.fetch_chunks_for_graph(&id)
                    .await
                    .unwrap_or_else(|_| vec![(id.clone(), content.to_string())]);

                let chunk_count = chunks_for_graph.len();
                let _ = self.memory_store.spawn_graph_extraction(
                    chunks_for_graph,
                    self.provider.clone(),
                ).await;

                if chunk_count > 1 {
                    format!("Indexed as {} ({} chunks)", id, chunk_count)
                } else {
                    format!("Indexed as {}", id)
                }
            }
            Err(e) => format!("Memory index error: {}", e),
        }
    }

    /// Internal tool: bulk re-index all .md/.txt files from the entire workspace into memory.
    /// Scans the whole workspace (excluding system dirs). Returns immediately — worker processes async.
    pub(super) async fn handle_memory_reindex(&self, args: &serde_json::Value) -> String {
        let clear_existing = args.get("clear_existing").and_then(|v| v.as_bool()).unwrap_or(false);
        let include_sessions = args.get("include_sessions").and_then(|v| v.as_bool()).unwrap_or(true);
        let _extract_graph = args.get("graph").and_then(|v| v.as_bool()).unwrap_or(true);

        if !self.memory_store.is_available() {
            return "Memory indexing is not available (embedding endpoint not configured).".to_string();
        }

        let workspace_root = std::path::PathBuf::from(&self.workspace_dir);
        if !workspace_root.exists() {
            return "Workspace directory not found.".to_string();
        }

        // Count indexable files for user feedback (entire workspace, skip system dirs)
        let mut file_count = 0usize;
        let exclude_dirs = crate::agent::workspace::MEMORY_INDEX_EXCLUDE_DIRS;
        let mut stack = vec![workspace_root.clone()];
        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let rel = path.strip_prefix(&workspace_root).ok()
                        .and_then(|p| p.components().next())
                        .and_then(|c| c.as_os_str().to_str())
                        .unwrap_or("");
                    if !name.starts_with('.') && !exclude_dirs.contains(&rel) {
                        stack.push(path);
                    }
                } else if matches!(path.extension().and_then(|e| e.to_str()), Some("md") | Some("txt")) {
                    file_count += 1;
                }
            }
        }

        // Clear existing memory synchronously (fast DB operation)
        if clear_existing {
            match self.memory_store.wipe_agent_memory(&self.agent.name).await {
                Ok(deleted) => tracing::info!(deleted, agent = %self.agent.name, "cleared memory before reindex"),
                Err(e) => return format!("Failed to clear memory: {}", e),
            }
        }

        // Create reindex task for memory-worker
        let task_id = match self.memory_store.enqueue_reindex_task(serde_json::json!({
            "clear_existing": clear_existing,
            "include_sessions": include_sessions,
            "agent_id": self.agent.name,
        })).await {
            Ok(id) => id,
            Err(e) => return format!("Failed to create reindex task: {}", e),
        };

        format!(
            "Reindex task created: ~{} indexable files in workspace{}. Task ID: {}. Worker will process.",
            file_count,
            if include_sessions { " + session transcripts" } else { "" },
            task_id
        )
    }

    /// Internal tool: query the knowledge graph for entity relations.
    pub(super) async fn handle_graph_query(&self, args: &serde_json::Value) -> String {
        let entity = match args.get("entity").and_then(|v| v.as_str()) {
            Some(e) if !e.is_empty() => e,
            _ => return "Error: 'entity' is required".to_string(),
        };
        let max_hops = args
            .get("max_hops")
            .and_then(|v| v.as_u64())
            .unwrap_or(2)
            .min(3) as u8;

        match self.memory_store.find_graph_related(entity, max_hops).await {
            Ok(related) if related.is_empty() => {
                format!("No relations found for entity '{}'.", entity)
            }
            Ok(related) => {
                let lines: Vec<String> = related
                    .iter()
                    .map(|e| format!("- {} ({})", e.name, e.entity_type))
                    .collect();
                format!(
                    "Entities related to '{}' (within {} hops):\n{}",
                    entity,
                    max_hops,
                    lines.join("\n")
                )
            }
            Err(e) => format!("Graph query error: {}", e),
        }
    }

    /// Internal tool: get memory chunks by ID or source.
    pub(super) async fn handle_memory_get(&self, args: &serde_json::Value) -> String {
        let chunk_id = args.get("chunk_id").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
        let source = args.get("source").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

        match self.memory_store.get(chunk_id, source, limit).await {
            Ok(chunks) if chunks.is_empty() => "No memory chunks found.".to_string(),
            Ok(chunks) => chunks
                .iter()
                .map(|c| {
                    let pin = if c.pinned { "📌 " } else { "" };
                    format!(
                        "[{}] {}(score:{:.2}) {}\n  id: {} | created: {}",
                        c.source, pin, c.relevance_score, c.content,
                        c.id, c.created_at.format("%Y-%m-%d %H:%M")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            Err(e) => format!("Memory get error: {}", e),
        }
    }

    /// Internal tool: delete a memory chunk by UUID.
    pub(super) async fn handle_memory_delete(&self, args: &serde_json::Value) -> String {
        let chunk_id = match args.get("chunk_id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id,
            _ => return "Error: 'chunk_id' is required".to_string(),
        };

        match self.memory_store.delete(chunk_id).await {
            Ok(true) => format!("Deleted memory chunk {}", chunk_id),
            Ok(false) => format!("Memory chunk {} not found", chunk_id),
            Err(e) => format!("Error deleting memory chunk: {}", e),
        }
    }

    /// Internal tool: add/update/remove an entry in the agent's MEMORY.md file.
    pub(super) async fn handle_memory_update(&self, args: &serde_json::Value) -> String {
        let section = match args.get("section").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return "Error: 'section' is required".to_string(),
        };
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("add");
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => return "Error: 'content' is required".to_string(),
        };

        // Atomic read-modify-write: hold lock for the entire operation
        let _lock = self.tex().memory_md_lock.lock().await;

        let memory_path = std::path::Path::new(&self.workspace_dir)
            .join("agents")
            .join(&self.agent.name)
            .join("MEMORY.md");

        let existing = tokio::fs::read_to_string(&memory_path).await.unwrap_or_default();

        let updated = match action {
            "add" => {
                let section_header = format!("# {}", section);
                if existing.contains(&section_header) {
                    existing.replacen(
                        &section_header,
                        &format!("{}\n- {}", section_header, content),
                        1,
                    )
                } else {
                    format!("{}\n# {}\n- {}\n", existing.trim_end(), section, content)
                }
            }
            "update" => {
                let lines: Vec<String> = existing
                    .lines()
                    .map(|l| {
                        let key = content.split(':').next().unwrap_or(&content).trim();
                        if l.starts_with("- ") && l.contains(key) {
                            format!("- {}", content)
                        } else {
                            l.to_string()
                        }
                    })
                    .collect();
                lines.join("\n")
            }
            "remove" => {
                let lines: Vec<&str> = existing
                    .lines()
                    .filter(|l| !l.contains(&content))
                    .collect();
                lines.join("\n")
            }
            _ => return format!("Unknown action '{}'. Use: add, update, remove", action),
        };

        // Guard against unbounded growth
        const MAX_MEMORY_MD_BYTES: usize = 8 * 1024;
        if updated.len() > MAX_MEMORY_MD_BYTES {
            return format!(
                "Error: MEMORY.md would exceed {} KB limit ({} KB). Remove old entries first or use memory_index for large data.",
                MAX_MEMORY_MD_BYTES / 1024,
                updated.len() / 1024
            );
        }

        match tokio::fs::write(&memory_path, &updated).await {
            Ok(_) => format!(
                "MEMORY.md updated ({} in section '{}'):\n- {}",
                action, section, content
            ),
            Err(e) => format!("Error writing MEMORY.md: {}", e),
        }
    }

    /// Internal tool: on-demand compression of old memory chunks by topic.
    /// Fetches compressible groups for this agent (optionally filtered by topic),
    /// runs LLM summarization via compress_group, and returns compressed chunk count.
    pub(super) async fn handle_memory_compress(&self, args: &serde_json::Value) -> String {
        let topic_filter = args.get("topic").and_then(|v| v.as_str());
        let agent_id = &self.agent.name;
        let age_days = self.app_config.memory.compression_age_days;

        if !self.memory_store.is_available() {
            return "Memory compression is not available (embedding endpoint not configured).".to_string();
        }

        let groups = match self.memory_store.fetch_compressible_groups(age_days).await {
            Ok(g) => g,
            Err(e) => return format!("{{\"error\": \"Failed to fetch compressible groups: {}\"}}", e),
        };

        // Filter to this agent's groups, optionally by topic
        let filtered: Vec<_> = groups
            .into_iter()
            .filter(|g| g.agent_id == *agent_id)
            .filter(|g| topic_filter.map_or(true, |t| g.topic == t))
            .collect();

        if filtered.is_empty() {
            return "{\"compressed\": 0, \"topics\": []}".to_string();
        }

        let mut total_compressed = 0u64;
        let mut topics_done: Vec<String> = Vec::new();

        for group in filtered {
            let topic_name = group.topic.clone();
            match self.memory_store.compress_memory_group(
                self.provider.clone(),
                group,
            )
            .await
            {
                Ok(count) => {
                    total_compressed += count;
                    topics_done.push(topic_name);
                }
                Err(e) => {
                    tracing::warn!(topic = %topic_name, error = %e, "handle_memory_compress: compression failed for topic");
                }
            }
        }

        serde_json::json!({
            "compressed": total_compressed,
            "topics": topics_done
        })
        .to_string()
    }
}
