# Medium Architecture Fixes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 6 medium-severity architectural issues from the HydeClaw audit — tool dispatch, error handling, audit logging, approval extraction, transaction boundaries, handler decomposition.

**Architecture:** Each task is independent and produces a working commit. No cross-task dependencies except Task 1 (ApiError) which other handlers will adopt. Order: ApiError first (foundation), then dispatch table, audit queue, approval manager, transactions, handler split.

**Tech Stack:** Rust (axum, sqlx, tokio, serde), PostgreSQL

---

## File Structure

| Task | Creates | Modifies |
|------|---------|----------|
| 1. ApiError | `src/gateway/error.rs` | `src/gateway/mod.rs`, 5+ handler files |
| 2. Dispatch table | `src/agent/tool_registry_core.rs` | `src/agent/engine_dispatch.rs` |
| 3. Audit queue | `src/db/audit_queue.rs` | `src/agent/engine_dispatch.rs` |
| 4. ApprovalManager | `src/agent/approval_manager.rs` | `src/agent/engine_dispatch.rs` |
| 5. Batch transactions | — | `src/db/memory_queries.rs`, `src/agent/knowledge_extractor.rs` |
| 6. Handler split | `src/gateway/handlers/agents/` (3 files) | `src/gateway/handlers/agents.rs` → dir |

All paths relative to `crates/hydeclaw-core/`.

---

### Task 1: Standard ApiError Type (#21)

**Files:**
- Create: `crates/hydeclaw-core/src/gateway/error.rs`
- Modify: `crates/hydeclaw-core/src/gateway/mod.rs` (add `mod error;`)
- Modify: `crates/hydeclaw-core/src/gateway/handlers/sessions.rs` (adopt ApiError in 2-3 handlers as proof)
- Test: cargo check + existing tests

- [ ] **Step 1: Create `gateway/error.rs` with ApiError enum**

```rust
use axum::response::{IntoResponse, Response};
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Conflict(String),
    Forbidden(String),
    Internal(String),
    ServiceUnavailable(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            ApiError::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
        };
        (status, Json(json!({"error": message}))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e.to_string())
    }
}
```

- [ ] **Step 2: Register module in `gateway/mod.rs`**

Add `pub mod error;` after existing module declarations. Add `pub use error::ApiError;`.

- [ ] **Step 3: Migrate 3 handlers in sessions.rs as proof**

Replace:
```rust
Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
```
With:
```rust
Err(e) => return ApiError::Internal(e.to_string()).into_response(),
```

Migrate `api_list_sessions`, `api_get_session`, `api_delete_session` — these are simple CRUD handlers.

- [ ] **Step 4: cargo check + cargo test**

Run: `cargo check --quiet && cargo test sessions`
Expected: PASS (no behavior change, same JSON output)

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/error.rs crates/hydeclaw-core/src/gateway/mod.rs crates/hydeclaw-core/src/gateway/handlers/sessions.rs
git commit -m "refactor: add ApiError type, migrate sessions handlers"
```

---

### Task 2: Tool Dispatch Table (#4)

**Files:**
- Create: `crates/hydeclaw-core/src/agent/tool_registry_core.rs`
- Modify: `crates/hydeclaw-core/src/agent/engine_dispatch.rs:289-529`
- Test: cargo check + existing tests

The 29-if-chain is the dispatch function `dispatch_core_tool()` in engine_dispatch.rs. Replace with a static HashMap lookup.

- [ ] **Step 1: Create `tool_registry_core.rs` with dispatch table type**

```rust
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;

type ToolHandler = for<'a> fn(
    &'a super::AgentEngine,
    serde_json::Value,
) -> Pin<Box<dyn Future<Output = String> + Send + 'a>>;

static CORE_TOOLS: LazyLock<HashMap<&'static str, ToolHandler>> = LazyLock::new(|| {
    let mut m: HashMap<&'static str, ToolHandler> = HashMap::new();
    m.insert("workspace_write", |e, a| Box::pin(e.handle_workspace_write(a)));
    m.insert("workspace_read", |e, a| Box::pin(e.handle_workspace_read(a)));
    m.insert("workspace_edit", |e, a| Box::pin(e.handle_workspace_edit(a)));
    m.insert("workspace_delete", |e, a| Box::pin(e.handle_workspace_delete(a)));
    m.insert("workspace_rename", |e, a| Box::pin(e.handle_workspace_rename(a)));
    m.insert("workspace_ls", |e, a| Box::pin(e.handle_workspace_ls(a)));
    m.insert("memory_search", |e, a| Box::pin(e.handle_memory_search(a)));
    m.insert("memory_write", |e, a| Box::pin(e.handle_memory_write(a)));
    // ... register all 29 tools
    m
});

pub fn lookup_core_tool(name: &str) -> Option<&'static ToolHandler> {
    CORE_TOOLS.get(name)
}
```

- [ ] **Step 2: Replace if-chain in `engine_dispatch.rs`**

Replace lines 289-529 with:
```rust
if let Some(handler) = crate::agent::tool_registry_core::lookup_core_tool(name) {
    return handler(self, args).await;
}
// Fallback: YAML tools, MCP tools, ToolRegistry (unchanged)
```

Note: Tools with complex inline logic (git, session, skill) need their dispatch extracted to dedicated `handle_*` methods first. If a tool has >5 lines of inline logic in the if-body, extract it to `fn handle_git_tool(&self, args) -> String` etc. before registering.

- [ ] **Step 3: Handle multi-action tools (git, session, process)**

For tools like `git` that have nested `action` dispatch inside the if-body:
1. Create `handle_git_tool(&self, args: Value) -> String` that contains the match on action
2. Register `m.insert("git", |e, a| Box::pin(e.handle_git_tool(a)));`

Same for `session`, `skill`, `process`.

- [ ] **Step 4: cargo check + cargo test**

Run: `cargo check --quiet && cargo test`
Expected: PASS (identical behavior, O(1) lookup instead of O(n))

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/agent/tool_registry_core.rs crates/hydeclaw-core/src/agent/engine_dispatch.rs
git commit -m "refactor: replace 29-if tool dispatch chain with HashMap lookup"
```

---

### Task 3: Audit Queue (#26)

**Files:**
- Create: `crates/hydeclaw-core/src/db/audit_queue.rs`
- Modify: `crates/hydeclaw-core/src/agent/engine_dispatch.rs:46-83`
- Test: cargo check + new unit test

Replace 3 fire-and-forget `tokio::spawn` calls with a bounded channel + background worker.

- [ ] **Step 1: Create `audit_queue.rs`**

```rust
use tokio::sync::mpsc;

pub enum AuditEvent {
    ToolExecution {
        agent_name: String,
        session_id: Option<uuid::Uuid>,
        tool_name: String,
        params: Option<String>,
        status: &'static str,
        duration_ms: u64,
        error: Option<String>,
    },
    ToolQuality {
        tool_name: String,
        success: bool,
        duration_ms: u64,
        error: Option<String>,
    },
}

pub struct AuditQueue {
    tx: mpsc::Sender<AuditEvent>,
}

impl AuditQueue {
    pub fn new(db: sqlx::PgPool) -> Self {
        let (tx, mut rx) = mpsc::channel::<AuditEvent>(1024);
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    AuditEvent::ToolExecution { agent_name, session_id, tool_name, params, status, duration_ms, error } => {
                        if let Err(e) = crate::db::tool_audit::record_tool_execution(
                            &db, &agent_name, session_id, &tool_name,
                            params.as_deref(), status, Some(duration_ms), error.as_deref(),
                        ).await {
                            tracing::warn!(error = %e, tool = %tool_name, "audit write failed");
                        }
                    }
                    AuditEvent::ToolQuality { tool_name, success, duration_ms, error } => {
                        if let Err(e) = crate::db::tool_quality::record_tool_result(
                            &db, &tool_name, success, duration_ms, error.as_deref(),
                        ).await {
                            tracing::warn!(error = %e, tool = %tool_name, "quality write failed");
                        }
                    }
                }
            }
        });
        Self { tx }
    }

    pub fn send(&self, event: AuditEvent) {
        if self.tx.try_send(event).is_err() {
            tracing::warn!("audit queue full — dropping event");
        }
    }
}
```

- [ ] **Step 2: Register in engine/AppState**

Add `audit_queue: Arc<AuditQueue>` to AgentEngine or pass via AgentDeps. Initialize in main.rs alongside db pool.

- [ ] **Step 3: Replace spawns in engine_dispatch.rs**

Replace lines 57-82 (two `tokio::spawn` blocks) with:
```rust
self.audit_queue.send(AuditEvent::ToolExecution { ... });
self.audit_queue.send(AuditEvent::ToolQuality { ... });
```

- [ ] **Step 4: cargo check + cargo test**

Run: `cargo check --quiet && cargo test`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/db/audit_queue.rs crates/hydeclaw-core/src/agent/engine_dispatch.rs crates/hydeclaw-core/src/main.rs
git commit -m "refactor: replace fire-and-forget audit spawns with bounded queue"
```

---

### Task 4: ApprovalManager Extraction (#16)

**Files:**
- Create: `crates/hydeclaw-core/src/agent/approval_manager.rs`
- Modify: `crates/hydeclaw-core/src/agent/engine_dispatch.rs:104-277`
- Test: cargo check + existing tests

Extract the 173-line approval workflow from `execute_tool_call_inner` into a dedicated struct.

- [ ] **Step 1: Create `approval_manager.rs`**

```rust
use anyhow::Result;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, RwLock, Mutex};

type ApprovalWaiters = Arc<RwLock<HashMap<uuid::Uuid, (oneshot::Sender<bool>, std::time::Instant)>>>;

pub struct ApprovalManager {
    db: PgPool,
    waiters: ApprovalWaiters,
    timeout_secs: u64,
}

impl ApprovalManager {
    pub fn new(db: PgPool, waiters: ApprovalWaiters, timeout_secs: u64) -> Self {
        Self { db, waiters, timeout_secs }
    }

    /// Check if tool needs approval, create record, wait for result.
    /// Returns Ok(true) if approved, Ok(false) if rejected, Err on timeout/error.
    pub async fn request_approval(
        &self,
        agent_name: &str,
        tool_name: &str,
        tool_args: &serde_json::Value,
        channel_router: Option<&crate::channels::ChannelRouter>,
        ui_event_tx: &tokio::sync::broadcast::Sender<String>,
    ) -> Result<bool> {
        // 1. Create approval record in DB
        // 2. Send notification to channel + UI
        // 3. Insert oneshot waiter
        // 4. Wait with timeout
        // 5. Cleanup waiter on completion/timeout
        // (move lines 114-265 from engine_dispatch.rs here)
        todo!("migrate from engine_dispatch.rs lines 114-265")
    }

    /// Prune stale waiters (called periodically or on message start)
    pub async fn prune_stale(&self) {
        let mut waiters = self.waiters.write().await;
        let now = std::time::Instant::now();
        waiters.retain(|_, (_, created)| {
            now.duration_since(*created) < std::time::Duration::from_secs(600)
        });
    }
}
```

- [ ] **Step 2: Move approval logic from engine_dispatch.rs**

Replace lines 104-277 in `execute_tool_call_inner` with:
```rust
if needs_approval {
    match self.approval_manager.request_approval(
        &self.name(), name, &args,
        self.channel_router.as_ref().map(|r| r.as_ref()),
        &self.ui_event_tx,
    ).await {
        Ok(true) => { /* approved, continue */ }
        Ok(false) => return "Tool execution rejected by user".to_string(),
        Err(e) => return format!("Approval error: {e}"),
    }
}
```

- [ ] **Step 3: Wire ApprovalManager into AgentEngine**

Add `approval_manager: Arc<ApprovalManager>` field. Initialize in engine constructor using existing `approval_waiters` map.

- [ ] **Step 4: cargo check + cargo test**

Run: `cargo check --quiet && cargo test`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/agent/approval_manager.rs crates/hydeclaw-core/src/agent/engine_dispatch.rs
git commit -m "refactor: extract ApprovalManager from engine_dispatch"
```

---

### Task 5: Transaction Boundaries for Batch Operations (#12)

**Files:**
- Modify: `crates/hydeclaw-core/src/db/memory_queries.rs` (add `insert_chunk_tx` variant)
- Modify: `crates/hydeclaw-core/src/memory.rs` (`index_batch` uses transaction)
- Test: cargo check + existing memory tests

- [ ] **Step 1: Add transaction-accepting insert function**

In `memory_queries.rs`, add alongside existing `insert_chunk`:
```rust
pub async fn insert_chunk_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    // same params as insert_chunk
) -> Result<()> {
    // Same SQL, but uses tx instead of db pool
    let sql = format!(/* same INSERT */);
    sqlx::query(&sql)
        // same bindings
        .execute(&mut **tx)
        .await
        .context("failed to insert memory chunk (tx)")?;
    Ok(())
}
```

- [ ] **Step 2: Wrap `index_batch` in transaction**

In `memory.rs`, modify the batch insert loop in `index_batch()`:
```rust
let mut tx = self.db.begin().await.context("failed to begin transaction for batch index")?;
for (i, item) in items.iter().enumerate() {
    crate::db::memory_queries::insert_chunk_tx(&mut tx, /* params */).await?;
}
tx.commit().await.context("failed to commit batch index")?;
```

- [ ] **Step 3: cargo check + cargo test memory**

Run: `cargo check --quiet && cargo test memory`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/db/memory_queries.rs crates/hydeclaw-core/src/memory.rs
git commit -m "refactor: wrap memory batch index in transaction"
```

---

### Task 6: Split agents.rs Handler (1630 lines → 3 files) (#22)

**Files:**
- Create: `crates/hydeclaw-core/src/gateway/handlers/agents/mod.rs`
- Create: `crates/hydeclaw-core/src/gateway/handlers/agents/crud.rs`
- Create: `crates/hydeclaw-core/src/gateway/handlers/agents/lifecycle.rs`
- Create: `crates/hydeclaw-core/src/gateway/handlers/agents/schema.rs`
- Delete: `crates/hydeclaw-core/src/gateway/handlers/agents.rs`
- Modify: `crates/hydeclaw-core/src/gateway/handlers/mod.rs` (update module path)
- Test: cargo check + existing tests

- [ ] **Step 1: Create directory + mod.rs**

```bash
mkdir -p crates/hydeclaw-core/src/gateway/handlers/agents
```

`agents/mod.rs`:
```rust
mod crud;
mod lifecycle;
mod schema;

pub(crate) use crud::*;
pub(crate) use lifecycle::*;
pub(crate) use schema::*;

pub(crate) fn routes() -> axum::Router<crate::gateway::state::AppState> {
    // Move routes() from old agents.rs
}
```

- [ ] **Step 2: Split by responsibility**

- `crud.rs`: `api_list_agents`, `api_get_agent`, `api_create_agent`, `api_update_agent`, `api_delete_agent`, `api_rename_agent` (~600 lines)
- `lifecycle.rs`: `api_start_agent`, `api_stop_agent`, `api_restart_agent`, agent config loading, TOML parsing (~500 lines)
- `schema.rs`: Types, validation, scaffold generation, `AgentCreatePayload`, `AgentUpdatePayload` (~530 lines)

Move functions by cutting from old agents.rs into new files. Keep imports minimal — each file imports only what it needs.

- [ ] **Step 3: Delete old agents.rs, update mod.rs**

In `handlers/mod.rs`, change `mod agents;` — Rust automatically picks up `agents/mod.rs` over `agents.rs`.

- [ ] **Step 4: cargo check + cargo test agents**

Run: `cargo check --quiet && cargo test agents`
Expected: PASS (no behavior change)

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/agents/
git rm crates/hydeclaw-core/src/gateway/handlers/agents.rs
git commit -m "refactor: split agents.rs (1630 lines) into crud/lifecycle/schema modules"
```

---

## Execution Order

```
Task 1 (ApiError)      ← foundation, other tasks can adopt
Task 2 (Dispatch)      ← independent
Task 3 (Audit Queue)   ← independent
Task 4 (Approval)      ← independent
Task 5 (Transactions)  ← independent
Task 6 (Handler split) ← independent, can adopt ApiError from Task 1
```

Tasks 2-6 are independent of each other and can be parallelized with subagents.

## Not Included (deferred to Phase 4)

- #1/#2/#3 God objects (AppState, AgentEngine, MemoryStore) — require holistic redesign
- #7 main.rs decomposition — depends on god object refactoring
- #24 Provider fallback chain — requires new trait design
- #10/#13 Config hot-reload + auth middleware — cross-cutting concerns
