use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post, put, delete, patch},
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/memory", get(api_list_memory).post(api_create_memory))
        .route("/api/memory/stats", get(api_memory_stats))
        .route("/api/memory/graph", get(api_memory_graph))
        .route("/api/memory/export", get(api_export_memory))
        .route("/api/memory/fts-language", get(api_get_fts_language).put(api_set_fts_language))
        .route("/api/memory/{id}", delete(api_delete_memory).patch(api_patch_memory))
        .route("/api/memory/tasks", get(api_memory_tasks))
        .route("/api/memory/extraction-queue", get(api_extraction_queue))
        .route("/api/memory/categories", get(api_memory_categories))
        .route("/api/memory/topics", get(api_memory_topics))
        .route("/api/memory/documents", get(api_list_documents))
        .route("/api/memory/documents/{id}", get(api_get_document).patch(api_patch_document).delete(api_delete_memory))
}

// ── Memory API ──

#[derive(Debug, Deserialize)]
pub(crate) struct MemoryQuery {
    query: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

pub(crate) async fn api_list_memory(
    State(state): State<AppState>,
    Query(q): Query<MemoryQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(20).min(100) as usize;
    let offset = q.offset.unwrap_or(0).max(0);

    // Search with query: semantic → FTS fallback (handled inside MemoryStore::search)
    if let Some(ref search) = q.query
        && !search.trim().is_empty() {
            match state.memory_store.search(search, limit, &[], None, None).await {
                Ok((results, mode)) => {
                    let chunks: Vec<Value> = results
                        .iter()
                        .map(|r| {
                            json!({
                                "id": r.id,
                                "content": r.content,
                                "source": r.source,
                                "relevance_score": r.relevance_score,
                                "similarity": r.similarity,
                                "pinned": r.pinned,
                                "parent_id": r.parent_id,
                                "chunk_index": r.chunk_index,
                            })
                        })
                        .collect();
                    return Json(json!({ "chunks": chunks, "search_mode": mode })).into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": e.to_string()})),
                    ).into_response();
                }
            }
        }

    // No query: list all chunks by relevance
    let result = sqlx::query_as::<_, MemoryChunkRow>(
        "SELECT id, content, source, relevance_score, pinned, created_at, accessed_at, parent_id, chunk_index \
         FROM memory_chunks ORDER BY relevance_score DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit as i64)
    .bind(offset)
    .fetch_all(&state.db)
    .await;

    match result {
        Ok(rows) => {
            let chunks: Vec<Value> = rows
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "content": c.content,
                        "source": c.source,
                        "relevance_score": c.relevance_score,
                        "pinned": c.pinned,
                        "created_at": c.created_at.to_rfc3339(),
                        "accessed_at": c.accessed_at.to_rfc3339(),
                        "parent_id": c.parent_id,
                        "chunk_index": c.chunk_index,
                    })
                })
                .collect();
            Json(json!({ "chunks": chunks })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct MemoryChunkRow {
    id: uuid::Uuid,
    content: String,
    source: Option<String>,
    relevance_score: f64,
    pinned: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    accessed_at: chrono::DateTime<chrono::Utc>,
    parent_id: Option<uuid::Uuid>,
    chunk_index: i32,
}

pub(crate) async fn api_memory_stats(State(state): State<AppState>) -> Json<Value> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_chunks")
        .fetch_one(&state.db).await
        .inspect_err(|e| tracing::error!(error = %e, "stats: failed to count chunks"))
        .unwrap_or(0);

    let documents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_chunks WHERE parent_id IS NULL")
        .fetch_one(&state.db).await
        .inspect_err(|e| tracing::error!(error = %e, "stats: failed to count documents"))
        .unwrap_or(0);

    let pinned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_chunks WHERE pinned = true AND parent_id IS NULL")
        .fetch_one(&state.db).await
        .inspect_err(|e| tracing::error!(error = %e, "stats: failed to count pinned"))
        .unwrap_or(0);

    let avg_score: f64 = sqlx::query_scalar("SELECT COALESCE(AVG(relevance_score), 0) FROM memory_chunks WHERE parent_id IS NULL")
        .fetch_one(&state.db).await
        .inspect_err(|e| tracing::error!(error = %e, "stats: failed to get avg score"))
        .unwrap_or(0.0);

    // Graph metrics (from relational graph tables)
    let graph_entities: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM graph_entities")
        .fetch_one(&state.db).await.unwrap_or(0);
    let graph_edges: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM graph_edges WHERE invalid_at IS NULL")
        .fetch_one(&state.db).await.unwrap_or(0);
    let graph_top_types: Vec<(String, i64)> = sqlx::query_as(
        "SELECT entity_type, COUNT(*) FROM graph_entities GROUP BY entity_type ORDER BY count DESC LIMIT 6"
    ).fetch_all(&state.db).await.unwrap_or_default();
    let graph_per_agent: Vec<(String, i64)> = crate::memory_graph::get_entity_counts_by_agent(&state.db)
        .await.unwrap_or_default();

    let (q_pending, q_processing, q_done, q_failed) = crate::graph_worker::queue_status(&state.db)
        .await.unwrap_or((0, 0, 0, 0));

    let (t_pending, t_processing, t_done, t_failed) = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        "SELECT
            COUNT(*) FILTER (WHERE status = 'pending'),
            COUNT(*) FILTER (WHERE status = 'processing'),
            COUNT(*) FILTER (WHERE status = 'done'),
            COUNT(*) FILTER (WHERE status = 'failed')
         FROM memory_tasks"
    ).fetch_one(&state.db).await.unwrap_or((0, 0, 0, 0));

    let embed_dim = state.memory_store.embed_dim();
    let embed_model = state.memory_store.embed_model_name();

    Json(json!({
        "total": documents,
        "total_chunks": total,
        "pinned": pinned,
        "avg_score": avg_score,
        "embed_model": if embed_model.is_empty() { None } else { Some(&embed_model) },
        "embed_dim": if embed_dim > 0 { Some(embed_dim) } else { None },
        "graph": {
            "entities": graph_entities,
            "edges": graph_edges,
            "types": graph_top_types.into_iter().map(|(t, c)| json!({"type": t, "count": c})).collect::<Vec<_>>(),
            "per_agent": graph_per_agent.into_iter().map(|(a, c)| json!({"agent": a, "entities": c})).collect::<Vec<_>>(),
        },
        "extraction_queue": {
            "pending": q_pending,
            "processing": q_processing,
            "done": q_done,
            "failed": q_failed,
        },
        "tasks": {
            "pending": t_pending,
            "processing": t_processing,
            "done": t_done,
            "failed": t_failed,
        },
    }))
}


/// GET /api/memory/extraction-queue — queue status
pub(crate) async fn api_extraction_queue(State(state): State<AppState>) -> Json<Value> {
    let (pending, processing, done, failed) = crate::graph_worker::queue_status(&state.db)
        .await.unwrap_or((0, 0, 0, 0));
    Json(json!({
        "pending": pending, "processing": processing,
        "done": done, "failed": failed,
        "total": pending + processing + done + failed,
    }))
}

/// GET /api/memory/tasks — list memory worker tasks
pub(crate) async fn api_memory_tasks(State(state): State<AppState>) -> Json<Value> {
    let rows = sqlx::query_as::<_, (uuid::Uuid, String, String, serde_json::Value, Option<String>, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, task_type, status, params, error, created_at FROM memory_tasks ORDER BY created_at DESC LIMIT 50"
    ).fetch_all(&state.db).await.unwrap_or_default();
    let tasks: Vec<Value> = rows.iter().map(|(id, tt, st, p, e, ca)| json!({
        "id": id, "task_type": tt, "status": st, "params": p, "error": e, "created_at": ca.to_rfc3339()
    })).collect();
    Json(json!({"tasks": tasks}))
}

// ── Taxonomy API (categories and topics) ──

#[derive(Debug, Deserialize)]
pub(crate) struct TaxonomyQuery {
    agent_id: Option<String>,
}

/// GET /api/memory/categories — distinct categories with chunk counts
pub(crate) async fn api_memory_categories(
    State(state): State<AppState>,
    Query(q): Query<TaxonomyQuery>,
) -> impl IntoResponse {
    let result: Result<Vec<(String, i64)>, _> = if let Some(agent_id) = q.agent_id.as_deref() {
        sqlx::query_as(
            "SELECT category, COUNT(*)::bigint AS count \
             FROM memory_chunks \
             WHERE category IS NOT NULL AND parent_id IS NULL AND agent_id = $1 \
             GROUP BY category \
             ORDER BY count DESC",
        )
        .bind(agent_id)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as(
            "SELECT category, COUNT(*)::bigint AS count \
             FROM memory_chunks \
             WHERE category IS NOT NULL AND parent_id IS NULL \
             GROUP BY category \
             ORDER BY count DESC",
        )
        .fetch_all(&state.db)
        .await
    };

    match result {
        Ok(rows) => {
            let categories: Vec<Value> = rows
                .iter()
                .map(|(cat, count)| json!({"category": cat, "count": count}))
                .collect();
            Json(json!(categories)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// GET /api/memory/topics — distinct topics with chunk counts
pub(crate) async fn api_memory_topics(
    State(state): State<AppState>,
    Query(q): Query<TaxonomyQuery>,
) -> impl IntoResponse {
    let result: Result<Vec<(String, i64)>, _> = if let Some(agent_id) = q.agent_id.as_deref() {
        sqlx::query_as(
            "SELECT topic, COUNT(*)::bigint AS count \
             FROM memory_chunks \
             WHERE topic IS NOT NULL AND parent_id IS NULL AND agent_id = $1 \
             GROUP BY topic \
             ORDER BY count DESC",
        )
        .bind(agent_id)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as(
            "SELECT topic, COUNT(*)::bigint AS count \
             FROM memory_chunks \
             WHERE topic IS NOT NULL AND parent_id IS NULL \
             GROUP BY topic \
             ORDER BY count DESC",
        )
        .fetch_all(&state.db)
        .await
    };

    match result {
        Ok(rows) => {
            let topics: Vec<Value> = rows
                .iter()
                .map(|(top, count)| json!({"topic": top, "count": count}))
                .collect();
            Json(json!(topics)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Documents API (document-level view) ──

#[derive(Debug, sqlx::FromRow)]
struct DocumentRow {
    id: uuid::Uuid,
    source: Option<String>,
    pinned: bool,
    relevance_score: f64,
    created_at: chrono::DateTime<chrono::Utc>,
    accessed_at: chrono::DateTime<chrono::Utc>,
    preview: Option<String>,
    chunks_count: i64,
    total_chars: Option<i64>,
    category: Option<String>,
    topic: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocumentsQuery {
    query: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    category: Option<String>,
    topic: Option<String>,
}

pub(crate) async fn api_list_documents(
    State(state): State<AppState>,
    Query(q): Query<DocumentsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(20).min(100);
    let offset = q.offset.unwrap_or(0).max(0);

    // Search mode: search at chunk level, group by document
    if let Some(ref search) = q.query
        && !search.trim().is_empty() {
            return match state.memory_store.search(search, (limit * 5) as usize, &[], None, None).await {
                Ok((results, mode)) => {
                    // Group by document: COALESCE(parent_id, id), keep best similarity
                    let mut seen = std::collections::HashMap::<String, (f64, &crate::memory::MemoryResult)>::new();
                    for r in &results {
                        let doc_id = r.parent_id.as_deref().unwrap_or(&r.id).to_string();
                        match seen.entry(doc_id) {
                            std::collections::hash_map::Entry::Vacant(e) => { e.insert((r.similarity, r)); }
                            std::collections::hash_map::Entry::Occupied(mut e) => {
                                if r.similarity > e.get().0 { e.insert((r.similarity, r)); }
                            }
                        }
                    }
                    let total_found = seen.len() as i64;
                    let mut docs: Vec<_> = seen.into_values().collect();
                    docs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                    let page: Vec<_> = docs.into_iter().skip(offset as usize).take(limit as usize).collect();

                    // Batch fetch metadata for all docs in one query
                    let doc_ids: Vec<String> = page.iter().map(|(_, r)| r.parent_id.as_deref().unwrap_or(&r.id).to_string()).collect();
                    let meta_rows: Vec<(uuid::Uuid, i64, Option<i64>, Option<String>)> = if !doc_ids.is_empty() {
                        sqlx::query_as(
                            "SELECT m.id, \
                               (SELECT COUNT(*) FROM memory_chunks WHERE parent_id = m.id) + 1, \
                               (SELECT SUM(LENGTH(content)) FROM memory_chunks WHERE id = m.id OR parent_id = m.id), \
                               LEFT(m.content, 200) \
                             FROM memory_chunks m WHERE m.id = ANY($1::uuid[])"
                        )
                        .bind(&doc_ids)
                        .fetch_all(&state.db)
                        .await
                        .unwrap_or_default()
                    } else { vec![] };
                    let meta_map: std::collections::HashMap<String, (i64, Option<i64>, Option<String>)> =
                        meta_rows.into_iter().map(|(id, cnt, chars, prev)| (id.to_string(), (cnt, chars, prev))).collect();

                    let documents: Vec<Value> = page.iter().map(|(sim, r)| {
                        let doc_id = r.parent_id.as_deref().unwrap_or(&r.id);
                        let (chunks_count, total_chars, preview) = meta_map.get(doc_id)
                            .cloned()
                            .unwrap_or((1, None, None));
                        json!({
                            "id": doc_id,
                            "source": r.source,
                            "pinned": r.pinned,
                            "relevance_score": r.relevance_score,
                            "similarity": sim,
                            "preview": preview.unwrap_or_else(|| r.content.chars().take(200).collect()),
                            "chunks_count": chunks_count,
                            "total_chars": total_chars,
                            "category": r.category,
                            "topic": r.topic,
                        })
                    }).collect();
                    Json(json!({ "documents": documents, "total": total_found, "search_mode": mode })).into_response()
                }
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
            };
        }

    // List mode: CTE to avoid correlated subqueries
    // Build dynamic WHERE clause for category/topic filters
    let mut where_extra = String::new();
    let mut bind_idx = 3u32; // $1=limit, $2=offset
    if q.category.is_some() {
        where_extra.push_str(&format!(" AND m.category = ${bind_idx}"));
        bind_idx += 1;
    }
    if q.topic.is_some() {
        where_extra.push_str(&format!(" AND m.topic = ${bind_idx}"));
    }

    let sql = format!(
        "WITH doc_stats AS ( \
           SELECT parent_id, COUNT(*) AS child_count, SUM(LENGTH(content)) AS child_chars \
           FROM memory_chunks WHERE parent_id IS NOT NULL \
           GROUP BY parent_id \
         ) \
         SELECT \
           m.id, m.source, m.pinned, \
           COALESCE(m.relevance_score, 1.0) AS relevance_score, \
           m.created_at, COALESCE(m.accessed_at, m.created_at) AS accessed_at, \
           LEFT(m.content, 200) AS preview, \
           COALESCE(ds.child_count, 0) + 1 AS chunks_count, \
           COALESCE(ds.child_chars, 0) + LENGTH(m.content) AS total_chars, \
           m.category, m.topic \
         FROM memory_chunks m \
         LEFT JOIN doc_stats ds ON ds.parent_id = m.id \
         WHERE m.parent_id IS NULL{where_extra} \
         ORDER BY COALESCE(m.accessed_at, m.created_at) DESC \
         LIMIT $1 OFFSET $2"
    );

    let mut query = sqlx::query_as::<_, DocumentRow>(&sql)
        .bind(limit)
        .bind(offset);
    if let Some(ref cat) = q.category {
        query = query.bind(cat);
    }
    if let Some(ref top) = q.topic {
        query = query.bind(top);
    }
    let rows = query.fetch_all(&state.db).await;

    match rows {
        Ok(rows) => {
            let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_chunks WHERE parent_id IS NULL")
                .fetch_one(&state.db).await.unwrap_or(0);
            let documents: Vec<Value> = rows.iter().map(|r| json!({
                "id": r.id,
                "source": r.source,
                "pinned": r.pinned,
                "relevance_score": r.relevance_score,
                "created_at": r.created_at.to_rfc3339(),
                "accessed_at": r.accessed_at.to_rfc3339(),
                "preview": r.preview,
                "chunks_count": r.chunks_count,
                "total_chars": r.total_chars,
                "category": r.category,
                "topic": r.topic,
            })).collect();
            Json(json!({ "documents": documents, "total": total })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

pub(crate) async fn api_get_document(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, (String, i32)>(
        "SELECT content, chunk_index FROM memory_chunks \
         WHERE id = $1 OR parent_id = $1 \
         ORDER BY chunk_index"
    )
    .bind(id)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(rows) if !rows.is_empty() => {
            let content: String = rows.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>().join("\n");
            let total_chars = content.len();
            let meta = sqlx::query_as::<_, (Option<String>, bool, f64, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(
                "SELECT source, pinned, COALESCE(relevance_score,1.0), created_at, COALESCE(accessed_at,created_at) FROM memory_chunks WHERE id = $1"
            ).bind(id).fetch_optional(&state.db).await;
            let (source, pinned, score, created, accessed) = match meta {
                Ok(Some(m)) => m,
                Ok(None) => {
                    tracing::warn!(id = %id, "document chunks exist but parent metadata missing");
                    (None, false, 1.0, chrono::Utc::now(), chrono::Utc::now())
                }
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
                }
            };
            Json(json!({
                "id": id,
                "source": source,
                "pinned": pinned,
                "relevance_score": score,
                "created_at": created.to_rfc3339(),
                "accessed_at": accessed.to_rfc3339(),
                "content": content,
                "chunks_count": rows.len(),
                "total_chars": total_chars,
            })).into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({"error": "document not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

pub(crate) async fn api_patch_document(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(req): Json<PatchMemoryRequest>,
) -> impl IntoResponse {
    if let Some(pinned) = req.pinned {
        let result = sqlx::query("UPDATE memory_chunks SET pinned = $2 WHERE id = $1 OR parent_id = $1")
            .bind(id).bind(pinned).execute(&state.db).await;
        match result {
            Ok(r) if r.rows_affected() > 0 => {}
            Ok(_) => return (StatusCode::NOT_FOUND, Json(json!({"error": "document not found"}))).into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        }
    }
    Json(json!({"ok": true})).into_response()
}

// POST /api/memory — create a new memory chunk
#[derive(Debug, Deserialize)]
pub(crate) struct CreateMemoryRequest {
    content: String,
    source: Option<String>,
    pinned: Option<bool>,
}

pub(crate) async fn api_create_memory(
    State(state): State<AppState>,
    Json(req): Json<CreateMemoryRequest>,
) -> impl IntoResponse {
    if req.content.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "content must not be empty"})),
        )
            .into_response();
    }
    let source = req.source.as_deref().unwrap_or("ui");
    let pinned = req.pinned.unwrap_or(false);
    match state.memory_store.index(&req.content, source, pinned, None, None).await {
        Ok(id) => Json(json!({"id": id, "ok": true})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

pub(crate) async fn api_delete_memory(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> impl IntoResponse {
    let result = sqlx::query("DELETE FROM memory_chunks WHERE id = $1 OR parent_id = $1")
        .bind(id)
        .execute(&state.db)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            crate::db::audit::audit_spawn(state.db.clone(), String::new(), crate::db::audit::event_types::MEMORY_DELETED, None, json!({"chunk_id": id.to_string()}));
            Json(json!({"ok": true})).into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({"error": "chunk not found"}))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchMemoryRequest {
    pinned: Option<bool>,
    content: Option<String>,
}

pub(crate) async fn api_patch_memory(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(req): Json<PatchMemoryRequest>,
) -> impl IntoResponse {
    if req.pinned.is_none() && req.content.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "nothing to update"})),
        )
            .into_response();
    }

    // Validate content early — before any DB writes
    if let Some(ref content) = req.content
        && content.trim().is_empty() {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "content must not be empty"}))).into_response();
        }

    // Update pinned flag if provided
    if let Some(pinned) = req.pinned {
        let result = sqlx::query("UPDATE memory_chunks SET pinned = $2 WHERE id = $1")
            .bind(id)
            .bind(pinned)
            .execute(&state.db)
            .await;
        match result {
            Ok(r) if r.rows_affected() > 0 => {
                crate::db::audit::audit_spawn(state.db.clone(), String::new(), crate::db::audit::event_types::MEMORY_PINNED, None, json!({"chunk_id": id.to_string(), "pinned": pinned}));
            }
            Ok(_) => {
                return (StatusCode::NOT_FOUND, Json(json!({"error": "chunk not found"}))).into_response();
            }
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
            }
        }
    }

    // Update content if provided — re-embed and rebuild tsvector
    if let Some(ref content) = req.content {
        let embedding = match state.memory_store.embed(content).await {
            Ok(e) => e,
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("embedding failed: {e}")}))).into_response();
            }
        };
        let vec_str = crate::memory::MemoryStore::fmt_vec(&embedding);
        let lang = match state.memory_store.validated_fts_language() {
            Ok(l) => l,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "invalid FTS language configuration"}))).into_response(),
        };
        // SAFETY: `lang` comes from `validated_fts_language()` which allowlists lowercase ASCII identifiers only.
        let sql = format!(
            "UPDATE memory_chunks SET content = $2, embedding = $3::halfvec, tsv = to_tsvector('{lang}', $2) WHERE id = $1"
        );
        let result = sqlx::query(&sql)
            .bind(id)
            .bind(content)
            .bind(&vec_str)
            .execute(&state.db)
            .await;
        match result {
            Ok(r) if r.rows_affected() > 0 => {}
            Ok(_) => {
                return (StatusCode::NOT_FOUND, Json(json!({"error": "chunk not found"}))).into_response();
            }
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
            }
        }
    }

    Json(json!({"ok": true})).into_response()
}

// ── FTS Language API ──

/// GET /api/memory/fts-language — return current FTS language and available options.
pub(crate) async fn api_get_fts_language(State(state): State<AppState>) -> Json<Value> {
    let current = state.memory_store.fts_language();
    Json(json!({
        "language": current,
        "available": [
            "simple", "danish", "dutch", "english", "finnish", "french",
            "german", "hungarian", "italian", "norwegian", "portuguese",
            "romanian", "russian", "spanish", "swedish", "turkish"
        ]
    }))
}

/// PUT /api/memory/fts-language — change FTS language and rebuild tsvector index.
pub(crate) async fn api_set_fts_language(
    State(state): State<AppState>,
    Json(req): Json<Value>,
) -> impl IntoResponse {
    let lang = match req.get("language").and_then(|v| v.as_str()) {
        Some(l) => l.to_string(),
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "'language' is required"}))).into_response(),
    };

    // Validate
    let valid = [
        "simple", "danish", "dutch", "english", "finnish", "french",
        "german", "hungarian", "italian", "norwegian", "portuguese",
        "romanian", "russian", "spanish", "swedish", "turkish",
    ];
    if !valid.contains(&lang.as_str()) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("unsupported language: {}", lang)}))).into_response();
    }

    state.memory_store.set_fts_language(&lang);

    match state.memory_store.rebuild_fts().await {
        Ok(rows) => Json(json!({
            "ok": true,
            "language": lang,
            "rows_rebuilt": rows,
        })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ).into_response(),
    }
}

// ── Memory Export ──

/// GET /api/memory/export — bulk export all memory chunks (without embeddings).
/// Limited to 100k chunks to prevent OOM.
pub(crate) async fn api_export_memory(
    State(state): State<AppState>,
) -> impl IntoResponse {
    const EXPORT_LIMIT: i64 = 100_000;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_chunks")
        .fetch_one(&state.db).await.unwrap_or(0);
    if total > EXPORT_LIMIT {
        tracing::warn!(total, limit = EXPORT_LIMIT, "memory export truncated");
    }
    match sqlx::query_as::<_, (uuid::Uuid, String, Option<String>, bool, f64, chrono::DateTime<chrono::Utc>, Option<uuid::Uuid>, i32)>(
        "SELECT id, content, source, pinned, relevance_score, created_at, parent_id, chunk_index \
         FROM memory_chunks ORDER BY created_at LIMIT $1",
    )
    .bind(EXPORT_LIMIT)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => {
            let chunks: Vec<Value> = rows
                .iter()
                .map(|r| {
                    json!({
                        "id": r.0,
                        "content": r.1,
                        "source": r.2,
                        "pinned": r.3,
                        "relevance_score": r.4,
                        "created_at": r.5.to_rfc3339(),
                        "parent_id": r.6,
                        "chunk_index": r.7,
                    })
                })
                .collect();
            Json(json!({ "chunks": chunks, "total": chunks.len() })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Graph API ──

#[derive(Debug, serde::Deserialize)]
pub(crate) struct GraphQuery {
    limit: Option<i64>,
    #[allow(dead_code)]
    agent: Option<String>,
}

#[derive(serde::Serialize)]
struct GraphApiNode {
    id: String,
    kind: String, // "chunk" | "entity"
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entity_type: Option<String>,
}

#[derive(serde::Serialize)]
struct GraphApiEdge {
    from: String,
    to: String,
    kind: String,
}

pub(crate) async fn api_memory_graph(
    State(state): State<AppState>,
    Query(q): Query<GraphQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(150).min(500);

    // 1. Fetch documents (parent chunks only)
    let doc_rows = sqlx::query_as::<_, MemoryChunkRow>(
        "SELECT id, content, source, relevance_score, pinned, created_at, accessed_at, parent_id, chunk_index \
         FROM memory_chunks WHERE parent_id IS NULL \
         ORDER BY COALESCE(accessed_at, created_at) DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&state.db)
    .await;

    let doc_rows = match doc_rows {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    let doc_ids: Vec<uuid::Uuid> = doc_rows.iter().map(|r| r.id).collect();

    // 2. Build chunk→document map in one query (parents + children)
    let chunk_doc_rows: Vec<(uuid::Uuid, Option<uuid::Uuid>)> = sqlx::query_as(
        "SELECT id, parent_id FROM memory_chunks WHERE id = ANY($1) OR parent_id = ANY($1)"
    )
    .bind(&doc_ids)
    .fetch_all(&state.db)
    .await
    .inspect_err(|e| tracing::error!(error = %e, "graph: failed to fetch chunk-document mapping"))
    .unwrap_or_default();

    let all_chunk_ids: Vec<uuid::Uuid> = chunk_doc_rows.iter().map(|(cid, _)| *cid).collect();
    let chunk_to_doc: std::collections::HashMap<uuid::Uuid, uuid::Uuid> =
        chunk_doc_rows.iter().map(|(cid, pid)| (*cid, pid.unwrap_or(*cid))).collect();

    // 3. Get entity data for all chunks
    let entity_rows = crate::memory_graph::get_chunk_entity_rows(&state.db, &all_chunk_ids)
        .await
        .inspect_err(|e| tracing::error!(error = %e, "graph: failed to fetch entity rows"))
        .unwrap_or_default();
    let has_entities = !entity_rows.is_empty();

    let mut nodes: Vec<GraphApiNode> = Vec::new();
    let mut edges: Vec<GraphApiEdge> = Vec::new();

    // Document nodes
    for row in &doc_rows {
        let label = row.source.as_deref().unwrap_or("untitled").to_string();
        nodes.push(GraphApiNode {
            id: format!("doc:{}", row.id),
            kind: "document".to_string(),
            label,
            content: None,
            source: row.source.clone(),
            pinned: Some(row.pinned),
            score: Some(row.relevance_score),
            entity_type: None,
        });
    }

    if has_entities {
        let mut seen_entities: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut doc_entity_edges: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();

        for (chunk_id, entity_name, etype) in &entity_rows {
            if seen_entities.insert(entity_name.clone()) {
                nodes.push(GraphApiNode {
                    id: format!("entity:{}", entity_name),
                    kind: "entity".to_string(),
                    label: entity_name.clone(),
                    content: None, source: None, pinned: None, score: None,
                    entity_type: Some(etype.clone()),
                });
            }
            if let Some(doc_id) = chunk_to_doc.get(chunk_id) {
                let from = format!("doc:{}", doc_id);
                let to = format!("entity:{}", entity_name);
                if doc_entity_edges.insert((from.clone(), to.clone())) {
                    edges.push(GraphApiEdge { from, to, kind: "mentions".to_string() });
                }
            }
        }

        let entity_names: Vec<String> = seen_entities.into_iter().collect();
        match crate::memory_graph::get_entity_edges(&state.db, &entity_names).await {
            Err(e) => tracing::warn!(error = %e, "graph: failed to fetch entity-entity edges"),
            Ok(relations) => {
                for rel in relations {
                    edges.push(GraphApiEdge {
                        from: format!("entity:{}", rel.source),
                        to: format!("entity:{}", rel.target),
                        kind: rel.relation_type,
                    });
                }
            }
        }
    }

    Json(json!({ "nodes": nodes, "edges": edges, "fallback": !has_entities })).into_response()
}
