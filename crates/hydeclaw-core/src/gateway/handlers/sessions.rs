use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::db::sessions;
use super::super::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(api_list_sessions).delete(api_delete_all_sessions))
        .route("/api/sessions/latest", get(api_latest_session))
        .route("/api/sessions/search", get(api_search_sessions))
        .route("/api/sessions/stuck", get(api_stuck_sessions))
        .route("/api/sessions/{id}", delete(api_delete_session).patch(api_patch_session))
        .route("/api/sessions/{id}/compact", post(api_compact_session))
        .route("/api/sessions/{id}/export", get(api_export_session))
        .route("/api/sessions/{id}/invite", post(api_invite_to_session))
        .route("/api/sessions/{id}/documents", post(api_session_upload_document))
        .route("/api/sessions/{id}/messages", get(api_session_messages))
        .route("/api/messages/{id}", delete(api_delete_message).patch(api_patch_message))
        .route("/api/messages/{id}/feedback", post(api_message_feedback))
        .route("/api/sessions/{id}/fork", post(api_fork_session))
        .route("/api/sessions/{id}/active-path", get(api_active_path))
        .route("/api/sessions/{id}/retry", post(api_retry_session))
}

// ── Latest Session endpoint ──

pub(crate) async fn api_latest_session(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let agent = params.get("agent").map_or("", std::string::String::as_str);
    if agent.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "agent parameter required"}))).into_response();
    }

    let session = match sessions::get_latest_ui_session(&state.db, agent).await {
        Ok(Some(s)) => s,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    };

    let messages = match sessions::load_messages(&state.db, session.id, Some(100)).await {
        Ok(m) => m,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    };

    let msg_json: Vec<Value> = messages
        .iter()
        .map(|m| {
            json!({
                "id": m.id.to_string(),
                "role": m.role,
                "content": m.content,
                "tool_calls": m.tool_calls,
                "tool_call_id": m.tool_call_id,
                "created_at": m.created_at.to_rfc3339(),
                "feedback": m.feedback.unwrap_or(0),
                "edited_at": m.edited_at.map(|t| t.to_rfc3339()),
                "status": m.status,
            })
        })
        .collect();

    Json(json!({
        "session": {
            "id": session.id.to_string(),
            "agent_id": session.agent_id,
            "channel": session.channel,
            "started_at": session.started_at.to_rfc3339(),
            "last_message_at": session.last_message_at.to_rfc3339(),
            "title": session.title,
            "metadata": session.metadata,
            "run_status": session.run_status,
            "participants": session.participants,
        },
        "messages": msg_json,
    }))
    .into_response()
}

// ── Sessions & Messages API ──

#[derive(Debug, Deserialize)]
pub(crate) struct SessionsQuery {
    pub(crate) agent: Option<String>,
    pub(crate) channel: Option<String>,
    pub(crate) limit: Option<i64>,
}

pub(crate) async fn api_list_sessions(
    State(state): State<AppState>,
    Query(q): Query<SessionsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(20).min(100);

    let agent = match q.agent.as_deref() {
        Some(a) if !a.is_empty() => a,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "agent parameter required"})),
            ).into_response();
        }
    };

    let query = match q.channel.as_deref() {
        Some(channel) => {
            let channels: Vec<&str> = channel.split(',').collect();
            sqlx::query_as::<_, sessions::Session>(
                "SELECT id, agent_id, user_id, channel, started_at, last_message_at, title, metadata, run_status, activity_at, participants \
                 FROM sessions WHERE (agent_id = $1 OR $1 = ANY(participants)) AND channel = ANY($2) \
                 ORDER BY last_message_at DESC LIMIT $3",
            )
            .bind(agent)
            .bind(&channels)
            .bind(limit)
            .fetch_all(&state.db)
            .await
        }
        None => {
            sqlx::query_as::<_, sessions::Session>(
                "SELECT id, agent_id, user_id, channel, started_at, last_message_at, title, metadata, run_status, activity_at, participants \
                 FROM sessions WHERE agent_id = $1 OR $1 = ANY(participants) \
                 ORDER BY last_message_at DESC LIMIT $2",
            )
            .bind(agent)
            .bind(limit)
            .fetch_all(&state.db)
            .await
        }
    };

    match query {
        Ok(rows) => {
            let sessions: Vec<Value> = rows
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "agent_id": s.agent_id,
                        "user_id": s.user_id,
                        "channel": s.channel,
                        "started_at": s.started_at.to_rfc3339(),
                        "last_message_at": s.last_message_at.to_rfc3339(),
                        "title": s.title,
                        "metadata": s.metadata,
                        "run_status": s.run_status,
                        "participants": s.participants,
                    })
                })
                .collect();
            Json(json!({ "sessions": sessions })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessagesQuery {
    limit: Option<i64>,
    agent: Option<String>,
}

pub(crate) async fn api_session_messages(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Query(q): Query<MessagesQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).min(200);

    if let Some(ref agent) = q.agent
        && let Err(resp) = verify_session_agent(&state.db, id, agent).await {
            return resp;
        }

    match sessions::load_messages(&state.db, id, Some(limit)).await {
        Ok(rows) => {
            let messages: Vec<Value> = rows
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "role": m.role,
                        "content": m.content,
                        "tool_calls": m.tool_calls,
                        "tool_call_id": m.tool_call_id,
                        "created_at": m.created_at.to_rfc3339(),
                        "agent_id": m.agent_id,
                        "feedback": m.feedback.unwrap_or(0),
                        "edited_at": m.edited_at.map(|t| t.to_rfc3339()),
                        "status": m.status,
                        "parent_message_id": m.parent_message_id,
                        "branch_from_message_id": m.branch_from_message_id,
                    })
                })
                .collect();
            Json(json!({ "messages": messages })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// DELETE /api/messages/{id}
/// Deletes a single message by UUID.
pub(crate) async fn api_delete_message(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> impl IntoResponse {
    let result = sqlx::query("DELETE FROM messages WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"ok": true})).into_response(),
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({"error": "message not found"}))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// DELETE /api/sessions/{id}
/// Deletes a session and all its messages. Requires agent param for ownership check.
pub(crate) async fn api_delete_session(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Query(q): Query<SessionsQuery>,
) -> impl IntoResponse {
    let agent = match q.agent.as_deref() {
        Some(a) if !a.is_empty() => a,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "agent parameter required for session deletion"})),
            ).into_response();
        }
    };

    let _ = sqlx::query("DELETE FROM messages WHERE session_id = $1 AND session_id IN (SELECT id FROM sessions WHERE agent_id = $2)")
        .bind(id)
        .bind(agent)
        .execute(&state.db)
        .await;

    let result = sqlx::query("DELETE FROM sessions WHERE id = $1 AND agent_id = $2")
        .bind(id)
        .bind(agent)
        .execute(&state.db)
        .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "session not found or does not belong to agent"}))).into_response()
        }
        Ok(_) => {
            tracing::info!(session_id = %id, agent = %agent, "session deleted via API");
            // Kill any live agents in the session pool
            let mut pools = state.session_pools.write().await;
            if let Some(mut pool) = pools.remove(&id)
                && !pool.is_empty() {
                    tracing::info!(session_id = %id, count = pool.len(), "killing session agent pool on delete");
                    pool.kill_all();
                }
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ).into_response(),
    }
}

async fn verify_session_agent(db: &sqlx::PgPool, session_id: uuid::Uuid, expected_agent: &str) -> Result<(), axum::response::Response> {
    let row = sqlx::query_scalar::<_, String>(
        "SELECT agent_id FROM sessions WHERE id = $1"
    )
    .bind(session_id)
    .fetch_optional(db)
    .await;

    match row {
        Ok(Some(agent_id)) if agent_id == expected_agent => Ok(()),
        Ok(Some(_)) => Err((StatusCode::FORBIDDEN, Json(json!({"error": "session belongs to a different agent"}))).into_response()),
        Ok(None) => Err((StatusCode::NOT_FOUND, Json(json!({"error": "session not found"}))).into_response()),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response()),
    }
}

/// DELETE /api/sessions?agent=xxx or DELETE /api/sessions?channel=discuss,group
/// Deletes all sessions (and their messages) for a specific agent or channel(s).
pub(crate) async fn api_delete_all_sessions(
    State(state): State<AppState>,
    Query(q): Query<SessionsQuery>,
) -> impl IntoResponse {
    // Support either agent or channel filter
    let result = if let Some(ref channel) = q.channel {
        let channels: Vec<&str> = channel.split(',').collect();
        let _ = sqlx::query(
            "DELETE FROM messages WHERE session_id IN (SELECT id FROM sessions WHERE channel = ANY($1))",
        )
        .bind(&channels)
        .execute(&state.db)
        .await;
        sqlx::query("DELETE FROM sessions WHERE channel = ANY($1)")
            .bind(&channels)
            .execute(&state.db)
            .await
    } else if let Some(ref agent) = q.agent {
        let _ = sqlx::query(
            "DELETE FROM messages WHERE session_id IN (SELECT id FROM sessions WHERE agent_id = $1)",
        )
        .bind(agent)
        .execute(&state.db)
        .await;
        sqlx::query("DELETE FROM sessions WHERE agent_id = $1")
            .bind(agent)
            .execute(&state.db)
            .await
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "agent or channel parameter required"})),
        )
            .into_response();
    };

    match result {
        Ok(r) => {
            let filter = q.agent.as_deref().or(q.channel.as_deref()).unwrap_or("?");
            tracing::info!(filter = %filter, deleted = r.rows_affected(), "sessions deleted via API");
            Json(json!({"ok": true, "deleted": r.rows_affected()})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ).into_response(),
    }
}

/// GET /api/sessions/search?q=...&agent=...&limit=50
/// Full-text search across conversation history (messages).
pub(crate) async fn api_search_sessions(
    State(state): State<AppState>,
    Query(q): Query<SessionSearchQuery>,
) -> impl IntoResponse {
    let query_str = q.q.as_deref().unwrap_or("").trim();
    if query_str.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "q parameter required"})),
        ).into_response();
    }
    let agent = q.agent.as_deref().unwrap_or("main");
    let limit = q.limit.unwrap_or(50).min(200);

    match sessions::search_messages(&state.db, agent, query_str, limit).await {
        Ok(results) => {
            let items: Vec<Value> = results.iter().map(|r| json!({
                "content": r.content,
                "session_id": r.session_id.to_string(),
                "user_id": r.user_id,
                "channel": r.channel,
                "role": r.role,
                "created_at": r.created_at.to_rfc3339(),
                "rank": r.rank,
            })).collect();
            Json(json!({"results": items, "count": items.len()})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SessionSearchQuery {
    q: Option<String>,
    agent: Option<String>,
    limit: Option<i64>,
}

// ── Session Invite ──

#[derive(Debug, Deserialize)]
pub(crate) struct InviteRequest {
    pub agent_name: String,
}

/// POST /api/sessions/{id}/invite — invite an agent into a multi-agent session.
pub(crate) async fn api_invite_to_session(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(req): Json<InviteRequest>,
) -> impl IntoResponse {
    // Validate agent exists
    let agent_exists = {
        let map = state.agents.read().await;
        map.contains_key(&req.agent_name)
    };
    if !agent_exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("agent '{}' not found", req.agent_name)})),
        ).into_response();
    }

    match crate::db::sessions::add_participant(&state.db, id, &req.agent_name).await {
        Ok(participants) => {
            // Broadcast to WebSocket for live UI updates
            let event = serde_json::json!({
                "type": "agent_joined",
                "agent_name": req.agent_name,
                "session_id": id.to_string(),
                "invited_by": "user",
                "participants": participants,
            });
            state.ui_event_tx.send(event.to_string()).ok();

            Json(json!({ "participants": participants })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ).into_response(),
    }
}

// ── Session Compaction ──

/// POST /api/sessions/{id}/compact — manually compact a session's history.
pub(crate) async fn api_compact_session(
    State(state): State<AppState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    // Find which agent owns this session
    let session = match sessions::get_session(&state.db, id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Json(json!({"error": "session not found"}))).into_response()
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response()
        }
    };

    let agents = state.agents.read().await;
    let engine = match agents.get(&session.agent_id) {
        Some(handle) => handle.engine.clone(),
        None => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "agent not running"}))).into_response()
        }
    };
    drop(agents);

    match engine.compact_session(id).await {
        Ok((facts, new_count)) => Json(json!({
            "ok": true,
            "facts_extracted": facts,
            "new_message_count": new_count,
        }))
        .into_response(),
        Err(e) => {
            tracing::error!(session_id = %id, error = %e, "session compaction failed");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response()
        }
    }
}

// ── Session Patch (rename) ──

/// POST /api/messages/{id}/feedback — set feedback (1=like, -1=dislike, 0=clear)
pub(crate) async fn api_message_feedback(
    State(state): State<AppState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<FeedbackRequest>,
) -> impl IntoResponse {
    let feedback = body.feedback.clamp(-1, 1);
    let result = sqlx::query("UPDATE messages SET feedback = $1 WHERE id = $2")
        .bind(feedback as i16)
        .bind(id)
        .execute(&state.db)
        .await;
    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"ok": true})).into_response(),
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({"error": "message not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct FeedbackRequest {
    feedback: i32, // 1 = like, -1 = dislike, 0 = clear
}

/// PATCH /api/messages/{id} — edit message content
pub(crate) async fn api_patch_message(
    State(state): State<AppState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<PatchMessageRequest>,
) -> impl IntoResponse {
    let result = sqlx::query(
        "UPDATE messages SET content = $1, edited_at = now() WHERE id = $2 AND role = 'user'"
    )
        .bind(&body.content)
        .bind(id)
        .execute(&state.db)
        .await;
    match result {
        Ok(r) if r.rows_affected() > 0 => Json(json!({"ok": true})).into_response(),
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({"error": "message not found or not a user message"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct PatchMessageRequest {
    content: String,
}

// ── Fork (branch) endpoint ──────────────────────────────

#[derive(Deserialize)]
pub(crate) struct ForkRequest {
    branch_from_message_id: uuid::Uuid, // the user message being replaced
    content: String,                     // new user message text
}

/// POST /api/sessions/{id}/fork — create a branched user message from an existing message.
pub(crate) async fn api_fork_session(
    State(state): State<AppState>,
    Path(session_id): Path<uuid::Uuid>,
    Json(body): Json<ForkRequest>,
) -> impl IntoResponse {
    // 1. Find the parent of the branch_from message (the message BEFORE it)
    let parent_id = match sessions::find_parent_of_message(
        &state.db,
        session_id,
        body.branch_from_message_id,
    )
    .await
    {
        Ok(pid) => pid,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };

    // 2. Save the new user message with branch pointers
    match sessions::save_message_branched(
        &state.db,
        session_id,
        "user",
        &body.content,
        None,
        None,
        None,
        None,
        parent_id,
        Some(body.branch_from_message_id),
    )
    .await
    {
        Ok(new_msg_id) => Json(json!({
            "message_id": new_msg_id,
            "parent_message_id": parent_id,
            "branch_from_message_id": body.branch_from_message_id,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// PATCH /api/sessions/{id} — update session metadata (title).
pub(crate) async fn api_patch_session(
    State(state): State<AppState>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if let Some(raw_title) = body.get("title").and_then(|v| v.as_str()) {
        let title: String = raw_title.chars().take(200).collect();
        match sqlx::query("UPDATE sessions SET title = $1 WHERE id = $2")
            .bind(title)
            .bind(id)
            .execute(&state.db)
            .await
        {
            Ok(r) if r.rows_affected() == 0 => {
                return (StatusCode::NOT_FOUND, Json(json!({"error": "session not found"}))).into_response();
            }
            Ok(_) => {}
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
            }
        }
    }
    // Persist UI state inside metadata JSONB (merge, don't overwrite)
    if let Some(ui_state) = body.get("ui_state") {
        // Validate: must be an object, max 1KB serialized
        let serialized = ui_state.to_string();
        if !ui_state.is_object() || serialized.len() > 1024 {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "ui_state must be a JSON object under 1KB"}))).into_response();
        }
        match sqlx::query(
            "UPDATE sessions SET metadata = COALESCE(metadata, '{}'::jsonb) || jsonb_build_object('ui_state', $1::jsonb) WHERE id = $2"
        )
        .bind(ui_state)
        .bind(id)
        .execute(&state.db)
        .await
        {
            Ok(r) if r.rows_affected() == 0 => {
                return (StatusCode::NOT_FOUND, Json(json!({"error": "session not found"}))).into_response();
            }
            Ok(_) => {}
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
            }
        }
    }

    Json(json!({"ok": true})).into_response()
}

/// GET /api/sessions/{id}/export — export full session as JSON (metadata + all messages).
pub(crate) async fn api_export_session(
    State(state): State<AppState>,
    Path(id): Path<uuid::Uuid>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if let Some(agent) = params.get("agent")
        && let Err(resp) = verify_session_agent(&state.db, id, agent).await {
            return resp;
        }

    let format = params.get("format").map_or("json", std::string::String::as_str);
    match format {
        "markdown" | "md" => {
            match crate::db::sessions::export_session(&state.db, id).await {
                Ok(Some(data)) => {
                    let md = format_session_as_markdown(&data);
                    let disposition = format!("attachment; filename=\"session-{id}.md\"");
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
                         (axum::http::header::CONTENT_DISPOSITION, disposition.as_str())],
                        md,
                    ).into_response()
                }
                Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "session not found"}))).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
            }
        }
        _ => {
            match crate::db::sessions::export_session(&state.db, id).await {
                Ok(Some(data)) => Json(data).into_response(),
                Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "session not found"}))).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
            }
        }
    }
}

fn format_session_as_markdown(data: &serde_json::Value) -> String {
    let mut md = String::new();
    let session = &data["session"];
    let title = session["title"].as_str().unwrap_or("Untitled");
    let agent = session["agent_id"].as_str().unwrap_or("unknown");
    let started = session["started_at"].as_str().unwrap_or("");

    md.push_str(&format!("# {title}\n\n"));
    md.push_str(&format!("**Agent:** {agent} | **Started:** {started}\n\n---\n\n"));

    if let Some(messages) = data["messages"].as_array() {
        for msg in messages {
            let role = msg["role"].as_str().unwrap_or("unknown");
            let content = msg["content"].as_str().unwrap_or("");
            let ts = msg["created_at"].as_str().unwrap_or("");
            let ts_short = if ts.len() >= 16 { &ts[..16] } else { ts };

            let role_label = match role {
                "user" => "User",
                "assistant" => "Assistant",
                "system" => "System",
                "tool" => "Tool Result",
                _ => role,
            };

            md.push_str(&format!("## {role_label} ({ts_short})\n\n"));

            if let Some(tool_calls) = msg["tool_calls"].as_array() {
                for tc in tool_calls {
                    let name = tc["name"].as_str().unwrap_or("unknown");
                    let args = tc["arguments"].to_string();
                    md.push_str(&format!("### Tool: {name}\n```json\n{args}\n```\n\n"));
                }
            }

            if !content.is_empty() {
                md.push_str(content);
                md.push_str("\n\n");
            }
        }
    }
    md
}

// ── Session Document Upload (per-session RAG) ──────────────────────────────

/// Upload a text document and chunk+embed it for session-scoped RAG.
pub(crate) async fn api_session_upload_document(
    State(state): State<AppState>,
    Path(session_id): Path<uuid::Uuid>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    // Read file from multipart
    let (filename, content) = match multipart.next_field().await {
        Ok(Some(field)) => {
            let name = field.file_name().unwrap_or("document.txt").to_string();
            match field.text().await {
                Ok(text) => (name, text),
                Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("failed to read file: {}", e)}))).into_response(),
            }
        }
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": "no file uploaded"}))).into_response(),
    };

    if content.len() > 10 * 1024 * 1024 {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error": "document too large (max 10MB)"}))).into_response();
    }

    if content.len() < 50 {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "file too small (min 50 chars)"}))).into_response();
    }

    // Check memory store availability
    let memory_store = &state.memory_store;
    if !memory_store.is_available() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "embeddings not configured"}))).into_response();
    }

    // Chunk the document (~500 chars per chunk, split on paragraphs)
    let chunks = chunk_text(&content, 500);
    let mut indexed = 0u32;

    for (i, chunk) in chunks.iter().enumerate() {
        let embedding = match memory_store.embed(chunk).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(chunk = i, error = %e, "failed to embed document chunk");
                continue;
            }
        };
        let vec_str = format!("[{}]", embedding.iter().map(std::string::ToString::to_string).collect::<Vec<_>>().join(","));
        if let Err(e) = crate::db::session_documents::insert_chunk(
            &state.db, session_id, &filename, chunk, i as i32, &vec_str,
        ).await {
            tracing::warn!(chunk = i, error = %e, "failed to insert document chunk");
            continue;
        }
        indexed += 1;
    }

    (StatusCode::OK, Json(json!({
        "filename": filename,
        "chunks": indexed,
        "total_chars": content.len(),
    }))).into_response()
}

/// Split text into chunks of approximately `target_size` characters, preferring paragraph breaks.
fn chunk_text(text: &str, target_size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for paragraph in text.split("\n\n") {
        if current.len() + paragraph.len() > target_size && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current = String::new();
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }
    chunks
}

// ── Branching endpoints ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct ActivePathQuery {
    leaf: Option<uuid::Uuid>,
}

// ── Session Auto-Retry ──────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub(crate) struct StuckSessionsQuery {
    stale_secs: Option<i64>,
    max_retries: Option<i32>,
}

/// GET /api/sessions/stuck — find sessions needing retry
pub(crate) async fn api_stuck_sessions(
    State(state): State<AppState>,
    Query(q): Query<StuckSessionsQuery>,
) -> impl IntoResponse {
    let stale_secs = q.stale_secs.unwrap_or(90);
    let max_retries = q.max_retries.unwrap_or(3);

    match sessions::find_stuck_sessions(&state.db, stale_secs, max_retries).await {
        Ok(rows) => {
            let sessions: Vec<serde_json::Value> = rows.iter().map(|(id, agent)| {
                serde_json::json!({"id": id, "agent_id": agent})
            }).collect();
            Json(serde_json::json!({"sessions": sessions})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

/// POST /api/sessions/{id}/retry — replay last user message through engine
pub(crate) async fn api_retry_session(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> impl IntoResponse {
    // 1. Load session
    let session: crate::db::sessions::Session = match sqlx::query_as(
        "SELECT * FROM sessions WHERE id = $1"
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "session not found"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    };

    // 2. Get last user message
    let user_text = match sessions::get_last_user_message(&state.db, id).await {
        Ok(Some(text)) => text,
        Ok(None) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "no user message in session"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    };

    // 3. Cleanup: delete empty assistant messages and the last user message
    //    (handle_sse will re-save it, so we remove to avoid duplicates)
    if let Ok(deleted) = sessions::delete_empty_assistant_messages(&state.db, id).await {
        if deleted > 0 {
            tracing::info!(session_id = %id, deleted, "cleaned up empty assistant messages before retry");
        }
    }
    // Delete the last user message — handle_sse will re-insert it
    let _ = sqlx::query(
        "DELETE FROM messages WHERE id = (\
         SELECT id FROM messages WHERE session_id = $1 AND role = 'user' \
         ORDER BY created_at DESC LIMIT 1)"
    )
    .bind(id)
    .execute(&state.db)
    .await;

    // 4. Increment retry count (atomic guard against concurrent double-retry)
    let retry_count = match sessions::increment_retry_count(&state.db, id).await {
        Ok(Some(c)) => c,
        Ok(None) => return (StatusCode::CONFLICT, Json(serde_json::json!({"error": "session not in running state (concurrent retry?)"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    };

    tracing::info!(session_id = %id, agent = %session.agent_id, retry_count, "retrying stuck session");

    // 5. Get engine
    let engine = match state.get_engine(&session.agent_id).await {
        Some(e) => e,
        None => {
            let _ = sessions::mark_session_failed(&state.db, id).await;
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": format!("agent '{}' not found", session.agent_id)}))).into_response();
        }
    };

    // 6. Build message and run via handle_sse with resume_session_id
    let msg = hydeclaw_types::IncomingMessage {
        text: Some(user_text),
        user_id: session.user_id.clone(),
        channel: session.channel.clone(),
        agent_id: session.agent_id.clone(),
        context: Default::default(),
        attachments: vec![],
        leaf_message_id: None,
        tool_policy_override: None,
        timestamp: chrono::Utc::now(),
        formatting_prompt: None,
    };

    // Spawn background task
    let db = state.db.clone();
    let session_id = id;
    tokio::spawn(async move {
        // Bounded drain channel — prevents unbounded memory growth during retry
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move { while event_rx.recv().await.is_some() {} });

        match engine.handle_sse(&msg, event_tx, Some(session_id), false).await {
            Ok(_msg_id) => {
                tracing::info!(session_id = %session_id, "retry succeeded");
            }
            Err(e) => {
                tracing::error!(session_id = %session_id, error = %e, "retry failed");
                sessions::mark_session_failed(&db, session_id).await.ok();
            }
        }
    });

    Json(serde_json::json!({"ok": true, "retry_count": retry_count, "session_id": id})).into_response()
}

/// GET /api/sessions/{id}/active-path -- resolve the linear message chain for display.
pub(crate) async fn api_active_path(
    State(state): State<AppState>,
    Path(session_id): Path<uuid::Uuid>,
    Query(q): Query<ActivePathQuery>,
) -> impl IntoResponse {
    match sessions::resolve_active_path(&state.db, session_id, q.leaf).await {
        Ok(msgs) => {
            let messages: Vec<Value> = msgs.iter().map(|m| {
                json!({
                    "id": m.id,
                    "role": m.role,
                    "content": m.content,
                    "tool_calls": m.tool_calls,
                    "tool_call_id": m.tool_call_id,
                    "created_at": m.created_at.to_rfc3339(),
                    "agent_id": m.agent_id,
                    "feedback": m.feedback.unwrap_or(0),
                    "edited_at": m.edited_at.map(|t| t.to_rfc3339()),
                    "status": m.status,
                    "parent_message_id": m.parent_message_id,
                    "branch_from_message_id": m.branch_from_message_id,
                })
            }).collect();
            Json(json!({ "messages": messages })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

