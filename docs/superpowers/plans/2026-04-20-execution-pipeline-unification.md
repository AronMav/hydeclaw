# Execution Pipeline Unification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify `handle_sse`, `handle_with_status`, `handle_streaming` into a single `pipeline::execute<S: EventSink>` with `bootstrap` / `execute` / `finalize` modules, while keeping the public API of `AgentEngine` backwards-compatible.

**Architecture:** New `pipeline/sink.rs` defines `EventSink` trait over a `PipelineEvent` type (wraps `StreamEvent` and `ProcessingPhase`). New `pipeline/{bootstrap,execute,finalize}.rs` host the three phases of the agent turn. Three thin adapter methods on `AgentEngine` construct a sink and delegate. Existing `SessionLifecycleGuard`, `ProcessingGuard`, `LoopDetector`, `chat_stream_with_transient_retry` are reused as-is.

**Tech Stack:** Rust 2024, tokio, sqlx (PostgreSQL), flume, tokio::sync::mpsc, `tokio_util::sync::CancellationToken`, `async fn in trait`, `tokio-test`, `sqlx::test` macro.

**Spec:** [docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md](../specs/2026-04-20-execution-pipeline-unification-design.md)

---

## File structure

**Created:**
- `crates/hydeclaw-core/src/agent/pipeline/sink.rs` — `EventSink` trait, `PipelineEvent`, `SinkError`, three production sinks, `MockSink` (cfg(test))
- `crates/hydeclaw-core/src/agent/pipeline/bootstrap.rs` — session entry, user message save, WAL running, ProcessingGuard creation, slash-command early exit
- `crates/hydeclaw-core/src/agent/pipeline/execute.rs` — main LLM+tools loop over `EventSink`
- `crates/hydeclaw-core/src/agent/pipeline/finalize.rs` — single exit point: persist, WAL done/failed/interrupted, knowledge extraction
- `crates/hydeclaw-core/src/agent/engine/run.rs` — three thin wrappers (`handle_sse`, `handle_with_status`, `handle_streaming`) that build `EventSink` + `ExecutionContext` and call `pipeline::execute`
- `crates/hydeclaw-core/tests/pipeline_snapshots.rs` — regression snapshots for three entry points

**Modified:**
- `crates/hydeclaw-core/src/agent/pipeline/mod.rs` — add `sink`, `bootstrap`, `execute`, `finalize` submodules
- `crates/hydeclaw-core/src/agent/engine/mod.rs` — register `run` submodule
- `crates/hydeclaw-core/src/agent/mod.rs` — remove `engine_execution`, `engine_sse` modules after Task 10
- `crates/hydeclaw-core/src/agent/session_manager.rs` — add `SessionLifecycleGuard::interrupt(reason)` method (Task 4)
- `CLAUDE.md` — update `Agent Engine` architecture section (Task 11)
- `crates/hydeclaw-core/Cargo.toml` — add `scopeguard = "1.2"` (Task 5)

**Deleted (Task 10):**
- `crates/hydeclaw-core/src/agent/engine_execution.rs`
- `crates/hydeclaw-core/src/agent/engine_sse.rs`
- `crates/hydeclaw-core/src/agent/pipeline/execution.rs` (helpers absorbed into bootstrap/finalize)
- `crates/hydeclaw-core/src/agent/pipeline/entry.rs` (absorbed into `SseSink`)

---

## Task 1: Regression snapshot tests for three entry points

**Purpose:** Lock current behaviour before any change. These tests stay green through Tasks 4–10.

**Files:**
- Create: `crates/hydeclaw-core/tests/pipeline_snapshots.rs`

### Steps

- [ ] **Step 1: Verify test support module exists**

Run: `ls crates/hydeclaw-core/tests/support/`
Expected: includes `mock_provider.rs` (confirmed to exist at `tests/support/mock_provider.rs:21-135`).

If `mod.rs` is missing or the `support` module is not re-exportable from integration tests, check an existing test such as `crates/hydeclaw-core/tests/integration_aborted_usage.rs` for the import pattern.

- [ ] **Step 2: Write the snapshot test scaffolding**

Create `crates/hydeclaw-core/tests/pipeline_snapshots.rs`:

```rust
//! Regression snapshots for the three agent execution entry points.
//! These tests MUST stay green throughout the pipeline unification refactor.
//! If a snapshot legitimately changes, update it in the same commit with a
//! comment explaining the intentional behaviour change.

mod support;

use hydeclaw_core::agent::engine::StreamEvent;
use hydeclaw_core::agent::engine_event_sender::EngineEventSender;
use hydeclaw_types::IncomingMessage;
use sqlx::PgPool;
use std::sync::Arc;
use support::mock_provider::MockProvider;

/// Helper: drain SSE channel into Vec<StreamEvent> until sender drops.
async fn drain_sse(mut rx: tokio::sync::mpsc::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    out
}

/// Helper: reduce StreamEvent sequence to a canonical "shape" vector that
/// is stable across unrelated refactors (drops message_id UUIDs, timestamps).
fn shape(events: &[StreamEvent]) -> Vec<&'static str> {
    events.iter().map(|e| match e {
        StreamEvent::SessionId(_) => "SessionId",
        StreamEvent::MessageStart { .. } => "MessageStart",
        StreamEvent::TextDelta(_) => "TextDelta",
        StreamEvent::ToolCallStart { .. } => "ToolCallStart",
        StreamEvent::ToolCallArgs { .. } => "ToolCallArgs",
        StreamEvent::ToolCallResult { .. } => "ToolCallResult",
        StreamEvent::Finish { .. } => "Finish",
        StreamEvent::Error(_) => "Error",
        _ => "Other",
    }).collect()
}
```

- [ ] **Step 3: Write failing test for handle_sse happy path**

Append to `pipeline_snapshots.rs`:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn sse_happy_path_snapshot(pool: PgPool) {
    let engine = support::build_test_engine(
        pool.clone(),
        MockProvider::new().expect_text("hello world", "stop"),
    ).await;

    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(256);
    let event_tx = EngineEventSender::new(tx);

    let msg = IncomingMessage {
        text: Some("hi".to_string()),
        ..Default::default()
    };

    let task = tokio::spawn(async move { drain_sse(rx).await });
    let session_id = engine.handle_sse(&msg, event_tx, None, false).await.unwrap();
    let events = task.await.unwrap();

    assert!(!session_id.is_nil(), "session_id returned");
    assert_eq!(
        shape(&events),
        vec!["MessageStart", "TextDelta", "Finish"],
        "snapshot: SSE happy path event shape"
    );
}
```

- [ ] **Step 4: Check whether `support::build_test_engine` already exists**

Run: `grep -rn 'build_test_engine\|fn build_engine' crates/hydeclaw-core/tests/`
Expected: no match OR one matching helper.

If no such helper exists, create it in `crates/hydeclaw-core/tests/support/mod.rs` (create the file if needed):

```rust
pub mod mock_provider;

use hydeclaw_core::agent::engine::AgentEngine;
use sqlx::PgPool;
use std::sync::Arc;

pub async fn build_test_engine(
    db: PgPool,
    provider: mock_provider::MockProvider,
) -> Arc<AgentEngine> {
    // Minimal AgentEngine for tests. Follow the same construction pattern as
    // `crates/hydeclaw-core/tests/integration_aborted_usage.rs` if that file
    // builds an engine; otherwise use the public AgentEngine::builder API.
    // Required fields: db, provider (dyn LlmProvider), agent config with name
    // "test-agent", empty memory store, default ToolLoopConfig.
    todo!("see reference test integration_aborted_usage.rs and replicate the AgentEngine construction pattern used there")
}
```

If `integration_aborted_usage.rs` already constructs an `AgentEngine`, copy its construction code and drop the `todo!`. The point of this helper is that the same builder is reused by all three snapshot tests.

- [ ] **Step 5: Run the test, confirm it fails with a meaningful error**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots sse_happy_path_snapshot -- --nocapture`
Expected: FAIL with either `todo!()` panic (if build_test_engine is stubbed) or the actual snapshot shape discovered. If the test panics on `todo!`, finish `build_test_engine` by copying construction from the reference test. Re-run until the test PASSES with the exact shape listed above.

If the actual shape differs from `["MessageStart", "TextDelta", "Finish"]`, **update the snapshot to match the observed shape** — the point is to freeze current behaviour, not to enforce a hypothesis.

- [ ] **Step 6: Add snapshots for handle_with_status and handle_streaming**

Append to `pipeline_snapshots.rs`:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn with_status_happy_path_snapshot(pool: PgPool) {
    let engine = support::build_test_engine(
        pool.clone(),
        MockProvider::new().expect_text("hello", "stop"),
    ).await;

    let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel();
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();

    let msg = IncomingMessage {
        text: Some("hi".to_string()),
        ..Default::default()
    };

    let status_collector = tokio::spawn(async move {
        let mut v = Vec::new();
        while let Some(p) = status_rx.recv().await { v.push(format!("{:?}", p)); }
        v
    });
    let chunk_collector = tokio::spawn(async move {
        let mut v = Vec::new();
        while let Some(c) = chunk_rx.recv().await { v.push(c); }
        v
    });

    let result = engine.handle_with_status(&msg, Some(status_tx), Some(chunk_tx)).await.unwrap();
    let statuses = status_collector.await.unwrap();
    let chunks = chunk_collector.await.unwrap();

    assert_eq!(result, "hello");
    assert!(statuses.iter().any(|s| s.contains("Thinking")), "saw Thinking phase");
    assert_eq!(chunks.concat(), "hello", "chunks concatenate to final text");
}

#[sqlx::test(migrations = "../../migrations")]
async fn streaming_happy_path_snapshot(pool: PgPool) {
    let engine = support::build_test_engine(
        pool.clone(),
        MockProvider::new().expect_text("world", "stop"),
    ).await;

    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();
    let msg = IncomingMessage {
        text: Some("hi".to_string()),
        ..Default::default()
    };

    let chunks_task = tokio::spawn(async move {
        let mut v = Vec::new();
        while let Some(c) = chunk_rx.recv().await { v.push(c); }
        v
    });
    let result = engine.handle_streaming(&msg, chunk_tx).await.unwrap();
    let chunks = chunks_task.await.unwrap();

    assert_eq!(result, "world");
    assert_eq!(chunks.concat(), "world");
}
```

- [ ] **Step 7: Run all three snapshot tests**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots -- --nocapture`
Expected: PASS on all three. Adjust `expect_text` assertions if the MockProvider API differs; the goal is three green snapshots that lock current behaviour.

- [ ] **Step 8: Commit**

```bash
git add crates/hydeclaw-core/tests/pipeline_snapshots.rs crates/hydeclaw-core/tests/support/
git commit -m "test(pipeline): regression snapshots for three entry points"
```

---

## Task 2: EventSink trait, PipelineEvent, MockSink

**Files:**
- Create: `crates/hydeclaw-core/src/agent/pipeline/sink.rs`
- Modify: `crates/hydeclaw-core/src/agent/pipeline/mod.rs` (add `pub mod sink;`)

### Steps

- [ ] **Step 1: Write failing unit test in sink.rs**

Create `crates/hydeclaw-core/src/agent/pipeline/sink.rs`:

```rust
//! EventSink — transport-agnostic output for pipeline::execute.
//!
//! PipelineEvent is the union of StreamEvent (for web SSE / rich events)
//! and ProcessingPhase (for channel typing indicators). Each sink decides
//! which variants it cares about and silently drops the rest.

use crate::agent::engine::stream::{ProcessingPhase, StreamEvent};

#[derive(Debug, Clone)]
pub enum PipelineEvent {
    Stream(StreamEvent),
    Phase(ProcessingPhase),
}

impl From<StreamEvent> for PipelineEvent {
    fn from(ev: StreamEvent) -> Self { PipelineEvent::Stream(ev) }
}

impl From<ProcessingPhase> for PipelineEvent {
    fn from(p: ProcessingPhase) -> Self { PipelineEvent::Phase(p) }
}

#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("sink closed (client disconnected)")]
    Closed,
    #[error("sink full (backpressure)")]
    Full,
    #[error(transparent)]
    Fatal(#[from] anyhow::Error),
}

pub trait EventSink: Send {
    async fn emit(&mut self, ev: PipelineEvent) -> Result<(), SinkError>;

    /// Graceful flush/close. Default: no-op.
    async fn close(&mut self) -> Result<(), SinkError> { Ok(()) }
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    #[derive(Default, Debug)]
    pub struct MockSink {
        pub events: Vec<PipelineEvent>,
        pub closed_after: Option<usize>, // if set, emit returns Closed after N events
    }

    impl MockSink {
        pub fn new() -> Self { Self::default() }
        pub fn close_after(n: usize) -> Self {
            Self { events: vec![], closed_after: Some(n) }
        }
    }

    impl EventSink for MockSink {
        async fn emit(&mut self, ev: PipelineEvent) -> Result<(), SinkError> {
            if let Some(n) = self.closed_after {
                if self.events.len() >= n { return Err(SinkError::Closed); }
            }
            self.events.push(ev);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_support::MockSink;

    #[tokio::test]
    async fn mock_sink_records_events() {
        let mut sink = MockSink::new();
        sink.emit(PipelineEvent::Stream(StreamEvent::TextDelta("a".into()))).await.unwrap();
        sink.emit(PipelineEvent::Phase(ProcessingPhase::Thinking)).await.unwrap();
        assert_eq!(sink.events.len(), 2);
    }

    #[tokio::test]
    async fn mock_sink_closes_after_limit() {
        let mut sink = MockSink::close_after(1);
        sink.emit(PipelineEvent::Stream(StreamEvent::TextDelta("ok".into()))).await.unwrap();
        let err = sink.emit(PipelineEvent::Stream(StreamEvent::TextDelta("drop".into()))).await;
        assert!(matches!(err, Err(SinkError::Closed)));
    }
}
```

- [ ] **Step 2: Register module in pipeline/mod.rs**

Open `crates/hydeclaw-core/src/agent/pipeline/mod.rs` and add at the top (preserve existing declarations):

```rust
pub mod sink;
```

- [ ] **Step 3: Run tests**

Run: `cd crates/hydeclaw-core && cargo test --lib agent::pipeline::sink -- --nocapture`
Expected: PASS `mock_sink_records_events` and `mock_sink_closes_after_limit`.

- [ ] **Step 4: Verify lint**

Run: `cd crates/hydeclaw-core && cargo clippy --lib -- -D warnings`
Expected: no warnings in `pipeline/sink.rs`.

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/agent/pipeline/sink.rs crates/hydeclaw-core/src/agent/pipeline/mod.rs
git commit -m "feat(pipeline): add EventSink trait and MockSink"
```

---

## Task 3: Three production sinks (SseSink, ChannelStatusSink, ChunkSink)

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/pipeline/sink.rs` (append production sinks + tests)

### Steps

- [ ] **Step 1: Append SseSink to sink.rs**

Append BEFORE the `#[cfg(test)]` block in `pipeline/sink.rs`:

```rust
// ── Production sinks ──────────────────────────────────────────────────

use crate::agent::engine_event_sender::EngineEventSender;

/// SSE transport for web clients. Wraps EngineEventSender which already
/// handles text-vs-non-text delivery semantics.
pub struct SseSink { tx: EngineEventSender }

impl SseSink {
    pub fn new(tx: EngineEventSender) -> Self { Self { tx } }
}

impl EventSink for SseSink {
    async fn emit(&mut self, ev: PipelineEvent) -> Result<(), SinkError> {
        match ev {
            PipelineEvent::Stream(se) => {
                self.tx.send_async(se).await.map_err(|_| SinkError::Closed)
            }
            PipelineEvent::Phase(_) => Ok(()), // SSE does not transport typing indicator
        }
    }
}

/// Channel adapter transport (Telegram/Discord) — two channels: status+chunks.
pub struct ChannelStatusSink {
    status_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::agent::engine::stream::ProcessingPhase>>,
    chunk_tx:  Option<tokio::sync::mpsc::UnboundedSender<String>>,
    pub buffer: String,
}

impl ChannelStatusSink {
    pub fn new(
        status_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::agent::engine::stream::ProcessingPhase>>,
        chunk_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Self { Self { status_tx, chunk_tx, buffer: String::new() } }
}

impl EventSink for ChannelStatusSink {
    async fn emit(&mut self, ev: PipelineEvent) -> Result<(), SinkError> {
        match ev {
            PipelineEvent::Phase(p) => {
                if let Some(tx) = &self.status_tx { let _ = tx.send(p); }
                Ok(())
            }
            PipelineEvent::Stream(StreamEvent::TextDelta(s)) => {
                self.buffer.push_str(&s);
                if let Some(tx) = &self.chunk_tx {
                    tx.send(s).map_err(|_| SinkError::Closed)
                } else { Ok(()) }
            }
            _ => Ok(()), // tool/file/card events not relevant to channel transport
        }
    }
}

/// Plain-chunk transport — only text deltas, no status.
pub struct ChunkSink {
    chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
    pub buffer: String,
}

impl ChunkSink {
    pub fn new(chunk_tx: tokio::sync::mpsc::UnboundedSender<String>) -> Self {
        Self { chunk_tx, buffer: String::new() }
    }
}

impl EventSink for ChunkSink {
    async fn emit(&mut self, ev: PipelineEvent) -> Result<(), SinkError> {
        if let PipelineEvent::Stream(StreamEvent::TextDelta(s)) = ev {
            self.buffer.push_str(&s);
            self.chunk_tx.send(s).map_err(|_| SinkError::Closed)
        } else { Ok(()) }
    }
}
```

- [ ] **Step 2: Append unit tests inside the existing `#[cfg(test)] mod tests`**

Append inside the `mod tests` block in `pipeline/sink.rs`:

```rust
    #[tokio::test]
    async fn sse_sink_forwards_stream_events() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamEvent>(8);
        let engine_tx = crate::agent::engine_event_sender::EngineEventSender::new(tx);
        let mut sink = SseSink::new(engine_tx);
        sink.emit(PipelineEvent::Stream(StreamEvent::TextDelta("hi".into()))).await.unwrap();
        let got = rx.recv().await.unwrap();
        assert!(matches!(got, StreamEvent::TextDelta(ref s) if s == "hi"));
    }

    #[tokio::test]
    async fn sse_sink_ignores_phase() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamEvent>(8);
        let engine_tx = crate::agent::engine_event_sender::EngineEventSender::new(tx);
        let mut sink = SseSink::new(engine_tx);
        sink.emit(PipelineEvent::Phase(ProcessingPhase::Thinking)).await.unwrap();
        drop(sink);
        assert!(rx.recv().await.is_none(), "no StreamEvent forwarded for Phase");
    }

    #[tokio::test]
    async fn sse_sink_returns_closed_on_dropped_receiver() {
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(8);
        let engine_tx = crate::agent::engine_event_sender::EngineEventSender::new(tx);
        let mut sink = SseSink::new(engine_tx);
        drop(rx);
        let err = sink.emit(PipelineEvent::Stream(StreamEvent::TextDelta("x".into()))).await;
        assert!(matches!(err, Err(SinkError::Closed)));
    }

    #[tokio::test]
    async fn channel_status_sink_routes_phase_to_status() {
        let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel::<ProcessingPhase>();
        let (chunk_tx, _chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut sink = ChannelStatusSink::new(Some(status_tx), Some(chunk_tx));
        sink.emit(PipelineEvent::Phase(ProcessingPhase::Thinking)).await.unwrap();
        assert!(matches!(status_rx.recv().await, Some(ProcessingPhase::Thinking)));
    }

    #[tokio::test]
    async fn channel_status_sink_routes_text_to_chunks() {
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut sink = ChannelStatusSink::new(None, Some(chunk_tx));
        sink.emit(PipelineEvent::Stream(StreamEvent::TextDelta("hello".into()))).await.unwrap();
        assert_eq!(chunk_rx.recv().await, Some("hello".to_string()));
        assert_eq!(sink.buffer, "hello");
    }

    #[tokio::test]
    async fn channel_status_sink_drops_tool_events() {
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut sink = ChannelStatusSink::new(None, Some(chunk_tx));
        sink.emit(PipelineEvent::Stream(StreamEvent::MessageStart { message_id: "m1".into() })).await.unwrap();
        drop(sink);
        assert!(chunk_rx.recv().await.is_none(), "no chunk for MessageStart");
    }

    #[tokio::test]
    async fn chunk_sink_emits_only_text_deltas() {
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut sink = ChunkSink::new(chunk_tx);
        sink.emit(PipelineEvent::Stream(StreamEvent::TextDelta("abc".into()))).await.unwrap();
        sink.emit(PipelineEvent::Stream(StreamEvent::MessageStart { message_id: "m".into() })).await.unwrap();
        assert_eq!(chunk_rx.recv().await, Some("abc".into()));
        drop(sink);
        assert!(chunk_rx.recv().await.is_none());
    }
```

- [ ] **Step 3: Run tests**

Run: `cd crates/hydeclaw-core && cargo test --lib agent::pipeline::sink`
Expected: all eight tests (`mock_sink_*` + six new sink tests) PASS.

- [ ] **Step 4: Lint**

Run: `cd crates/hydeclaw-core && cargo clippy --lib -- -D warnings`
Expected: zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/agent/pipeline/sink.rs
git commit -m "feat(pipeline): SseSink, ChannelStatusSink, ChunkSink implementations"
```

---

## Task 4: Extract finalize() — single exit point

**Purpose:** Centralize persistence + WAL + knowledge extraction. Both `engine_execution.rs` and `engine_sse.rs` delegate to `pipeline::finalize` in this commit; snapshots stay green. Also add `SessionLifecycleGuard::interrupt(reason)` since pipeline will need it.

**Files:**
- Create: `crates/hydeclaw-core/src/agent/pipeline/finalize.rs`
- Modify: `crates/hydeclaw-core/src/agent/session_manager.rs` (add `interrupt` method)
- Modify: `crates/hydeclaw-core/src/agent/pipeline/mod.rs` (add `pub mod finalize;`)
- Modify: `crates/hydeclaw-core/src/agent/engine_execution.rs` (replace tail with `finalize` call)
- Modify: `crates/hydeclaw-core/src/agent/engine_sse.rs` (replace tail with `finalize` call)

### Steps

- [ ] **Step 1: Add `interrupt` method to SessionLifecycleGuard**

Open `crates/hydeclaw-core/src/agent/session_manager.rs` and add after the existing `fail` method (around line 252):

```rust
    /// Mark session as interrupted (client disconnected / user cancel).
    /// Sets outcome to `Interrupted` only on DB success; logs WAL `interrupted`
    /// event. `reason` is opaque string, e.g. "sink_closed" or "cancel_token".
    pub async fn interrupt(&mut self, reason: &str) {
        match crate::db::sessions::set_session_run_status(&self.db, self.session_id, "interrupted").await {
            Ok(()) => {
                self.outcome = SessionOutcome::Interrupted;
                let payload = serde_json::json!({ "reason": reason });
                if let Err(e) = crate::db::session_wal::log_event(
                    &self.db, self.session_id, "interrupted", Some(&payload)
                ).await {
                    tracing::warn!(session_id = %self.session_id, error = %e,
                        "failed to log WAL interrupted event");
                }
            }
            Err(e) => tracing::warn!(
                session_id = %self.session_id, error = %e, reason,
                "failed to mark session interrupted in DB"
            ),
        }
    }
```

Also extend the `SessionOutcome` enum in the same file to include `Interrupted`:

```rust
// Find the existing enum definition and add the new variant:
pub(crate) enum SessionOutcome {
    Running,
    Done,
    Failed,
    Interrupted, // NEW
}
```

- [ ] **Step 2: Write failing test for `interrupt`**

In the existing `#[cfg(test)] mod tests` block in `session_manager.rs` (or at the bottom of the file if none), add:

```rust
    #[sqlx::test(migrations = "../../../migrations")]
    async fn lifecycle_guard_interrupt_writes_wal(pool: PgPool) {
        let session_id = crate::db::sessions::get_or_create_session(
            &pool, "test-agent", None, None, None, false,
        ).await.unwrap();

        let mut guard = SessionLifecycleGuard::new(pool.clone(), session_id);
        guard.interrupt("sink_closed").await;

        let status: String = sqlx::query_scalar("SELECT run_status FROM sessions WHERE id = $1")
            .bind(session_id).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "interrupted");

        let event_type: String = sqlx::query_scalar(
            "SELECT event_type FROM session_events WHERE session_id = $1 ORDER BY created_at DESC LIMIT 1"
        ).bind(session_id).fetch_one(&pool).await.unwrap();
        assert_eq!(event_type, "interrupted");
    }
```

- [ ] **Step 3: Run the interrupt test**

Run: `cd crates/hydeclaw-core && cargo test --lib lifecycle_guard_interrupt_writes_wal -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Create pipeline/finalize.rs**

Create `crates/hydeclaw-core/src/agent/pipeline/finalize.rs`:

```rust
//! Single exit point for pipeline::execute — persists final/partial message,
//! transitions SessionLifecycleGuard, enqueues knowledge extraction.
//!
//! See docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md §4.

use crate::agent::pipeline::sink::{EventSink, PipelineEvent, SinkError};
use crate::agent::session_manager::{SessionLifecycleGuard, SessionManager};
use crate::agent::engine::stream::StreamEvent;
use hydeclaw_types::IncomingMessage;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug)]
pub enum FinalizeOutcome {
    Done { assistant_text: String, thinking_json: Option<String> },
    Failed { partial: String, reason: String },
    Interrupted { partial: String, reason: &'static str },
}

pub struct FinalizeContext<'a> {
    pub db: PgPool,
    pub session_id: Uuid,
    pub agent_name: String,
    pub message_count: usize,
    pub msg: &'a IncomingMessage,
    pub provider: Arc<dyn crate::agent::providers::LlmProvider>,
    pub memory_store: Arc<dyn crate::agent::memory_service::MemoryService>,
}

pub async fn finalize<S: EventSink>(
    ctx: FinalizeContext<'_>,
    outcome: FinalizeOutcome,
    sink: &mut S,
    lifecycle_guard: &mut SessionLifecycleGuard,
) -> anyhow::Result<String> {
    let sm = SessionManager::new(ctx.db.clone());
    let sender_agent_id = crate::agent::pipeline::execution::extract_sender_agent_id(&ctx.msg.user_id);

    match &outcome {
        FinalizeOutcome::Done { assistant_text, thinking_json } => {
            sm.save_message_ex(
                ctx.session_id, "assistant", assistant_text,
                None, None, sender_agent_id, thinking_json.clone(), None,
            ).await?;
            lifecycle_guard.done().await;
            crate::agent::pipeline::execution::spawn_knowledge_extraction(
                ctx.db.clone(), ctx.session_id, ctx.agent_name.clone(),
                ctx.provider.clone(), ctx.memory_store.clone(), ctx.message_count,
            );
            Ok(assistant_text.clone())
        }
        FinalizeOutcome::Failed { partial, reason } => {
            if !partial.is_empty() {
                let _ = sm.save_message_ex(
                    ctx.session_id, "assistant", partial,
                    None, None, sender_agent_id, None, None,
                ).await;
            }
            lifecycle_guard.fail(reason).await;
            // Best-effort Error event; sink may already be dead.
            let _ = sink.emit(PipelineEvent::Stream(StreamEvent::Error(reason.clone()))).await;
            Ok(partial.clone())
        }
        FinalizeOutcome::Interrupted { partial, reason } => {
            if !partial.is_empty() {
                let _ = sm.save_message_ex(
                    ctx.session_id, "assistant", partial,
                    None, None, sender_agent_id, None, None,
                ).await;
            }
            lifecycle_guard.interrupt(reason).await;
            // No sink.emit — sink is closed or cancel path, skip.
            Ok(partial.clone())
        }
    }
}
```

- [ ] **Step 5: Register the module**

In `crates/hydeclaw-core/src/agent/pipeline/mod.rs` append:

```rust
pub mod finalize;
```

- [ ] **Step 6: Replace the tail of `handle_streaming` to call `finalize`**

Open `crates/hydeclaw-core/src/agent/engine_execution.rs`. Find the tail of `handle_streaming` (around lines 593-608) that does `maybe_trim_session` + `lifecycle_guard.done()` + `spawn_knowledge_extraction` + `Ok(final_response)`. Replace with:

```rust
    self.maybe_trim_session(session_id).await;

    let fin_ctx = crate::agent::pipeline::finalize::FinalizeContext {
        db: self.cfg().db.clone(),
        session_id,
        agent_name: self.cfg().agent.name.clone(),
        message_count: messages.len(),
        msg,
        provider: self.cfg().provider.clone(),
        memory_store: self.cfg().memory_store.clone(),
    };
    // Minimal no-op sink for handle_streaming's tail; chunk_tx has already been consumed
    // by chat_stream. A true sink is introduced in Task 9.
    let mut noop = crate::agent::pipeline::sink::test_support::MockSink::new();
    let outcome = crate::agent::pipeline::finalize::FinalizeOutcome::Done {
        assistant_text: final_response.clone(),
        thinking_json: stream_thinking_json,
    };
    let out = crate::agent::pipeline::finalize::finalize(
        fin_ctx, outcome, &mut noop, &mut lifecycle_guard,
    ).await?;
    Ok(out)
```

Note the `test_support::MockSink` import is only accessible in non-cfg-test code if you make it non-test-cfg. To avoid that, create a tiny `pub(crate) struct NoopSink;` at the bottom of `pipeline/sink.rs` (outside `#[cfg(test)]`):

```rust
/// No-op sink used in intermediate refactor stages where finalize is called
/// after the stream is already consumed. Removed after Task 10.
pub(crate) struct NoopSink;

impl EventSink for NoopSink {
    async fn emit(&mut self, _ev: PipelineEvent) -> Result<(), SinkError> { Ok(()) }
}
```

And use `let mut noop = crate::agent::pipeline::sink::NoopSink;` in `handle_streaming` above.

- [ ] **Step 7: Do the same for `handle_with_status` tail**

Find the tail of `handle_with_status` in `engine_execution.rs` that currently does `lifecycle_guard.done()` + knowledge extraction. Replace with the same `finalize` call pattern as Step 6 (adjust `assistant_text` source — use whatever variable holds the final response at that point; read 10 lines before the tail to identify it).

- [ ] **Step 8: Do the same for `handle_sse` tail**

In `engine_sse.rs`, the `handle_sse` function has success tail around the end of the main loop and the command-early-exit path. For each success exit:
- Success (Done) → call `finalize(... , FinalizeOutcome::Done { ... }, ..)`.
- Failure (Failed) → call `finalize(... , FinalizeOutcome::Failed { partial, reason }, ..)`.

Replace inline `lifecycle_guard.done().await` / `lifecycle_guard.fail(...)` calls with the respective `FinalizeOutcome` + `finalize(...)`.

- [ ] **Step 9: Run snapshots**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots -- --nocapture`
Expected: all three PASS with the same shape. If anything diverges, fix the replacement to match the original behaviour.

- [ ] **Step 10: Run full tests and lint**

Run: `cd crates/hydeclaw-core && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add crates/hydeclaw-core/src/agent/session_manager.rs \
        crates/hydeclaw-core/src/agent/pipeline/finalize.rs \
        crates/hydeclaw-core/src/agent/pipeline/sink.rs \
        crates/hydeclaw-core/src/agent/pipeline/mod.rs \
        crates/hydeclaw-core/src/agent/engine_execution.rs \
        crates/hydeclaw-core/src/agent/engine_sse.rs
git commit -m "refactor(pipeline): extract finalize() and add SessionLifecycleGuard::interrupt"
```

---

## Task 5: Extract bootstrap() — session entry, ProcessingGuard, slash commands

**Files:**
- Create: `crates/hydeclaw-core/src/agent/pipeline/bootstrap.rs`
- Modify: `crates/hydeclaw-core/src/agent/pipeline/mod.rs` (`pub mod bootstrap;`)
- Modify: `crates/hydeclaw-core/src/agent/engine_execution.rs` (replace head of `handle_with_status` and `handle_streaming`)
- Modify: `crates/hydeclaw-core/src/agent/engine_sse.rs` (replace head of `handle_sse`)

### Steps

- [ ] **Step 1: Define BootstrapOutcome in new module**

Create `crates/hydeclaw-core/src/agent/pipeline/bootstrap.rs`:

```rust
//! Session entry, user-message persist, ProcessingGuard, slash-command early exit.
//!
//! See docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md §3.

use crate::agent::engine::stream::{ProcessingGuard, ProcessingPhase, StreamEvent};
use crate::agent::pipeline::sink::{EventSink, PipelineEvent};
use crate::agent::session_manager::{SessionLifecycleGuard, SessionManager};
use crate::agent::tool_loop::LoopDetector;
use hydeclaw_types::IncomingMessage;
use sqlx::PgPool;
use uuid::Uuid;

pub struct BootstrapOutcome {
    pub session_id: Uuid,
    pub enriched_text: String,
    pub messages: Vec<crate::agent::providers::Message>,
    pub tools: Vec<crate::agent::providers::ToolDefinition>,
    pub loop_detector: LoopDetector,
    pub processing_guard: ProcessingGuard,
    pub lifecycle_guard: SessionLifecycleGuard,
    /// If `Some`, a slash command produced a text result and the pipeline
    /// must render it and go straight to finalize(Done) without calling the LLM.
    pub command_output: Option<String>,
}

pub struct BootstrapContext<'a> {
    pub msg: &'a IncomingMessage,
    pub resume_session_id: Option<Uuid>,
    pub force_new_session: bool,
    pub use_history: bool, // true for handle_with_status/handle_sse, true for streaming (same behaviour currently)
}

pub async fn bootstrap<S: EventSink>(
    engine: &crate::agent::engine::AgentEngine,
    ctx: BootstrapContext<'_>,
    sink: &mut S,
) -> anyhow::Result<BootstrapOutcome> {
    // 1. Build context (messages + tools + session_id)
    let crate::agent::context_builder::ContextSnapshot {
        session_id, mut messages, tools,
    } = engine.build_context(ctx.msg, ctx.use_history, ctx.resume_session_id, ctx.force_new_session).await?;

    // 2. Mark session running
    let sm = SessionManager::new(engine.cfg().db.clone());
    if let Err(e) = sm.set_run_status(session_id, "running").await {
        tracing::warn!(session_id = %session_id, error = %e, "set_run_status(running) failed");
    }
    crate::agent::pipeline::execution::log_wal_running_with_retry(&sm, session_id).await;

    // 3. Emit first Phase
    let _ = sink.emit(PipelineEvent::Phase(ProcessingPhase::Thinking)).await;

    // 4. Lifecycle guard (WAL done/failed via RAII)
    let lifecycle_guard = SessionLifecycleGuard::new(engine.cfg().db.clone(), session_id);

    // 5. ProcessingGuard (typing indicator via ui_event_tx broadcast)
    let start_event = serde_json::json!({
        "type": "agent_processing",
        "agent": engine.cfg().agent.name,
        "session_id": session_id.to_string(),
    });
    let processing_guard = ProcessingGuard::new(
        engine.state().ui_event_tx.clone(),
        engine.state().processing_tracker.clone(),
        engine.cfg().agent.name.clone(),
        start_event,
    );

    // 6. Enrich + save user message
    let user_text = ctx.msg.text.clone().unwrap_or_default();
    let enriched_text = crate::agent::pipeline::execution::enrich_message_text(&user_text, ctx.msg);
    let sender_agent_id = crate::agent::pipeline::execution::extract_sender_agent_id(&ctx.msg.user_id);
    sm.save_message_ex(session_id, "user", &enriched_text, None, None, sender_agent_id, None, None).await?;

    // 7. LoopDetector
    let loop_detector = LoopDetector::new(&engine.cfg().config.tool_loop);

    // 8. Slash-command early exit
    //
    // NOTE (spec §11.1): handle_command currently returns Option<Result<String>>.
    // All commands produce plain text. If this ever changes, extend CommandOutput
    // and update render here — no sink/pipeline changes needed.
    let command_output = match engine.handle_command(&user_text, ctx.msg).await {
        Some(result) => Some(result?),
        None => None,
    };

    // Push user message into the working vec (for LLM call downstream)
    messages.push(crate::agent::providers::Message {
        role: crate::agent::providers::MessageRole::User,
        content: user_text,
        tool_calls: None,
        tool_call_id: None,
        thinking_blocks: vec![],
    });

    Ok(BootstrapOutcome {
        session_id, enriched_text, messages, tools,
        loop_detector, processing_guard, lifecycle_guard, command_output,
    })
}
```

- [ ] **Step 2: Register module**

Add to `pipeline/mod.rs`:

```rust
pub mod bootstrap;
```

- [ ] **Step 3: Run `cargo check`**

Run: `cd crates/hydeclaw-core && cargo check`
Expected: compiles. If the exact shape of `context_builder::ContextSnapshot` or `engine.state()` field names differs, fix imports — do not redesign.

- [ ] **Step 4: Replace the head of `handle_with_status` to call `bootstrap`**

In `engine_execution.rs`, replace the block from the start of `handle_with_status` up to (but not including) the main tool-loop with:

```rust
    self.cfg().approval_manager.prune_stale().await;
    let cancel_guard = Some(self.state.register_request());

    // Hook: BeforeMessage (kept at call site — its error is before lifecycle begins)
    if let crate::agent::hooks::HookAction::Block(reason) = self.hooks()
        .fire(&crate::agent::hooks::HookEvent::BeforeMessage)
    {
        if let Some((ref id, _)) = cancel_guard { self.state.unregister_request(id); }
        anyhow::bail!("blocked by hook: {}", reason);
    }

    // Build sink adapter over status_tx + chunk_tx for this path
    let mut sink = crate::agent::pipeline::sink::ChannelStatusSink::new(status_tx, chunk_tx);

    let bootstrap_ctx = crate::agent::pipeline::bootstrap::BootstrapContext {
        msg, resume_session_id: None, force_new_session: false, use_history: true,
    };
    let bootstrap_outcome = crate::agent::pipeline::bootstrap::bootstrap(self, bootstrap_ctx, &mut sink).await?;

    // Slash-command early exit (still inside this function — full execute extraction is Task 6)
    if let Some(text) = bootstrap_outcome.command_output {
        let _ = sink.emit(crate::agent::pipeline::sink::PipelineEvent::Stream(
            crate::agent::engine::stream::StreamEvent::TextDelta(text.clone())
        )).await;
        // Use finalize Done path for consistency
        let fin_ctx = crate::agent::pipeline::finalize::FinalizeContext {
            db: self.cfg().db.clone(),
            session_id: bootstrap_outcome.session_id,
            agent_name: self.cfg().agent.name.clone(),
            message_count: bootstrap_outcome.messages.len(),
            msg,
            provider: self.cfg().provider.clone(),
            memory_store: self.cfg().memory_store.clone(),
        };
        let mut lifecycle_guard = bootstrap_outcome.lifecycle_guard;
        let out = crate::agent::pipeline::finalize::finalize(
            fin_ctx,
            crate::agent::pipeline::finalize::FinalizeOutcome::Done {
                assistant_text: text, thinking_json: None,
            },
            &mut sink, &mut lifecycle_guard,
        ).await?;
        if let Some((ref id, _)) = cancel_guard { self.state.unregister_request(id); }
        return Ok(out);
    }

    // Continue with the existing tool-loop body using bootstrap_outcome.{messages, tools, session_id, loop_detector}
    let session_id = bootstrap_outcome.session_id;
    let mut messages = bootstrap_outcome.messages;
    let available_tools = bootstrap_outcome.tools;
    let mut loop_detector = bootstrap_outcome.loop_detector;
    let _processing_guard = bootstrap_outcome.processing_guard;
    let mut lifecycle_guard = bootstrap_outcome.lifecycle_guard;
    // ... rest of the existing main loop unchanged until Task 6 ...
```

The existing main-loop body (reading from the old code: tool invocation, LLM call, iteration) stays, unchanged, until Task 6. The only thing removed is the duplicated initialization. Delete the replaced lines cleanly.

- [ ] **Step 5: Do the same for `handle_streaming`**

Similar replacement — but `handle_streaming` uses `build_context(..., false, ...)` and has no hook check. Use `BootstrapContext { use_history: false, ... }`. Sink: `ChunkSink::new(chunk_tx)`.

- [ ] **Step 6: Do the same for `handle_sse`**

In `engine_sse.rs`, replace the head. Sink: `SseSink::new(event_tx.clone())`. `BootstrapContext { resume_session_id, force_new_session, use_history: true }`. Slash-command block in handle_sse already emits `MessageStart → TextDelta → Finish` explicitly — keep that shape in the early-exit block here to satisfy snapshot.

Specifically, in handle_sse's slash-command early exit, after `bootstrap`, emit:

```rust
let msg_id_str = format!("msg_{}", Uuid::new_v4());
let _ = sink.emit(PipelineEvent::Stream(StreamEvent::MessageStart { message_id: msg_id_str })).await;
let _ = sink.emit(PipelineEvent::Stream(StreamEvent::TextDelta(text.clone()))).await;
let _ = sink.emit(PipelineEvent::Stream(StreamEvent::Finish {
    finish_reason: "command".into(), continuation: false,
})).await;
// then finalize Done, return Ok(session_id)
```

- [ ] **Step 7: Run snapshots + lints**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots && cargo clippy --all-targets -- -D warnings`
Expected: all three snapshots PASS, no clippy warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/hydeclaw-core/src/agent/pipeline/bootstrap.rs \
        crates/hydeclaw-core/src/agent/pipeline/mod.rs \
        crates/hydeclaw-core/src/agent/engine_execution.rs \
        crates/hydeclaw-core/src/agent/engine_sse.rs
git commit -m "refactor(pipeline): extract bootstrap() and move ProcessingGuard/commands into it"
```

---

## Task 6: Extract execute() — main LLM+tools loop

**Files:**
- Create: `crates/hydeclaw-core/src/agent/pipeline/execute.rs`
- Modify: `crates/hydeclaw-core/src/agent/pipeline/mod.rs` (`pub mod execute;`)
- Modify: `crates/hydeclaw-core/src/agent/engine_execution.rs` (delegate main loop to `pipeline::execute`)
- Modify: `crates/hydeclaw-core/src/agent/engine_sse.rs` (delegate main loop to `pipeline::execute`)

### Steps

- [ ] **Step 1: Create execute.rs with signature and placeholder body**

```rust
//! Main LLM+tools loop. Transport-agnostic via EventSink.
//!
//! See docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md §3, §5.

use crate::agent::pipeline::bootstrap::BootstrapOutcome;
use crate::agent::pipeline::sink::{EventSink, PipelineEvent, SinkError};
use crate::agent::engine::stream::StreamEvent;
use crate::agent::providers::{Message, ToolDefinition};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub struct ExecuteOutcome {
    pub status: ExecuteStatus,
    pub session_id: Uuid,
    pub final_text: String,
    pub thinking_json: Option<String>,
    pub messages_len_at_end: usize,
}

pub enum ExecuteStatus {
    Done,
    Failed(String),
    Interrupted(&'static str),
}

pub async fn execute<S: EventSink>(
    engine: &crate::agent::engine::AgentEngine,
    bootstrap_outcome: BootstrapOutcome,
    sink: &mut S,
    cancel: CancellationToken,
) -> anyhow::Result<ExecuteOutcome> {
    // The body of this function is the merged main loop currently duplicated
    // in engine_execution.rs::handle_with_status and engine_sse.rs::handle_sse.
    //
    // Porting strategy:
    //   1. Start from engine_sse.rs's loop (it has the richer event emission).
    //   2. Replace every direct `event_tx.send*(StreamEvent::X)` with
    //      `sink.emit(PipelineEvent::Stream(StreamEvent::X)).await` and
    //      handle SinkError as per spec §4.
    //   3. Replace typing-status calls with sink.emit(PipelineEvent::Phase(...)).
    //   4. Check `cancel.is_cancelled()` at top of loop and inside stream consumption.
    //   5. Track `partial: String` locally; accumulate TextDelta content.
    //   6. On LLM response without tool_calls → ExecuteStatus::Done.
    //   7. On provider exhausted error → ExecuteStatus::Failed(reason).
    //   8. On SinkError::Closed or cancel → ExecuteStatus::Interrupted(reason_str).
    //   9. Do NOT persist here — finalize does it.

    let BootstrapOutcome { session_id, mut messages, tools, mut loop_detector,
        processing_guard: _pg, lifecycle_guard: _lg, .. } = bootstrap_outcome;

    let mut partial = String::new();
    let mut thinking_json: Option<String> = None;

    let max_turns = engine.cfg().config.limits.max_agent_turns.max(1);
    for _turn in 0..max_turns {
        if cancel.is_cancelled() {
            return Ok(ExecuteOutcome {
                status: ExecuteStatus::Interrupted("cancel_token"),
                session_id, final_text: partial, thinking_json,
                messages_len_at_end: messages.len(),
            });
        }

        // ... PORTED loop body ...
        // See source lines engine_sse.rs:~200-700 for the merged implementation.
        // The key operations, in order:
        //   - sink.emit(MessageStart { message_id })
        //   - call chat_stream_with_transient_retry(provider, &mut messages, &tools, internal_chunk_tx, compactor)
        //   - for each chunk emitted by provider: sink.emit(TextDelta), accumulate partial
        //   - parse tool_calls from response; break outer if empty with sink.emit(Finish)
        //   - loop_detector.check_limits(...) → on break: ExecuteStatus::Failed("loop_detector: <name>")
        //   - for call in tool_calls: sink.emit(ToolCallStart/Args/Result), execute via existing
        //     ToolExecutor path, loop_detector.record_execution
        //   - continue loop

        // Implementer note: because this is a pure move, the unit-test coverage is the
        // snapshot suite from Task 1. Use those snapshots as a red-bar.
        todo!("move loop body from engine_sse.rs into this function in one commit; see spec §5");
    }

    // Reached turn limit
    let _ = sink.emit(PipelineEvent::Stream(StreamEvent::Finish {
        finish_reason: "turn_limit".into(), continuation: false,
    })).await;
    Ok(ExecuteOutcome {
        status: ExecuteStatus::Done,
        session_id, final_text: partial, thinking_json,
        messages_len_at_end: messages.len(),
    })
}
```

**Implementer note:** the `todo!` is intentional — the body is a mechanical port, not a design decision. The TDD anchor is the snapshot suite from Task 1.

- [ ] **Step 2: Port the main loop body from engine_sse.rs**

Open `engine_sse.rs` and read the main loop body (roughly from where the `MessageStart` is first emitted through to the final `Finish`). Copy-paste into `execute.rs` between the `for _turn` and the `todo!`, replacing direct `event_tx` calls with `sink.emit` as described in Step 1. Remove the `todo!`.

This is the largest mechanical change in the plan (~600 LOC). Do it in one sitting; do not split. Compile after — there will be many type/import fixups.

- [ ] **Step 3: Make engine_execution.rs::handle_with_status delegate to execute()**

Replace the rest of the body of `handle_with_status` (after `bootstrap` and command early-exit) with:

```rust
    let cancel = self.state.cancel_token_for_session(session_id);
    let exec_outcome = crate::agent::pipeline::execute::execute(
        self, bootstrap_outcome, &mut sink, cancel,
    ).await?;

    // Map ExecuteStatus → FinalizeOutcome → finalize
    let fin_ctx = crate::agent::pipeline::finalize::FinalizeContext { /* as in Task 5 */ };
    let fin_outcome = match exec_outcome.status {
        crate::agent::pipeline::execute::ExecuteStatus::Done =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Done {
                assistant_text: exec_outcome.final_text.clone(),
                thinking_json: exec_outcome.thinking_json.clone(),
            },
        crate::agent::pipeline::execute::ExecuteStatus::Failed(reason) =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Failed {
                partial: exec_outcome.final_text.clone(), reason,
            },
        crate::agent::pipeline::execute::ExecuteStatus::Interrupted(reason) =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Interrupted {
                partial: exec_outcome.final_text.clone(), reason,
            },
    };
    let mut lifecycle_guard = /* from bootstrap_outcome saved earlier */;
    let out = crate::agent::pipeline::finalize::finalize(
        fin_ctx, fin_outcome, &mut sink, &mut lifecycle_guard,
    ).await?;
    Ok(out)
```

(Note: in Task 5 we deconstructed `bootstrap_outcome` into locals; now we need `lifecycle_guard` to survive until finalize. Re-assemble so that we pass the whole `bootstrap_outcome` into `execute` and `execute` returns the lifecycle_guard in `ExecuteOutcome`. Adjust types accordingly — or keep lifecycle_guard as a separate local before calling execute, and pass a `&mut` to execute so it stays in the caller's scope.)

**Preferred shape:** keep `lifecycle_guard` in the caller's scope:

```rust
let session_id = bootstrap_outcome.session_id;
let mut lifecycle_guard = /* extract from bootstrap_outcome before calling execute */;
let messages_len_initial = bootstrap_outcome.messages.len();
let exec_outcome = pipeline::execute::execute(self, bootstrap_outcome.without_lifecycle(), &mut sink, cancel).await?;
// ... finalize using local lifecycle_guard
```

Add a helper `BootstrapOutcome::without_lifecycle(mut self) -> (SessionLifecycleGuard, BootstrapOutcome)` to `bootstrap.rs`:

```rust
impl BootstrapOutcome {
    pub fn take_lifecycle_guard(&mut self) -> SessionLifecycleGuard {
        // Option trick — wrap lifecycle_guard in Option so we can take it.
        // Or change field type to Option<SessionLifecycleGuard> from the start.
        unimplemented!("see Task 6 Step 3 — pick one approach")
    }
}
```

The cleanest shape: **change `BootstrapOutcome::lifecycle_guard` to `Option<SessionLifecycleGuard>`** from the start in Task 5, so it can be `.take()`-en in the caller. Edit Task 5 Step 1 accordingly if you find this easier — the plan is fine either way; pick one and stay consistent.

- [ ] **Step 4: Delegate `handle_sse` and `handle_streaming` the same way**

Apply identical pattern in `engine_sse.rs` and `engine_execution.rs::handle_streaming`. For `handle_streaming`, `execute_outcome.final_text` maps to the returned `String`.

- [ ] **Step 5: Snapshots + full test run + clippy**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green. If a snapshot diverges, compare events emitted in the ported loop against the original — common divergences are missing `ProcessingPhase::Composing` emission or misordered `Finish` events.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/agent/pipeline/execute.rs \
        crates/hydeclaw-core/src/agent/pipeline/bootstrap.rs \
        crates/hydeclaw-core/src/agent/pipeline/mod.rs \
        crates/hydeclaw-core/src/agent/engine_execution.rs \
        crates/hydeclaw-core/src/agent/engine_sse.rs
git commit -m "refactor(pipeline): extract execute() main loop into pipeline/execute.rs"
```

---

## Task 7: handle_sse becomes a thin adapter

**Purpose:** With bootstrap/execute/finalize in place, `handle_sse` now has no unique logic — just sink construction and delegation. Reduces `engine_sse.rs` drastically.

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine_sse.rs`

### Steps

- [ ] **Step 1: Rewrite `handle_sse` as adapter**

Replace the whole body of `handle_sse` in `engine_sse.rs` with:

```rust
pub async fn handle_sse(
    &self,
    msg: &IncomingMessage,
    event_tx: crate::agent::engine_event_sender::EngineEventSender,
    resume_session_id: Option<Uuid>,
    force_new_session: bool,
) -> Result<Uuid> {
    if let crate::agent::hooks::HookAction::Block(reason) = self.hooks()
        .fire(&crate::agent::hooks::HookEvent::BeforeMessage)
    {
        anyhow::bail!("blocked by hook: {}", reason);
    }

    let _cancel_guard = self.state.register_request();

    let mut sink = crate::agent::pipeline::sink::SseSink::new(event_tx);

    let boot_ctx = crate::agent::pipeline::bootstrap::BootstrapContext {
        msg, resume_session_id, force_new_session, use_history: true,
    };
    let mut bootstrap_outcome = crate::agent::pipeline::bootstrap::bootstrap(self, boot_ctx, &mut sink).await?;
    let session_id = bootstrap_outcome.session_id;
    let mut lifecycle_guard = bootstrap_outcome.lifecycle_guard.take().expect("bootstrap sets lifecycle_guard");

    // Slash-command early exit (renders in SSE shape)
    if let Some(text) = bootstrap_outcome.command_output.take() {
        use crate::agent::pipeline::sink::PipelineEvent;
        use crate::agent::engine::stream::StreamEvent;
        let msg_id = format!("msg_{}", Uuid::new_v4());
        let _ = sink.emit(PipelineEvent::Stream(StreamEvent::MessageStart { message_id: msg_id })).await;
        let _ = sink.emit(PipelineEvent::Stream(StreamEvent::TextDelta(text.clone()))).await;
        let _ = sink.emit(PipelineEvent::Stream(StreamEvent::Finish { finish_reason: "command".into(), continuation: false })).await;

        let fin_ctx = crate::agent::pipeline::finalize::FinalizeContext {
            db: self.cfg().db.clone(), session_id,
            agent_name: self.cfg().agent.name.clone(),
            message_count: bootstrap_outcome.messages.len(),
            msg,
            provider: self.cfg().provider.clone(),
            memory_store: self.cfg().memory_store.clone(),
        };
        let _ = crate::agent::pipeline::finalize::finalize(
            fin_ctx,
            crate::agent::pipeline::finalize::FinalizeOutcome::Done { assistant_text: text, thinking_json: None },
            &mut sink, &mut lifecycle_guard,
        ).await?;
        return Ok(session_id);
    }

    let cancel = self.state.cancel_token_for_session(session_id);
    let exec_outcome = crate::agent::pipeline::execute::execute(self, bootstrap_outcome, &mut sink, cancel).await?;

    let fin_ctx = crate::agent::pipeline::finalize::FinalizeContext {
        db: self.cfg().db.clone(), session_id,
        agent_name: self.cfg().agent.name.clone(),
        message_count: exec_outcome.messages_len_at_end,
        msg,
        provider: self.cfg().provider.clone(),
        memory_store: self.cfg().memory_store.clone(),
    };
    let fin_outcome = match exec_outcome.status {
        crate::agent::pipeline::execute::ExecuteStatus::Done =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Done {
                assistant_text: exec_outcome.final_text, thinking_json: exec_outcome.thinking_json,
            },
        crate::agent::pipeline::execute::ExecuteStatus::Failed(reason) =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Failed {
                partial: exec_outcome.final_text, reason,
            },
        crate::agent::pipeline::execute::ExecuteStatus::Interrupted(reason) =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Interrupted {
                partial: exec_outcome.final_text, reason,
            },
    };
    let _ = crate::agent::pipeline::finalize::finalize(fin_ctx, fin_outcome, &mut sink, &mut lifecycle_guard).await?;
    Ok(session_id)
}
```

If `state.cancel_token_for_session` does not exist, replace with `CancellationToken::new()` (best-effort; the previous code did not propagate cancel either). The TODO is: add `cancel_token_for_session` in a follow-up if we want real per-session cancel; not in scope of this PR.

- [ ] **Step 2: Delete all remaining helpers in engine_sse.rs that are no longer called**

Audit what's left in `engine_sse.rs`. Expected: only `handle_sse`. If `persist_partial_if_any` still exists, it was already replaced by `finalize` — delete it.

- [ ] **Step 3: Snapshots + full tests + clippy**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine_sse.rs
git commit -m "refactor(engine): handle_sse becomes a thin SseSink adapter"
```

---

## Task 8: handle_with_status becomes a thin adapter

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine_execution.rs`

### Steps

- [ ] **Step 1: Rewrite `handle_with_status` as adapter**

Replace the body of `handle_with_status` with the same shape as Task 7 Step 1, but using `ChannelStatusSink`:

```rust
pub async fn handle_with_status(
    &self,
    msg: &IncomingMessage,
    status_tx: Option<tokio::sync::mpsc::UnboundedSender<ProcessingPhase>>,
    chunk_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> Result<String> {
    self.cfg().approval_manager.prune_stale().await;

    if let crate::agent::hooks::HookAction::Block(reason) = self.hooks()
        .fire(&crate::agent::hooks::HookEvent::BeforeMessage)
    {
        anyhow::bail!("blocked by hook: {}", reason);
    }

    let _cancel_guard = self.state.register_request();

    let mut sink = crate::agent::pipeline::sink::ChannelStatusSink::new(status_tx, chunk_tx);

    let boot_ctx = crate::agent::pipeline::bootstrap::BootstrapContext {
        msg, resume_session_id: None, force_new_session: false, use_history: true,
    };
    let mut bootstrap_outcome = crate::agent::pipeline::bootstrap::bootstrap(self, boot_ctx, &mut sink).await?;
    let session_id = bootstrap_outcome.session_id;
    let mut lifecycle_guard = bootstrap_outcome.lifecycle_guard.take().expect("bootstrap sets lifecycle_guard");

    if let Some(text) = bootstrap_outcome.command_output.take() {
        let _ = sink.emit(crate::agent::pipeline::sink::PipelineEvent::Stream(
            crate::agent::engine::stream::StreamEvent::TextDelta(text.clone())
        )).await;
        let fin_ctx = crate::agent::pipeline::finalize::FinalizeContext {
            db: self.cfg().db.clone(), session_id,
            agent_name: self.cfg().agent.name.clone(),
            message_count: bootstrap_outcome.messages.len(),
            msg,
            provider: self.cfg().provider.clone(),
            memory_store: self.cfg().memory_store.clone(),
        };
        return crate::agent::pipeline::finalize::finalize(
            fin_ctx,
            crate::agent::pipeline::finalize::FinalizeOutcome::Done { assistant_text: text, thinking_json: None },
            &mut sink, &mut lifecycle_guard,
        ).await;
    }

    let cancel = tokio_util::sync::CancellationToken::new();
    let exec_outcome = crate::agent::pipeline::execute::execute(self, bootstrap_outcome, &mut sink, cancel).await?;

    let fin_ctx = crate::agent::pipeline::finalize::FinalizeContext {
        db: self.cfg().db.clone(), session_id,
        agent_name: self.cfg().agent.name.clone(),
        message_count: exec_outcome.messages_len_at_end,
        msg,
        provider: self.cfg().provider.clone(),
        memory_store: self.cfg().memory_store.clone(),
    };
    let fin_outcome = match exec_outcome.status {
        crate::agent::pipeline::execute::ExecuteStatus::Done =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Done {
                assistant_text: exec_outcome.final_text.clone(), thinking_json: exec_outcome.thinking_json,
            },
        crate::agent::pipeline::execute::ExecuteStatus::Failed(reason) =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Failed { partial: exec_outcome.final_text.clone(), reason },
        crate::agent::pipeline::execute::ExecuteStatus::Interrupted(reason) =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Interrupted { partial: exec_outcome.final_text.clone(), reason },
    };
    crate::agent::pipeline::finalize::finalize(fin_ctx, fin_outcome, &mut sink, &mut lifecycle_guard).await
}
```

- [ ] **Step 2: Run snapshots + tests + clippy**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine_execution.rs
git commit -m "refactor(engine): handle_with_status becomes a thin ChannelStatusSink adapter"
```

---

## Task 9: handle_streaming becomes a thin adapter

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine_execution.rs`

### Steps

- [ ] **Step 1: Rewrite `handle_streaming`**

Replace the body of `handle_streaming` with:

```rust
pub async fn handle_streaming(
    &self,
    msg: &IncomingMessage,
    chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<String> {
    let mut sink = crate::agent::pipeline::sink::ChunkSink::new(chunk_tx);

    let boot_ctx = crate::agent::pipeline::bootstrap::BootstrapContext {
        msg, resume_session_id: None, force_new_session: false, use_history: false,
    };
    let mut bootstrap_outcome = crate::agent::pipeline::bootstrap::bootstrap(self, boot_ctx, &mut sink).await?;
    let session_id = bootstrap_outcome.session_id;
    let mut lifecycle_guard = bootstrap_outcome.lifecycle_guard.take().expect("bootstrap sets lifecycle_guard");

    // handle_streaming does not support slash commands historically — if a command
    // slipped through, render as plain text (chunks only).
    if let Some(text) = bootstrap_outcome.command_output.take() {
        let _ = sink.emit(crate::agent::pipeline::sink::PipelineEvent::Stream(
            crate::agent::engine::stream::StreamEvent::TextDelta(text.clone())
        )).await;
        let fin_ctx = crate::agent::pipeline::finalize::FinalizeContext {
            db: self.cfg().db.clone(), session_id,
            agent_name: self.cfg().agent.name.clone(),
            message_count: bootstrap_outcome.messages.len(),
            msg, provider: self.cfg().provider.clone(), memory_store: self.cfg().memory_store.clone(),
        };
        return crate::agent::pipeline::finalize::finalize(
            fin_ctx,
            crate::agent::pipeline::finalize::FinalizeOutcome::Done { assistant_text: text, thinking_json: None },
            &mut sink, &mut lifecycle_guard,
        ).await;
    }

    let cancel = tokio_util::sync::CancellationToken::new();
    let exec_outcome = crate::agent::pipeline::execute::execute(self, bootstrap_outcome, &mut sink, cancel).await?;

    let fin_ctx = crate::agent::pipeline::finalize::FinalizeContext {
        db: self.cfg().db.clone(), session_id,
        agent_name: self.cfg().agent.name.clone(),
        message_count: exec_outcome.messages_len_at_end,
        msg, provider: self.cfg().provider.clone(), memory_store: self.cfg().memory_store.clone(),
    };
    let fin_outcome = match exec_outcome.status {
        crate::agent::pipeline::execute::ExecuteStatus::Done =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Done {
                assistant_text: exec_outcome.final_text.clone(), thinking_json: exec_outcome.thinking_json,
            },
        crate::agent::pipeline::execute::ExecuteStatus::Failed(reason) =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Failed { partial: exec_outcome.final_text.clone(), reason },
        crate::agent::pipeline::execute::ExecuteStatus::Interrupted(reason) =>
            crate::agent::pipeline::finalize::FinalizeOutcome::Interrupted { partial: exec_outcome.final_text.clone(), reason },
    };
    crate::agent::pipeline::finalize::finalize(fin_ctx, fin_outcome, &mut sink, &mut lifecycle_guard).await
}
```

- [ ] **Step 2: Run snapshots + tests + clippy**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine_execution.rs
git commit -m "refactor(engine): handle_streaming becomes a thin ChunkSink adapter"
```

---

## Task 10: Delete old files, move wrappers to engine/run.rs

**Files:**
- Create: `crates/hydeclaw-core/src/agent/engine/run.rs`
- Delete: `crates/hydeclaw-core/src/agent/engine_execution.rs`
- Delete: `crates/hydeclaw-core/src/agent/engine_sse.rs`
- Delete: `crates/hydeclaw-core/src/agent/pipeline/execution.rs` (absorbed; migrate `extract_sender_agent_id` and `enrich_message_text` to `pipeline/bootstrap.rs`)
- Delete: `crates/hydeclaw-core/src/agent/pipeline/entry.rs` (`extract_tool_result_events` is needed inside execute; migrate as private helper there)
- Modify: `crates/hydeclaw-core/src/agent/mod.rs` (remove deleted modules)
- Modify: `crates/hydeclaw-core/src/agent/engine/mod.rs` (add `pub mod run;`)
- Modify: `crates/hydeclaw-core/src/agent/pipeline/mod.rs` (remove `execution` and `entry` submodules)

### Steps

- [ ] **Step 1: Verify no external module imports from files to be deleted**

Run: `grep -rn 'engine_execution\|engine_sse\|pipeline::execution\|pipeline::entry' crates/hydeclaw-core/src --include='*.rs'`
Expected: matches only inside the files being deleted or inside files being modified in this task. If any other file imports from these paths, migrate the import first.

- [ ] **Step 2: Migrate helper functions**

- `extract_sender_agent_id` and `enrich_message_text` → move to top of `pipeline/bootstrap.rs` as `pub(crate) fn`. Update import sites (bootstrap internal, finalize).
- `spawn_knowledge_extraction` → move to `pipeline/finalize.rs` as `pub(crate) fn`. Update callers.
- `log_wal_running_with_retry` → move to `pipeline/bootstrap.rs` as `pub(crate) fn`. Update callers.
- `extract_tool_result_events` and `ToolResultParts` → move to `pipeline/execute.rs` as private `fn`. Used only there.

- [ ] **Step 3: Create engine/run.rs with the three adapter methods**

Move the `handle_sse`, `handle_with_status`, `handle_streaming` method bodies (now thin adapters from Tasks 7–9) from `engine_sse.rs` / `engine_execution.rs` into a single `impl AgentEngine` block in `crates/hydeclaw-core/src/agent/engine/run.rs`:

```rust
//! Three thin adapter methods on AgentEngine: handle_sse, handle_with_status,
//! handle_streaming. Each constructs an EventSink and delegates to pipeline::execute.

use anyhow::Result;
use hydeclaw_types::IncomingMessage;
use uuid::Uuid;

use crate::agent::engine::stream::ProcessingPhase;
use crate::agent::engine::AgentEngine;
use crate::agent::engine_event_sender::EngineEventSender;

impl AgentEngine {
    pub async fn handle_sse(
        &self,
        msg: &IncomingMessage,
        event_tx: EngineEventSender,
        resume_session_id: Option<Uuid>,
        force_new_session: bool,
    ) -> Result<Uuid> {
        // ... body from Task 7 ...
    }

    pub async fn handle_with_status(
        &self,
        msg: &IncomingMessage,
        status_tx: Option<tokio::sync::mpsc::UnboundedSender<ProcessingPhase>>,
        chunk_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<String> {
        // ... body from Task 8 ...
    }

    pub async fn handle_streaming(
        &self,
        msg: &IncomingMessage,
        chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        // ... body from Task 9 ...
    }
}
```

- [ ] **Step 4: Delete old files**

```bash
git rm crates/hydeclaw-core/src/agent/engine_execution.rs \
       crates/hydeclaw-core/src/agent/engine_sse.rs \
       crates/hydeclaw-core/src/agent/pipeline/execution.rs \
       crates/hydeclaw-core/src/agent/pipeline/entry.rs
```

- [ ] **Step 5: Update module declarations**

In `crates/hydeclaw-core/src/agent/mod.rs`, remove:

```rust
pub mod engine_execution;   // DELETE
pub mod engine_sse;         // DELETE
```

In `crates/hydeclaw-core/src/agent/engine/mod.rs`, add:

```rust
pub mod run;
```

In `crates/hydeclaw-core/src/agent/pipeline/mod.rs`, remove:

```rust
pub mod execution; // DELETE
pub mod entry;     // DELETE
```

Also remove the temporary `NoopSink` in `pipeline/sink.rs` if it was added in Task 4 — it's no longer used.

- [ ] **Step 6: Build and test**

Run: `cd crates/hydeclaw-core && cargo check --all-targets`
Expected: compiles. Fix any missing imports — these are mechanical.

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 7: Measure net LOC change**

Run: `git diff --stat origin/master -- crates/hydeclaw-core/src/agent/`
Expected: net negative LOC (~-800 as per spec §6 budget). If meaningfully higher, check for leftover duplicated code.

- [ ] **Step 8: Commit**

```bash
git add -A crates/hydeclaw-core/src/agent/
git commit -m "chore(agent): delete engine_execution.rs, engine_sse.rs; consolidate to pipeline/"
```

---

## Task 11: Update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` — section "Agent Engine"

### Steps

- [ ] **Step 1: Replace the "Agent Engine" overview**

Open `CLAUDE.md`. Find the section starting with `### Agent Engine (src/agent/)`. Replace its content with:

```markdown
### Agent Engine (`src/agent/`)

Three entry points on `AgentEngine`, all thin adapters that construct an `EventSink` and delegate to `pipeline::execute`:

- `handle_sse` — web SSE via `SseSink` (over `EngineEventSender`/flume)
- `handle_with_status` — channel adapters (Telegram/Discord) with typing indicator via `ChannelStatusSink` (two `UnboundedSender` channels)
- `handle_streaming` — plain-chunk text via `ChunkSink`

The unified pipeline lives in `src/agent/pipeline/`:

- `sink.rs` — `EventSink` trait, `PipelineEvent`, `SinkError`, three production sinks
- `bootstrap.rs` — session entry, user-message persist, WAL `running`, `ProcessingGuard`, slash-command early exit
- `execute.rs` — main LLM+tools loop over `EventSink`
- `finalize.rs` — single exit point: persist assistant or partial, WAL `done|failed|interrupted` via `SessionLifecycleGuard`, enqueue knowledge extraction

**Key execution paths:**
- `pipeline::execute::execute()` — LLM call + tool loop, transport-agnostic
- `pipeline::handlers::*` — tool implementations (workspace_write, workspace_read, etc.)
- `workspace.rs::is_read_only()` — path protection

**Loop detection (`tool_loop.rs`):** Two-phase `LoopDetector`. See design spec at `docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md`.

**Session-scoped agents (`session_agent_pool.rs` + `engine_agent_tool.rs`):** unchanged.

**Agent config** (TOML at `config/agents/{name}.toml`): unchanged.
```

- [ ] **Step 2: Verify no broken references**

Run: `grep -n 'engine_execution\|engine_sse' CLAUDE.md`
Expected: no matches.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(claude-md): update Agent Engine section for pipeline unification"
```

---

## Final verification

After Task 11:

- [ ] `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots` — green
- [ ] `make check && make test && make lint` — green
- [ ] `git log --oneline origin/master..HEAD` shows 11 commits following the planned order
- [ ] `git diff --stat origin/master` net LOC is negative (~−800)

If all checks pass, the branch is ready for PR. Do not push without explicit user approval.

---

## Self-review notes

**Spec coverage:**
- §1 Problem statement — covered implicitly (the plan addresses the duplication).
- §2 Decision — implemented as Tasks 2 (trait + sink), 3 (three impls), 5 (bootstrap), 6 (execute), 4 (finalize).
- §3 Architecture — file structure matches.
- §4 Components — types defined in Tasks 2, 5, 6.
- §5 Data flow — flow implemented in Tasks 5+6+4 with exit paths to finalize.
- §6 Error handling — `SinkError` in Task 2, `ExecuteStatus` in Task 6, `FinalizeOutcome` in Task 4. `Interrupted` WAL event added via `SessionLifecycleGuard::interrupt` in Task 4.
- §7 Testing — Task 1 snapshots; per-sink unit tests in Task 3; bootstrap/execute coverage is indirect via snapshots (YAGNI explicit unit suite until a real regression justifies it).
- §8 Migration — Tasks 1→11 match the eleven-commit plan.
- §9 Non-goals — respected (no changes to providers, retry, LoopDetector, gateway handlers).
- §10 Open questions — none blocking.
- §11.1 Slash-command extension — marker comment in Task 5 Step 1.

**Placeholder scan:** the word `todo!` appears once in Task 6 Step 1 as a deliberate TDD anchor, explicitly labelled as "mechanical port, see spec §5" — resolved in Step 2 of the same task. No vague "add appropriate error handling" or "similar to Task N". All code blocks are concrete.

**Type consistency:** `EventSink`, `PipelineEvent`, `SinkError`, `BootstrapOutcome`, `ExecuteOutcome`, `FinalizeOutcome`, `FinalizeContext` — same names used across tasks. `SessionLifecycleGuard::interrupt` signature (`&mut self, reason: &str`) consistent in Task 4 and Task 10. `BootstrapOutcome.lifecycle_guard: Option<SessionLifecycleGuard>` decision made explicit in Task 6 Step 3 with edit-back instruction to Task 5 Step 1.

**Known implementation flexibility points (not placeholders, but decision points for the implementer):**
- Task 5 Step 1: `lifecycle_guard: SessionLifecycleGuard` vs `Option<SessionLifecycleGuard>` — choose `Option` per Task 6 guidance.
- Task 7 Step 1: `state.cancel_token_for_session` if absent, use fresh `CancellationToken::new()` — explicit fallback in the step.
- Task 10 Step 2: helper migration is explicit file-by-file.
