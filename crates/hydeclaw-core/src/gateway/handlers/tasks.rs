use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::super::AppState;
use crate::tasks;

// ── Tasks API ──

#[derive(Debug, Deserialize)]
pub(crate) struct TasksQuery {
    agent: Option<String>,
    limit: Option<i64>,
}

/// GET /api/tasks?agent=main&limit=50
pub(crate) async fn api_list_tasks(
    State(state): State<AppState>,
    Query(q): Query<TasksQuery>,
) -> impl IntoResponse {
    let agent = q.agent.as_deref().unwrap_or("main");
    let limit = q.limit.unwrap_or(50).min(200);

    match tasks::list_tasks(&state.db, agent, limit).await {
        Ok(rows) => Json(json!({"tasks": rows})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateTaskRequest {
    agent: String,
    input: String,
    #[serde(default = "default_task_source")]
    source: String,
}

fn default_task_source() -> String { "api".to_string() }

/// POST /api/tasks — create a new task and optionally start execution.
pub(crate) async fn api_create_task_endpoint(
    State(state): State<AppState>,
    Json(req): Json<CreateTaskRequest>,
) -> impl IntoResponse {
    if req.input.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "input is required"}))).into_response();
    }

    // Check if agent exists
    if !state.agents.read().await.contains_key(&req.agent) {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("agent '{}' not found", req.agent)}))).into_response();
    }

    match tasks::create_task(&state.db, &req.agent, "api", &req.source, &req.input).await {
        Ok(task_id) => {
            tracing::info!(task_id = %task_id, agent = %req.agent, "task created via API");
            Json(json!({"ok": true, "task_id": task_id.to_string()})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// GET /api/tasks/{id}
pub(crate) async fn api_get_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match tasks::get_task(&state.db, id).await {
        Ok(Some(task)) => Json(json!(task)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "task not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// DELETE /api/tasks/{id}
pub(crate) async fn api_delete_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match tasks::delete_task(&state.db, id).await {
        Ok(true) => Json(json!({"ok": true})).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({"error": "task not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// GET /api/tasks/{id}/steps
pub(crate) async fn api_task_steps(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match tasks::load_task_steps(&state.db, id).await {
        Ok(steps) => {
            let items: Vec<Value> = steps.iter().map(|s| json!({
                "id": s.id.to_string(),
                "step_order": s.step_order,
                "mcp_name": s.mcp_name,
                "action": s.action,
                "params": s.params,
                "status": s.status,
                "output": s.output,
            })).collect();
            Json(json!({"steps": items})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuditQuery {
    agent: Option<String>,
    limit: Option<i64>,
    status: Option<String>,
}

/// GET /api/tasks/audit — tool execution audit trail.
pub(crate) async fn api_task_audit(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).min(200);

    let rows = sqlx::query_as::<_, crate::db::tool_audit::ToolAuditEntry>(
        "SELECT id, agent_id, session_id, tool_name, parameters, status, duration_ms, error, created_at \
         FROM audit_log \
         WHERE ($1::text IS NULL OR agent_id = $1) \
           AND ($2::text IS NULL OR status = $2) \
         ORDER BY created_at DESC \
         LIMIT $3"
    )
    .bind(q.agent.as_deref())
    .bind(q.status.as_deref())
    .bind(limit)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(entries) => Json(json!({"audit": entries})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}
