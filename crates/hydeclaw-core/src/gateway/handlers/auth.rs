use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;
use super::super::AppState;

const TICKET_TTL_SECS: u64 = 30;

/// POST /api/auth/ws-ticket — issue a one-time WebSocket ticket.
/// Requires Bearer token authentication (handled by auth middleware).
/// The ticket is valid for 30 seconds and consumed on first use.
pub(crate) async fn api_create_ws_ticket(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let ticket = uuid::Uuid::new_v4().to_string();
    let mut store = state.ws_tickets.lock().await;
    // Cleanup expired tickets on each call to prevent unbounded growth
    store.retain(|_, created| created.elapsed().as_secs() < TICKET_TTL_SECS);
    store.insert(ticket.clone(), std::time::Instant::now());
    Json(json!({ "ticket": ticket }))
}

/// Validate and consume a one-time WS ticket. Returns true if valid.
pub(crate) async fn validate_ws_ticket(
    tickets: &tokio::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    ticket: &str,
) -> bool {
    let mut map = tickets.lock().await;
    if let Some(created) = map.remove(ticket) {
        created.elapsed().as_secs() < TICKET_TTL_SECS
    } else {
        false
    }
}
