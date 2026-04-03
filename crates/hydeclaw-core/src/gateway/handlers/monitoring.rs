use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::AppState;

pub(crate) async fn api_setup_status(State(state): State<AppState>) -> Json<Value> {
    let agents = state.agents.read().await;
    let needs_setup = agents.is_empty();
    Json(json!({ "needs_setup": needs_setup }))
}

pub(crate) async fn api_status(State(state): State<AppState>) -> Json<Value> {
    let db_ok = sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .is_ok();

    let uptime_secs = state.started_at.elapsed().as_secs();

    let memory_chunks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memory_chunks")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let scheduled_jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scheduled_jobs WHERE enabled = true")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let active_sessions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions WHERE last_message_at > now() - interval '4 hours'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let config = state.shared_config.read().await;

    Json(json!({
        "status": if db_ok { "ok" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": uptime_secs,
        "db": db_ok,
        "listen": config.gateway.listen,
        "agents": state.agent_names().await,
        "memory_chunks": memory_chunks,
        "scheduled_jobs": scheduled_jobs,
        "active_sessions": active_sessions,
        "tools_registered": state.tools.len().await + {
            // Count YAML tool files without parsing them (avoid filesystem overhead per request)
            let yaml_count = match tokio::fs::read_dir("workspace/tools").await {
                Ok(mut dir) => {
                    let mut count = 0u64;
                    while let Ok(Some(entry)) = dir.next_entry().await {
                        if entry.path().extension().is_some_and(|e| e == "yaml" || e == "yml") {
                            count += 1;
                        }
                    }
                    count
                }
                Err(_) => 0,
            };
            yaml_count as usize
        },
    }))
}

pub(crate) async fn api_stats(State(state): State<AppState>) -> Json<Value> {
    let messages_today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE created_at > CURRENT_DATE",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let sessions_today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions WHERE started_at > CURRENT_DATE",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let total_messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let total_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    #[allow(clippy::type_complexity)]
    let recent_sessions: Vec<(uuid::Uuid, String, String, chrono::DateTime<chrono::Utc>, Option<String>)> =
        sqlx::query_as(
            "SELECT id, agent_id, channel, last_message_at, title \
             FROM sessions \
             WHERE last_message_at > NOW() - INTERVAL '24 hours' \
             ORDER BY last_message_at DESC LIMIT 10",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let recent: Vec<Value> = recent_sessions.iter().map(|(id, agent, channel, ts, title)| {
        json!({ "id": id, "agent_id": agent, "channel": channel, "last_message_at": ts, "title": title })
    }).collect();

    Json(json!({
        "messages_today": messages_today,
        "sessions_today": sessions_today,
        "total_messages": total_messages,
        "total_sessions": total_sessions,
        "recent_sessions": recent,
    }))
}

// ── Usage API ──

#[derive(Debug, Deserialize)]
pub(crate) struct UsageQuery {
    days: Option<u32>,
    agent: Option<String>,
}

pub(crate) async fn api_usage(
    State(state): State<AppState>,
    Query(q): Query<UsageQuery>,
) -> Json<Value> {
    let days = q.days.unwrap_or(30);
    match crate::db::usage::usage_summary(&state.db, days).await {
        Ok(summary) => Json(json!({"ok": true, "days": days, "usage": summary})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

pub(crate) async fn api_usage_daily(
    State(state): State<AppState>,
    Query(q): Query<UsageQuery>,
) -> Json<Value> {
    let days = q.days.unwrap_or(30);
    match crate::db::usage::usage_daily(&state.db, days).await {
        Ok(daily) => Json(json!({"ok": true, "days": days, "daily": daily})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

pub(crate) async fn api_usage_sessions(
    State(state): State<AppState>,
    Query(q): Query<UsageQuery>,
) -> Json<Value> {
    let days = q.days.unwrap_or(30);
    match crate::db::usage::usage_by_session(&state.db, q.agent.as_deref(), days).await {
        Ok(sessions) => Json(json!({"ok": true, "days": days, "sessions": sessions})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

// ── Audit API ──

#[derive(Deserialize)]
pub(crate) struct AuditQuery {
    agent: Option<String>,
    event_type: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

pub(crate) async fn api_audit_events(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> Json<Value> {
    let limit = q.limit.unwrap_or(100).min(500);
    let offset = q.offset.unwrap_or(0);
    match crate::db::audit::query_events(
        &state.db,
        q.agent.as_deref(),
        q.event_type.as_deref(),
        limit,
        offset,
    ).await {
        Ok(events) => Json(json!({"ok": true, "events": events, "limit": limit, "offset": offset})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

// ── Tool Audit Log API ──

#[derive(Deserialize)]
pub(crate) struct ToolAuditQuery {
    agent: Option<String>,
    tool: Option<String>,
    days: Option<u32>,
    limit: Option<i64>,
}

pub(crate) async fn api_tool_audit(
    State(state): State<AppState>,
    Query(q): Query<ToolAuditQuery>,
) -> Json<Value> {
    let days = q.days.unwrap_or(7);
    let limit = q.limit.unwrap_or(100).min(500);
    match crate::db::tool_audit::query_tool_audit(
        &state.db,
        q.agent.as_deref(),
        q.tool.as_deref(),
        days,
        limit,
    ).await {
        Ok(entries) => Json(json!({"ok": true, "entries": entries, "days": days, "limit": limit})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

// ── Doctor / Health-check API ──

pub(crate) async fn api_doctor(State(state): State<AppState>) -> Json<Value> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let config = state.shared_config.read().await;

    // 1. Database
    let db_start = std::time::Instant::now();
    let db_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();
    let db_ms = db_start.elapsed().as_millis() as u64;

    // 2–4. Toolgate, Browser renderer, SearXNG — run in parallel
    let toolgate_url = config.toolgate_url.clone()
        .unwrap_or_else(|| "http://localhost:9011".to_string());
    let br_base = std::env::var("BROWSER_RENDERER_URL")
        .unwrap_or_else(|_| "http://localhost:9020".to_string());

    let tg_start = std::time::Instant::now();
    let br_start = tg_start;
    let sx_start = tg_start;

    let (tg_result, br_result, sx_result) = tokio::join!(
        http.get(format!("{}/health", toolgate_url)).send(),
        http.get(format!("{br_base}/health")).send(),
        http.get("http://localhost:8080/healthz").send(),
    );

    let tg_ms = tg_start.elapsed().as_millis() as u64;
    let br_ms = br_start.elapsed().as_millis() as u64;
    let sx_ms = sx_start.elapsed().as_millis() as u64;

    let (tg_ok, tg_providers) = match tg_result {
        Ok(r) if r.status().is_success() => {
            let body: Value = r.json().await.unwrap_or(Value::Null);
            let providers = body.get("active_providers").cloned().unwrap_or(Value::Null);
            (true, providers)
        }
        _ => (false, Value::Null),
    };

    let br_ok = br_result.map(|r| r.status().is_success()).unwrap_or(false);
    let sx_ok = sx_result.map(|r| r.status().is_success()).unwrap_or(false);

    drop(config);

    // 6. Critical secrets — dynamic: check LLM providers have credentials
    let mut missing_critical: Vec<String> = Vec::new();
    if let Ok(providers) = crate::db::providers::list_providers_by_type(&state.db, "text").await {
        for p in &providers {
            let has_key = state.secrets.get_scoped(
                crate::agent::providers::PROVIDER_CREDENTIALS,
                &p.id.to_string(),
            ).await.is_some();
            if !has_key {
                missing_critical.push(format!("LLM:{}", p.name));
            }
        }
    }
    // Check active channels have credentials
    if let Ok(channels) = sqlx::query_as::<_, (sqlx::types::Uuid, String, String)>(
        "SELECT id, agent_name, channel_type FROM agent_channels WHERE status != 'deleted'"
    ).fetch_all(&state.db).await {
        for (id, agent, ch_type) in &channels {
            if state.secrets.get_scoped("CHANNEL_CREDENTIALS", &id.to_string()).await.is_none() {
                missing_critical.push(format!("Channel:{}:{}", agent, ch_type));
            }
        }
    }
    let secrets_count = state.secrets.list().await.map(|v| v.len()).unwrap_or(0);

    // 7. Channels container (generic Docker health check)
    let ch_start = std::time::Instant::now();
    let ch_ok = http.get("http://localhost:3100/health")
        .timeout(std::time::Duration::from_secs(3))
        .send().await
        .map(|r| r.status().is_success()).unwrap_or(false);
    let ch_ms = ch_start.elapsed().as_millis() as u64;

    // 8. Tool health (degraded tools from quality tracking)
    let degraded_tools = crate::db::tool_quality::get_degraded_tools(&state.db)
        .await.unwrap_or_default();
    let degraded_count = degraded_tools.len();

    // 9. Agent statuses
    let agents_map = state.agents.read().await;
    let mut agents_status = serde_json::Map::new();
    for (name, _handle) in agents_map.iter() {
        agents_status.insert(name.clone(), json!({
            "ok": true,
        }));
    }
    drop(agents_map);

    let all_ok = db_ok && tg_ok;

    let polling = state.polling_diagnostics.snapshot();

    Json(json!({
        "ok": all_ok,
        "checks": {
            "database": { "ok": db_ok, "latency_ms": db_ms },
            "toolgate": { "ok": tg_ok, "latency_ms": tg_ms, "providers": tg_providers },
            "browser_renderer": { "ok": br_ok, "latency_ms": br_ms },
            "searxng": { "ok": sx_ok, "latency_ms": sx_ms },
            "secrets": {
                "ok": missing_critical.is_empty(),
                "count": secrets_count,
                "missing_critical": missing_critical,
            },
            "channels": { "ok": ch_ok, "latency_ms": ch_ms },
            "agents": agents_status,
            "tool_health": {
                "ok": degraded_count == 0,
                "degraded": degraded_tools,
                "degraded_count": degraded_count,
            },
            "polling": polling,
        }
    }))
}

// ── Watchdog API ──

/// GET /api/watchdog/status
pub(crate) async fn api_watchdog_status() -> impl IntoResponse {
    match tokio::fs::read_to_string("/tmp/hydeclaw-watchdog.json").await {
        Ok(json) => match serde_json::from_str::<serde_json::Value>(&json) {
            Ok(v) => Json(v).into_response(),
            Err(_) => Json(json!({"error": "invalid status file"})).into_response(),
        },
        Err(_) => Json(json!({"error": "watchdog not running"})).into_response(),
    }
}

/// GET /api/watchdog/config
pub(crate) async fn api_watchdog_config() -> impl IntoResponse {
    match tokio::fs::read_to_string("config/watchdog.toml").await {
        Ok(text) => Json(json!({"config": text})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// POST /api/watchdog/restart/{name} — execute restart_cmd for a watchdog check
pub(crate) async fn api_watchdog_restart_check(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let config_text = match tokio::fs::read_to_string("config/watchdog.toml").await {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };
    let config: toml::Value = match toml::from_str(&config_text) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };
    let checks = config.get("checks").and_then(|v| v.as_array());
    let restart_cmd = checks.and_then(|arr| {
        arr.iter().find(|c| c.get("name").and_then(|n| n.as_str()) == Some(&name))
            .and_then(|c| c.get("restart_cmd").and_then(|r| r.as_str()))
    });
    let Some(cmd) = restart_cmd else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no restart_cmd for check '{}'", name)}))).into_response();
    };
    tracing::info!(check = %name, cmd, "watchdog restart requested via API");
    let output = tokio::process::Command::new("bash").args(["-c", cmd]).output().await;
    match output {
        Ok(o) if o.status.success() => Json(json!({"ok": true, "check": name})).into_response(),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": err.to_string()}))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": e.to_string()}))).into_response(),
    }
}

/// GET /api/watchdog/settings — read alerting settings from DB
pub(crate) async fn api_watchdog_settings(
    State(state): State<AppState>,
) -> Json<Value> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT key, value FROM watchdog_settings",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut settings = serde_json::Map::new();
    for (key, value) in rows {
        settings.insert(key, value);
    }
    Json(Value::Object(settings))
}

/// PUT /api/watchdog/settings — update alerting settings
pub(crate) async fn api_watchdog_settings_update(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(obj) = body.as_object() else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "expected JSON object"}))).into_response();
    };

    let allowed = ["alert_channel_ids", "alert_events"];
    for (key, value) in obj {
        if !allowed.contains(&key.as_str()) {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("unknown key: {}", key)}))).into_response();
        }
        if let Err(e) = sqlx::query(
            "INSERT INTO watchdog_settings (key, value, updated_at) VALUES ($1, $2, now())
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
        )
        .bind(key)
        .bind(value)
        .execute(&state.db)
        .await
        {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response();
        }
    }

    Json(json!({"ok": true})).into_response()
}

/// PUT /api/watchdog/config
pub(crate) async fn api_watchdog_config_update(Json(req): Json<serde_json::Value>) -> impl IntoResponse {
    let text = match req.get("config").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "config field required"}))).into_response(),
    };
    if toml::from_str::<toml::Value>(text).is_err() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid TOML"}))).into_response();
    }
    match tokio::fs::write("config/watchdog.toml", text).await {
        Ok(_) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}
