# Session Auto-Retry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically retry sessions where the agent crashed mid-response, guaranteeing the user gets an answer.

**Architecture:** Migration adds `retry_count` to sessions. Backend endpoint `POST /api/sessions/{id}/retry` replays the last user message through `handle_with_status`. Watchdog periodically queries for stuck sessions and calls the retry endpoint.

**Tech Stack:** Rust (sqlx, axum), TOML config

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `migrations/017_session_retry_count.sql` | Add `retry_count` column |
| Modify | `crates/hydeclaw-core/src/db/sessions.rs` | Add `retry_count` to SessionRow, query helpers |
| Modify | `crates/hydeclaw-core/src/gateway/handlers/sessions.rs` | Add `POST /api/sessions/{id}/retry` endpoint |
| Modify | `crates/hydeclaw-core/src/gateway/mod.rs` | Register retry route |
| Modify | `crates/hydeclaw-watchdog/src/main.rs` | Add stuck session detection + retry loop |
| Modify | `crates/hydeclaw-watchdog/src/config.rs` | Add retry config fields |

---

### Task 1: Database Migration

**Files:**
- Create: `migrations/017_session_retry_count.sql`

- [ ] **Step 1: Write migration**

```sql
-- 017_session_retry_count.sql
-- Track how many times a stuck session has been auto-retried.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS retry_count INTEGER NOT NULL DEFAULT 0;
```

- [ ] **Step 2: Commit**

```bash
git add migrations/017_session_retry_count.sql
git commit -m "feat(db): add retry_count column to sessions (migration 017)"
```

---

### Task 2: Backend — DB helpers

**Files:**
- Modify: `crates/hydeclaw-core/src/db/sessions.rs`

- [ ] **Step 1: Add `retry_count` to `SessionRow` struct**

Find the `SessionRow` struct (has fields like `id`, `agent_id`, `run_status`, etc.) and add after `run_status`:

```rust
    #[sqlx(default)]
    pub retry_count: i32,
```

- [ ] **Step 2: Add `find_stuck_sessions` query function**

Add after `set_session_run_status`:

```rust
/// Find sessions stuck in 'running' state where the last message is from the user
/// (assistant never responded) and enough time has passed.
pub async fn find_stuck_sessions(
    db: &PgPool,
    stale_secs: i64,
    max_retries: i32,
) -> Result<Vec<(Uuid, String)>> {
    // Returns (session_id, agent_id) pairs
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT s.id, s.agent_id FROM sessions s \
         WHERE s.run_status = 'running' \
           AND s.last_message_at < NOW() - make_interval(secs => $1) \
           AND s.retry_count < $2 \
           AND (SELECT role FROM messages WHERE session_id = s.id ORDER BY created_at DESC LIMIT 1) = 'user'"
    )
    .bind(stale_secs as f64)
    .bind(max_retries)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Increment retry_count and reset run_status to 'running'.
pub async fn increment_retry_count(db: &PgPool, session_id: Uuid) -> Result<i32> {
    let new_count: i32 = sqlx::query_scalar(
        "UPDATE sessions SET retry_count = retry_count + 1, run_status = 'running' \
         WHERE id = $1 RETURNING retry_count"
    )
    .bind(session_id)
    .fetch_one(db)
    .await?;
    Ok(new_count)
}

/// Mark a session as permanently failed after max retries exhausted.
pub async fn mark_session_failed(db: &PgPool, session_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE sessions SET run_status = 'failed' WHERE id = $1")
        .bind(session_id)
        .execute(db)
        .await?;
    Ok(())
}
```

- [ ] **Step 3: Add `get_last_user_message` helper**

```rust
/// Get the last user message text from a session (for retry).
pub async fn get_last_user_message(db: &PgPool, session_id: Uuid) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT content FROM messages \
         WHERE session_id = $1 AND role = 'user' \
         ORDER BY created_at DESC LIMIT 1"
    )
    .bind(session_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(c,)| c))
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check --all-targets`

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/db/sessions.rs
git commit -m "feat(db): add stuck session detection and retry helpers"
```

---

### Task 3: Backend — Retry endpoint

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/sessions.rs`
- Modify: `crates/hydeclaw-core/src/gateway/mod.rs` (route registration)

- [ ] **Step 1: Add retry handler in sessions.rs**

Add at the end of the file:

```rust
/// POST /api/sessions/{id}/retry
/// Replays the last user message through the engine. Used by watchdog for auto-retry.
pub(crate) async fn api_retry_session(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> impl IntoResponse {
    // 1. Validate session exists and is retryable
    let session = match sessions::find_session(&state.db, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"error": "session not found"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    // 2. Get last user message
    let user_text = match sessions::get_last_user_message(&state.db, id).await {
        Ok(Some(text)) => text,
        Ok(None) => return (StatusCode::BAD_REQUEST, Json(json!({"error": "no user message found in session"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    // 3. Increment retry count
    let retry_count = match sessions::increment_retry_count(&state.db, id).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    tracing::info!(session_id = %id, agent = %session.agent_id, retry_count, "retrying stuck session");

    // 4. Get engine for agent
    let engine = match state.get_engine(&session.agent_id).await {
        Some(e) => e,
        None => {
            let _ = sessions::mark_session_failed(&state.db, id).await;
            return (StatusCode::NOT_FOUND, Json(json!({"error": format!("agent '{}' not found", session.agent_id)}))).into_response();
        }
    };

    // 5. Build IncomingMessage and run handle_with_status (no SSE — direct execution)
    let msg = crate::agent::engine::IncomingMessage {
        text: Some(user_text),
        user_id: session.user_id.clone(),
        channel: session.channel.clone(),
        agent_id: session.agent_id.clone(),
        context: Default::default(),
        attachments: vec![],
        leaf_message_id: None,
        tool_policy_override: None,
    };

    // Spawn background task — don't block the HTTP response
    let db = state.db.clone();
    tokio::spawn(async move {
        match engine.handle_with_status(&msg, None, None).await {
            Ok(_response) => {
                tracing::info!(session_id = %id, "retry succeeded");
                // handle_with_status already sets run_status to 'done'
            }
            Err(e) => {
                tracing::error!(session_id = %id, error = %e, "retry failed");
                sessions::mark_session_failed(&db, id).await.ok();
            }
        }
    });

    Json(json!({"ok": true, "retry_count": retry_count, "session_id": id})).into_response()
}
```

- [ ] **Step 2: Register the route**

In `crates/hydeclaw-core/src/gateway/handlers/sessions.rs`, find the `pub(crate) fn routes()` function and add:

```rust
.route("/api/sessions/{id}/retry", post(api_retry_session))
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check --all-targets`

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/sessions.rs crates/hydeclaw-core/src/gateway/mod.rs
git commit -m "feat(api): add POST /api/sessions/{id}/retry endpoint"
```

---

### Task 4: Watchdog — Config

**Files:**
- Modify: `crates/hydeclaw-watchdog/src/config.rs`

- [ ] **Step 1: Add retry settings to WatchdogSettings**

Add these fields to the `WatchdogSettings` struct:

```rust
    #[serde(default = "default_true")]
    pub session_retry_enabled: bool,
    #[serde(default = "default_90")]
    pub session_retry_stale_secs: u64,
    #[serde(default = "default_3")]
    pub session_retry_max_attempts: u32,
```

Add the missing default function:

```rust
fn default_90() -> u64 { 90 }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p hydeclaw-watchdog`

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-watchdog/src/config.rs
git commit -m "feat(watchdog): add session retry config fields"
```

---

### Task 5: Watchdog — Stuck session retry loop

**Files:**
- Modify: `crates/hydeclaw-watchdog/src/main.rs`

- [ ] **Step 1: Add retry logic to the main loop**

Inside the `loop { ... }` block, after the resource check section, add:

```rust
        // ── Session auto-retry ──────────────────────────────────────────
        if cfg.watchdog.session_retry_enabled {
            match http
                .get(format!("{}/api/sessions/stuck?stale_secs={}&max_retries={}",
                    core_url, cfg.watchdog.session_retry_stale_secs, cfg.watchdog.session_retry_max_attempts))
                .header("Authorization", format!("Bearer {}", auth_token))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if let Some(sessions) = body.get("sessions").and_then(|s| s.as_array()) {
                            for s in sessions {
                                let sid = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                let agent = s.get("agent_id").and_then(|v| v.as_str()).unwrap_or("?");
                                tracing::warn!(session_id = sid, agent, "retrying stuck session");

                                match http
                                    .post(format!("{}/api/sessions/{}/retry", core_url, sid))
                                    .header("Authorization", format!("Bearer {}", auth_token))
                                    .send()
                                    .await
                                {
                                    Ok(r) if r.status().is_success() => {
                                        tracing::info!(session_id = sid, "retry request accepted");
                                        alerter.send(&alert_config, "session_retry",
                                            &format!("Session retry: {}", agent),
                                            &format!("Auto-retrying stuck session {} (agent: {})", sid, agent),
                                        ).await;
                                    }
                                    Ok(r) => {
                                        let status = r.status();
                                        let body = r.text().await.unwrap_or_default();
                                        tracing::error!(session_id = sid, status = %status, body, "retry request failed");
                                    }
                                    Err(e) => tracing::error!(session_id = sid, error = %e, "retry request error"),
                                }
                            }
                        }
                    }
                }
                Ok(resp) => tracing::debug!(status = %resp.status(), "stuck sessions check returned non-200"),
                Err(e) => tracing::debug!(error = %e, "stuck sessions check failed"),
            }
        }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p hydeclaw-watchdog`

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-watchdog/src/main.rs
git commit -m "feat(watchdog): auto-retry stuck sessions via POST /api/sessions/{id}/retry"
```

---

### Task 6: Backend — Stuck sessions query endpoint

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/sessions.rs`

- [ ] **Step 1: Add GET /api/sessions/stuck endpoint**

The watchdog queries this to find sessions needing retry:

```rust
#[derive(Debug, Deserialize)]
pub(crate) struct StuckSessionsQuery {
    stale_secs: Option<i64>,
    max_retries: Option<i32>,
}

/// GET /api/sessions/stuck
/// Returns sessions stuck in 'running' where the last message is from the user.
pub(crate) async fn api_stuck_sessions(
    State(state): State<AppState>,
    Query(q): Query<StuckSessionsQuery>,
) -> impl IntoResponse {
    let stale_secs = q.stale_secs.unwrap_or(90);
    let max_retries = q.max_retries.unwrap_or(3);

    match sessions::find_stuck_sessions(&state.db, stale_secs, max_retries).await {
        Ok(rows) => {
            let sessions: Vec<Value> = rows.iter().map(|(id, agent)| {
                json!({"id": id, "agent_id": agent})
            }).collect();
            Json(json!({"sessions": sessions})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}
```

- [ ] **Step 2: Register the route**

Add to the routes function:

```rust
.route("/api/sessions/stuck", get(api_stuck_sessions))
```

Note: this route must be registered BEFORE `/api/sessions/{id}` to avoid path conflict.

- [ ] **Step 3: Verify compilation**

Run: `cargo check --all-targets`

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/sessions.rs
git commit -m "feat(api): add GET /api/sessions/stuck endpoint for watchdog"
```

---

### Task 7: Integration test

- [ ] **Step 1: Manual verification**

1. Deploy to Pi
2. Send a message that triggers agent tool (multi-agent delegation)
3. Kill hydeclaw-core mid-processing (`kill -9`)
4. Restart hydeclaw-core
5. Wait 90+ seconds
6. Check watchdog logs: should see "retrying stuck session"
7. Verify the user gets a response

- [ ] **Step 2: Commit any fixes**

If issues found, fix and commit.
