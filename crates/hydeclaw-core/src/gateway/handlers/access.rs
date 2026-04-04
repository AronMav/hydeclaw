use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde_json::{json, Value};

use super::super::AppState;

pub(crate) async fn api_access_pending(
    State(state): State<AppState>,
    axum::extract::Path(agent): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(guard) = state.access_guards.read().await.get(&agent).cloned() else {
        // Agent exists but has no access control configured — return empty list
        return Json(json!({ "pending": [], "mode": "open" })).into_response();
    };
    let pairings = guard.pending_pairings_list().await;
    Json(json!({ "pending": pairings })).into_response()
}

pub(crate) async fn api_access_approve(
    State(state): State<AppState>,
    axum::extract::Path((agent, code)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    let Some(guard) = state.access_guards.read().await.get(&agent).cloned() else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "agent not found"}))).into_response();
    };
    let approver = guard.owner_id.as_deref().unwrap_or("ui");
    let (success, info) = guard.approve_pairing(&code, approver).await;
    if success {
        crate::db::audit::audit_spawn(state.db.clone(), agent.clone(), crate::db::audit::event_types::ACCESS_APPROVED, Some(approver.to_string()), json!({"agent": agent, "user": info}));
        Json(json!({"ok": true, "user": info})).into_response()
    } else {
        (StatusCode::BAD_REQUEST, Json(json!({"error": info}))).into_response()
    }
}

pub(crate) async fn api_access_reject(
    State(state): State<AppState>,
    axum::extract::Path((agent, code)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    let Some(guard) = state.access_guards.read().await.get(&agent).cloned() else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "agent not found"}))).into_response();
    };
    let removed = guard.reject_pairing(&code).await;
    if removed {
        crate::db::audit::audit_spawn(state.db.clone(), agent.clone(), crate::db::audit::event_types::ACCESS_REJECTED, None, json!({"agent": agent, "code": code}));
    }
    Json(json!({"ok": removed})).into_response()
}

pub(crate) async fn api_access_list_users(
    State(state): State<AppState>,
    axum::extract::Path(agent): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(guard) = state.access_guards.read().await.get(&agent).cloned() else {
        // Agent exists but has no access control — return empty list
        return Json(json!({ "users": [], "mode": "open" })).into_response();
    };
    match crate::db::access::list_allowed_users(&guard.db, &agent).await {
        Ok(users) => {
            let list: Vec<Value> = users.iter().map(|u| json!({
                "channel_user_id": u.channel_user_id,
                "display_name": u.display_name,
                "approved_at": u.approved_at.to_rfc3339(),
            })).collect();
            Json(json!({ "users": list })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

pub(crate) async fn api_access_remove_user(
    State(state): State<AppState>,
    axum::extract::Path((agent, user_id)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    let Some(guard) = state.access_guards.read().await.get(&agent).cloned() else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "agent not found"}))).into_response();
    };
    match crate::db::access::remove_allowed_user(&guard.db, &agent, &user_id).await {
        Ok(deleted) => Json(json!({"ok": deleted})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}
