# Execution Pipeline Unification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify `handle_sse`, `handle_with_status`, `handle_streaming` into a single `pipeline::execute<S: EventSink>` with `bootstrap` / `execute` / `finalize` modules, keeping the public API of `AgentEngine` backwards-compatible.

**Architecture:** New `pipeline/sink.rs` defines `EventSink` trait over a `PipelineEvent` (wraps `StreamEvent` and `ProcessingPhase`). New `pipeline/{bootstrap,execute,finalize}.rs` host the three phases. Three thin adapter methods on `AgentEngine` construct a sink and delegate. Existing `SessionLifecycleGuard`, `ProcessingGuard`, `LoopDetector`, `chat_stream_with_transient_retry` are reused.

**Tech Stack:** Rust 2024, tokio, sqlx (PostgreSQL), flume, `tokio::sync::mpsc` (unbounded) and `tokio::sync::broadcast`, `tokio_util::sync::CancellationToken`, `async fn in trait`, `sqlx::test` macro, existing test support at `crates/hydeclaw-core/tests/support/mock_provider.rs`.

**Spec:** [docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md](../specs/2026-04-20-execution-pipeline-unification-design.md)

**Verified facts (from codebase at plan time, 2026-04-20):**
- `AgentState` at `src/agent/agent_state.rs:31-33` has `pub processing_tracker: Option<ProcessingTracker>` and `pub ui_event_tx: Option<tokio::sync::broadcast::Sender<String>>`.
- `AgentEngine::handle_command` and `AgentEngine::build_context` are `pub(super)` in `src/agent/engine/context_builder.rs:100, 169` — need visibility upgrade to `pub(crate)` to be callable from `pipeline/`.
- `ContextSnapshot { session_id, messages, tools }` at `src/agent/context_builder.rs:18-22`.
- `max_agent_turns` lives at `cfg().config.limits.max_agent_turns` (default 5), see `src/config/mod.rs:233-234`.
- **No** `cancel_token_for_session` method exists. The thin adapters use a fresh `CancellationToken::new()`; wiring real per-session cancel is **out of scope for this PR** and documented as future work.
- `SessionLifecycleGuard` at `src/agent/session_manager.rs:202-253` with methods `new`, `done`, `fail` and `SessionOutcome { Running, Done, Failed }`.
- `handle_command` signature: `pub(super) async fn handle_command(&self, text: &str, msg: &IncomingMessage) -> Option<Result<String>>`.

---

## File structure

**Created:**
- `crates/hydeclaw-core/src/agent/pipeline/sink.rs` — `EventSink` trait, `PipelineEvent`, `SinkError`, three production sinks + `MockSink` (cfg(test))
- `crates/hydeclaw-core/src/agent/pipeline/bootstrap.rs` — session entry, user-message persist, WAL `running`, `ProcessingGuard`, slash-command detection
- `crates/hydeclaw-core/src/agent/pipeline/execute.rs` — main LLM+tools loop over `EventSink`
- `crates/hydeclaw-core/src/agent/pipeline/finalize.rs` — single exit point: persist assistant or partial, WAL `done|failed|interrupted`, knowledge extraction
- `crates/hydeclaw-core/src/agent/engine/run.rs` — three thin `impl AgentEngine` adapter methods (moved from deleted files)
- `crates/hydeclaw-core/tests/pipeline_snapshots.rs` — regression snapshots
- `crates/hydeclaw-core/tests/support/pipeline_helpers.rs` — test fixture builder

**Modified:**
- `crates/hydeclaw-core/src/agent/pipeline/mod.rs` — add new submodules, later remove `execution`/`entry`
- `crates/hydeclaw-core/src/agent/engine/mod.rs` — add `pub mod run;`
- `crates/hydeclaw-core/src/agent/mod.rs` — remove deleted `engine_execution`/`engine_sse`
- `crates/hydeclaw-core/src/agent/session_manager.rs` — add `SessionLifecycleGuard::interrupt` + `SessionOutcome::Interrupted`
- `crates/hydeclaw-core/src/agent/engine/context_builder.rs` — raise `handle_command` and `build_context` visibility from `pub(super)` to `pub(crate)`
- `CLAUDE.md` — update `Agent Engine` section

**Deleted (Task 11):**
- `crates/hydeclaw-core/src/agent/engine_execution.rs`
- `crates/hydeclaw-core/src/agent/engine_sse.rs`
- `crates/hydeclaw-core/src/agent/pipeline/execution.rs` (helpers absorbed into bootstrap/finalize)
- `crates/hydeclaw-core/src/agent/pipeline/entry.rs` (absorbed into `pipeline/execute.rs` as private helper)

**Dependencies:** no new crates. `scopeguard` is NOT added — RAII is handled by existing `SessionLifecycleGuard` and `ProcessingGuard`.

---

## Porting strategy (for Tasks 6a/6b)

The main loop body is a mechanical port from `engine_sse.rs:~200-~900`. The transformation rules are:

| Current call | Ported call |
|---|---|
| `event_tx.send_async(StreamEvent::X).await` | `sink.emit(PipelineEvent::Stream(StreamEvent::X)).await` |
| `event_tx.send(StreamEvent::X)` | `sink.emit(PipelineEvent::Stream(StreamEvent::X)).await` |
| `status_tx.send(ProcessingPhase::X)` | `sink.emit(PipelineEvent::Phase(ProcessingPhase::X)).await` |
| direct `chunk_tx.send(text)` | emit `PipelineEvent::Stream(StreamEvent::TextDelta(text))` |
| raising error via `?` mid-loop | set `outcome = ExecuteStatus::Failed(reason)`, break outer |

Porting errors are caught by the snapshot tests from Task 1. If a snapshot fails, diff emitted-event sequence against `engine_sse.rs` source.

---

## Task 1: Skipped (architecture constraint)

**Original plan:** end-to-end regression snapshots via integration tests.

**Why skipped (discovered 2026-04-20 during execution):** `tests/support/mock_provider.rs` implements a local `MockLlmProvider` trait, not the real `LlmProvider` — the bridge is deliberately deferred to Phase 66 REF-01 ([lib.rs:14-18](crates/hydeclaw-core/src/lib.rs#L14)). Exposing `AgentEngine::new` via the test-facing lib facade would cascade 15+ modules into the 10-module `lib.rs` cap, which is a team-level architectural contract.

**Replacement strategy:**
1. **Unit tests live inside `src/agent/pipeline/*.rs #[cfg(test)]`** (binary-crate test modules have full access to private types). Tasks 4 / 6a / 6b use inline fake-types (`FakeLlmProvider`, `FakeMemoryService`) instead of `tests/support` fixtures.
2. **Manual smoke test** after Task 10 verifies integration end-to-end (new final Task 12, see below).

This keeps all new logic under `pipeline/` covered by unit tests, at the cost of not having automated end-to-end snapshots across the three entry points. Integration regression risk is mitigated by: (a) Task 10 is a file-move operation with `cargo check && cargo clippy`; (b) Tasks 7/8/9 adapters are each ~20 LOC with a clear delegate-to-pipeline shape.

**No files created or modified in this task.** Proceed to Task 2.

## Task 1 (original — retained for reference)

**Files:**
- Create: `crates/hydeclaw-core/tests/support/pipeline_helpers.rs`
- Create: `crates/hydeclaw-core/tests/pipeline_snapshots.rs`
- Modify: `crates/hydeclaw-core/tests/support/mod.rs` (register new helper module — create the file if missing)

### Steps

- [ ] **Step 1: Find the canonical AgentEngine construction for tests**

Run these two greps in parallel:

```bash
grep -rln 'AgentEngine::new\|Arc::new(AgentEngine' crates/hydeclaw-core/tests/ crates/hydeclaw-core/src/agent/
grep -rln 'fn build_engine\|fn make_engine\|fn test_engine' crates/hydeclaw-core/
```

Expected: at least one match — either in an existing integration test (e.g. `tests/integration_*.rs`) or a `#[cfg(test)]` helper inside `src/agent/engine/mod.rs` or `src/agent/engine/context_builder.rs`.

Open the shortest match. Note the exact construction sequence: which builder is called, how `db`, `provider`, `memory_store`, `agent_config`, `hooks`, `tool_loop_config` are assembled, and whether `Arc::new` wraps the result.

This is the canonical pattern — **your `build_test_engine` copies this verbatim**. Do not invent a new construction path.

- [ ] **Step 2: Create `tests/support/pipeline_helpers.rs`**

```rust
//! Shared helpers for pipeline integration and snapshot tests.
//!
//! `build_test_engine` is a verbatim copy of the canonical AgentEngine test
//! construction identified in Step 1. If that reference changes shape
//! (fields added/removed in AgentConfig, new required trait objects),
//! update this helper in lockstep.

use hydeclaw_core::agent::engine::{AgentEngine, StreamEvent};
use hydeclaw_core::agent::engine_event_sender::EngineEventSender;
use hydeclaw_types::IncomingMessage;
use sqlx::PgPool;
use std::sync::Arc;

use super::mock_provider::MockProvider;

pub async fn build_test_engine(db: PgPool, provider: MockProvider) -> Arc<AgentEngine> {
    // STEP 1 OUTPUT GOES HERE. The exact shape depends on the reference
    // construction found in Step 1 — typical skeleton:
    //
    //   let agent_config = hydeclaw_core::agent::agent_config::AgentConfig {
    //       agent: /* minimal AgentSettings with name = "test-agent" */,
    //       db: db.clone(),
    //       provider: Arc::new(provider) as Arc<dyn LlmProvider>,
    //       memory_store: Arc::new(/* NoOpMemoryStore or equivalent */),
    //       approval_manager: /* default */,
    //       config: /* default HydeClawConfig */,
    //   };
    //   let agent_state = hydeclaw_core::agent::agent_state::AgentState::new(
    //       /* empty processing_tracker, empty ui_event_tx */
    //   );
    //   let hooks = /* default HooksRegistry */;
    //   Arc::new(AgentEngine::new(agent_config, agent_state, hooks))
    //
    // The actual field names and method signatures come from Step 1's reference.
    // Keep the helper under 30 LOC.
    panic!("Task 1 Step 1: copy reference construction here. See the grep output.");
}

/// Drain an SSE receiver into a Vec until the sender drops.
pub async fn drain_sse(mut rx: tokio::sync::mpsc::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await { out.push(ev); }
    out
}

/// Reduce a StreamEvent sequence to a canonical tag vector for snapshot
/// comparison. Discards payload strings/UUIDs so unrelated refactors are stable.
pub fn shape(events: &[StreamEvent]) -> Vec<&'static str> {
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

/// Minimal message with just text.
pub fn user_msg(text: &str) -> IncomingMessage {
    IncomingMessage { text: Some(text.to_string()), ..Default::default() }
}
```

After copying the reference construction into `build_test_engine`, the `unreachable!` is gone.

- [ ] **Step 3: Register helper module**

Open or create `crates/hydeclaw-core/tests/support/mod.rs`:

```rust
pub mod mock_provider;
pub mod pipeline_helpers;
```

- [ ] **Step 4: Create snapshot test file**

Create `crates/hydeclaw-core/tests/pipeline_snapshots.rs`:

```rust
//! Regression snapshots for the three agent execution entry points.
//! MUST stay green throughout the pipeline unification refactor.

mod support;

use hydeclaw_core::agent::engine::StreamEvent;
use hydeclaw_core::agent::engine_event_sender::EngineEventSender;
use sqlx::PgPool;
use support::mock_provider::MockProvider;
use support::pipeline_helpers::{build_test_engine, drain_sse, shape, user_msg};

#[sqlx::test(migrations = "../../migrations")]
async fn sse_happy_path_snapshot(pool: PgPool) {
    let engine = build_test_engine(
        pool.clone(),
        MockProvider::new().expect_text("hello world", "stop"),
    ).await;

    let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(256);
    let event_tx = EngineEventSender::new(tx);
    let msg = user_msg("hi");

    let drain = tokio::spawn(drain_sse(rx));
    let session_id = engine.handle_sse(&msg, event_tx, None, false).await.unwrap();
    let events = drain.await.unwrap();

    assert!(!session_id.is_nil());
    let observed = shape(&events);
    // This assertion is the baseline. If it fails on first run, REPLACE the
    // expected vec with `observed` verbatim (baseline is lock-in, not a hypothesis).
    assert_eq!(observed, vec!["MessageStart", "TextDelta", "Finish"],
        "SSE snapshot shape (if this breaks on first run, update the baseline)");
}

#[sqlx::test(migrations = "../../migrations")]
async fn with_status_happy_path_snapshot(pool: PgPool) {
    let engine = build_test_engine(
        pool.clone(),
        MockProvider::new().expect_text("hello", "stop"),
    ).await;

    let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel();
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();

    let statuses = tokio::spawn(async move {
        let mut v = Vec::new();
        while let Some(p) = status_rx.recv().await { v.push(format!("{:?}", p)); }
        v
    });
    let chunks = tokio::spawn(async move {
        let mut v = Vec::new();
        while let Some(c) = chunk_rx.recv().await { v.push(c); }
        v
    });

    let result = engine.handle_with_status(&user_msg("hi"), Some(status_tx), Some(chunk_tx)).await.unwrap();
    let statuses = statuses.await.unwrap();
    let chunks = chunks.await.unwrap();

    assert_eq!(result, "hello");
    assert!(statuses.iter().any(|s| s.contains("Thinking")),
        "expected at least one Thinking phase (baseline)");
    assert_eq!(chunks.concat(), "hello",
        "chunks concatenate to the final text (baseline)");
}

#[sqlx::test(migrations = "../../migrations")]
async fn streaming_happy_path_snapshot(pool: PgPool) {
    let engine = build_test_engine(
        pool.clone(),
        MockProvider::new().expect_text("world", "stop"),
    ).await;

    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();
    let chunks = tokio::spawn(async move {
        let mut v = Vec::new();
        while let Some(c) = chunk_rx.recv().await { v.push(c); }
        v
    });

    let result = engine.handle_streaming(&user_msg("hi"), chunk_tx).await.unwrap();
    let chunks = chunks.await.unwrap();

    assert_eq!(result, "world");
    assert_eq!(chunks.concat(), "world");
}
```

- [ ] **Step 5: Run and set the baseline**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots -- --nocapture`

First run outcomes:
- If `build_test_engine` still has `unreachable!`, fix it with the reference pattern from Step 1 and re-run.
- If the SSE shape assertion fails, copy the `observed` printed by the panic into the expected vec in Step 4, re-run. This is the deliberate baseline-lock step described in the file comment.

Expected after fixup: three tests PASS. Record the exact shape in your own commit note.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/tests/pipeline_snapshots.rs \
        crates/hydeclaw-core/tests/support/
git commit -m "test(pipeline): regression snapshots for three entry points"
```

---

## Task 2: EventSink trait, PipelineEvent, MockSink

**Files:**
- Create: `crates/hydeclaw-core/src/agent/pipeline/sink.rs`
- Modify: `crates/hydeclaw-core/src/agent/pipeline/mod.rs`

### Steps

- [ ] **Step 1: Create sink.rs**

```rust
//! Transport-agnostic event sink for pipeline::execute.
//!
//! PipelineEvent = StreamEvent (web SSE events) | ProcessingPhase (channel typing).
//! Each sink chooses which variants to forward and silently drops the rest.

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
    async fn close(&mut self) -> Result<(), SinkError> { Ok(()) }
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    #[derive(Default, Debug)]
    pub struct MockSink {
        pub events: Vec<PipelineEvent>,
        pub closed_after: Option<usize>,
    }

    impl MockSink {
        pub fn new() -> Self { Self::default() }
        pub fn close_after(n: usize) -> Self { Self { closed_after: Some(n), ..Self::default() } }

        pub fn stream_shapes(&self) -> Vec<&'static str> {
            self.events.iter().filter_map(|e| match e {
                PipelineEvent::Stream(StreamEvent::MessageStart { .. }) => Some("MessageStart"),
                PipelineEvent::Stream(StreamEvent::TextDelta(_)) => Some("TextDelta"),
                PipelineEvent::Stream(StreamEvent::Finish { .. }) => Some("Finish"),
                PipelineEvent::Stream(StreamEvent::Error(_)) => Some("Error"),
                PipelineEvent::Stream(StreamEvent::ToolCallStart { .. }) => Some("ToolCallStart"),
                PipelineEvent::Stream(StreamEvent::ToolCallResult { .. }) => Some("ToolCallResult"),
                _ => None,
            }).collect()
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
        sink.emit(StreamEvent::TextDelta("a".into()).into()).await.unwrap();
        sink.emit(ProcessingPhase::Thinking.into()).await.unwrap();
        assert_eq!(sink.events.len(), 2);
    }

    #[tokio::test]
    async fn mock_sink_closes_after_limit() {
        let mut sink = MockSink::close_after(1);
        sink.emit(StreamEvent::TextDelta("ok".into()).into()).await.unwrap();
        let err = sink.emit(StreamEvent::TextDelta("drop".into()).into()).await;
        assert!(matches!(err, Err(SinkError::Closed)));
    }
}
```

- [ ] **Step 2: Register module in `pipeline/mod.rs`**

```rust
pub mod sink;
```

- [ ] **Step 3: Verify**

Run: `cd crates/hydeclaw-core && cargo test --lib agent::pipeline::sink && cargo clippy --lib -- -D warnings`
Expected: both mock_sink tests PASS, zero clippy warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/pipeline/sink.rs \
        crates/hydeclaw-core/src/agent/pipeline/mod.rs
git commit -m "feat(pipeline): add EventSink trait, PipelineEvent, MockSink"
```

---

## Task 3: Three production sinks

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/pipeline/sink.rs` (append)

### Steps

- [ ] **Step 1: Append production sinks BEFORE `#[cfg(test)]`**

```rust
// ── Production sinks ──────────────────────────────────────────────────

use crate::agent::engine_event_sender::EngineEventSender;

pub struct SseSink { tx: EngineEventSender }

impl SseSink {
    pub fn new(tx: EngineEventSender) -> Self { Self { tx } }
}

impl EventSink for SseSink {
    async fn emit(&mut self, ev: PipelineEvent) -> Result<(), SinkError> {
        match ev {
            PipelineEvent::Stream(se) => self.tx.send_async(se).await.map_err(|_| SinkError::Closed),
            PipelineEvent::Phase(_)   => Ok(()), // SSE does not transport typing indicator
        }
    }
}

pub struct ChannelStatusSink {
    status_tx: Option<tokio::sync::mpsc::UnboundedSender<ProcessingPhase>>,
    chunk_tx:  Option<tokio::sync::mpsc::UnboundedSender<String>>,
    pub buffer: String,
}

impl ChannelStatusSink {
    pub fn new(
        status_tx: Option<tokio::sync::mpsc::UnboundedSender<ProcessingPhase>>,
        chunk_tx:  Option<tokio::sync::mpsc::UnboundedSender<String>>,
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

- [ ] **Step 2: Append unit tests inside the existing `mod tests`**

```rust
    #[tokio::test]
    async fn sse_sink_forwards_stream_events() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamEvent>(8);
        let mut sink = SseSink::new(EngineEventSender::new(tx));
        sink.emit(StreamEvent::TextDelta("hi".into()).into()).await.unwrap();
        assert!(matches!(rx.recv().await, Some(StreamEvent::TextDelta(ref s)) if s == "hi"));
    }

    #[tokio::test]
    async fn sse_sink_drops_phase() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamEvent>(8);
        let mut sink = SseSink::new(EngineEventSender::new(tx));
        sink.emit(ProcessingPhase::Thinking.into()).await.unwrap();
        drop(sink);
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn sse_sink_returns_closed_on_drop() {
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(8);
        let mut sink = SseSink::new(EngineEventSender::new(tx));
        drop(rx);
        let err = sink.emit(StreamEvent::TextDelta("x".into()).into()).await;
        assert!(matches!(err, Err(SinkError::Closed)));
    }

    #[tokio::test]
    async fn channel_status_sink_routes_phase_to_status() {
        let (st, mut st_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ch, _ch_rx)    = tokio::sync::mpsc::unbounded_channel();
        let mut sink = ChannelStatusSink::new(Some(st), Some(ch));
        sink.emit(ProcessingPhase::Thinking.into()).await.unwrap();
        assert!(matches!(st_rx.recv().await, Some(ProcessingPhase::Thinking)));
    }

    #[tokio::test]
    async fn channel_status_sink_routes_text_to_chunks_and_buffers() {
        let (ch, mut ch_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut sink = ChannelStatusSink::new(None, Some(ch));
        sink.emit(StreamEvent::TextDelta("hello".into()).into()).await.unwrap();
        assert_eq!(ch_rx.recv().await, Some("hello".into()));
        assert_eq!(sink.buffer, "hello");
    }

    #[tokio::test]
    async fn channel_status_sink_drops_tool_events() {
        let (ch, mut ch_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut sink = ChannelStatusSink::new(None, Some(ch));
        sink.emit(StreamEvent::MessageStart { message_id: "m".into() }.into()).await.unwrap();
        drop(sink);
        assert!(ch_rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn chunk_sink_emits_only_text_deltas() {
        let (ch, mut ch_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut sink = ChunkSink::new(ch);
        sink.emit(StreamEvent::TextDelta("abc".into()).into()).await.unwrap();
        sink.emit(StreamEvent::MessageStart { message_id: "m".into() }.into()).await.unwrap();
        assert_eq!(ch_rx.recv().await, Some("abc".into()));
        drop(sink);
        assert!(ch_rx.recv().await.is_none());
    }
```

- [ ] **Step 3: Verify**

Run: `cd crates/hydeclaw-core && cargo test --lib agent::pipeline::sink && cargo clippy --lib -- -D warnings`
Expected: all nine tests (two mock + seven sink) PASS, zero warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/pipeline/sink.rs
git commit -m "feat(pipeline): SseSink, ChannelStatusSink, ChunkSink"
```

---

## Task 4: `SessionLifecycleGuard::interrupt` + `pipeline/finalize.rs`

**Purpose:** create `finalize` as an importable helper with unit tests via `MockSink`. We do **not** integrate it into `engine_*.rs` in this commit — integration happens in Task 5 (bootstrap) and Task 6 (execute) naturally. No `NoopSink` scaffolding.

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/session_manager.rs`
- Create: `crates/hydeclaw-core/src/agent/pipeline/finalize.rs`
- Modify: `crates/hydeclaw-core/src/agent/pipeline/mod.rs`

### Steps

- [ ] **Step 1: Extend `SessionOutcome` enum**

In `src/agent/session_manager.rs` find the `SessionOutcome` enum (near the `SessionLifecycleGuard` struct) and add the new variant:

```rust
pub(crate) enum SessionOutcome {
    Running,
    Done,
    Failed,
    Interrupted,
}
```

- [ ] **Step 2: Grep for existing matches on SessionOutcome**

Run: `grep -rn 'SessionOutcome::' crates/hydeclaw-core/src`
For each match that is a `match self.outcome` style expression, ensure it either handles `Interrupted` explicitly or falls into an acceptable `_` arm. Most matches should be in `Drop for SessionLifecycleGuard`.

The existing `Drop` impl (`session_manager.rs:256-`) tests `matches!(self.outcome, SessionOutcome::Running)` — that already skips `Interrupted` correctly. No change needed there.

- [ ] **Step 3: Add `interrupt` method**

Append to the `impl SessionLifecycleGuard` block (after `fail`):

```rust
    /// Mark session as interrupted (client disconnected / user cancel).
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

- [ ] **Step 4: Add a unit test for `interrupt`**

At the bottom of `session_manager.rs`, inside the existing `#[cfg(test)] mod tests` (or create one):

```rust
    #[sqlx::test(migrations = "../../migrations")]
    async fn lifecycle_guard_interrupt_writes_wal(pool: PgPool) {
        use crate::db::sessions::get_or_create_session;
        let session_id = get_or_create_session(&pool, "test-agent", None, None, None, false).await.unwrap();

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

Run: `cd crates/hydeclaw-core && cargo test --lib lifecycle_guard_interrupt_writes_wal -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Create `pipeline/finalize.rs`**

```rust
//! Single exit point for pipeline::execute — persists final/partial message,
//! transitions SessionLifecycleGuard, enqueues knowledge extraction.
//!
//! See docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md §4.

use crate::agent::memory_service::MemoryService;
use crate::agent::pipeline::sink::{EventSink, PipelineEvent, SinkError};
use crate::agent::providers::LlmProvider;
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
    pub provider: Arc<dyn LlmProvider>,
    pub memory_store: Arc<dyn MemoryService>,
}

pub(crate) fn extract_sender_agent_id(user_id: &Option<String>) -> Option<String> {
    // Absorbed from pipeline/execution.rs (Task 11 removes the source file).
    user_id.as_ref().and_then(|s| s.strip_prefix("agent:").map(|s| s.to_string()))
}

pub async fn finalize<S: EventSink>(
    ctx: FinalizeContext<'_>,
    outcome: FinalizeOutcome,
    sink: &mut S,
    lifecycle_guard: &mut SessionLifecycleGuard,
) -> anyhow::Result<String> {
    let sm = SessionManager::new(ctx.db.clone());
    let sender_agent_id = extract_sender_agent_id(&ctx.msg.user_id);

    let out = match &outcome {
        FinalizeOutcome::Done { assistant_text, thinking_json } => {
            sm.save_message_ex(
                ctx.session_id, "assistant", assistant_text,
                None, None, sender_agent_id, thinking_json.clone(), None,
            ).await?;
            lifecycle_guard.done().await;
            spawn_knowledge_extraction(
                ctx.db.clone(), ctx.session_id, ctx.agent_name.clone(),
                ctx.provider.clone(), ctx.memory_store.clone(), ctx.message_count,
            );
            assistant_text.clone()
        }
        FinalizeOutcome::Failed { partial, reason } => {
            if !partial.is_empty() {
                let _ = sm.save_message_ex(
                    ctx.session_id, "assistant", partial,
                    None, None, sender_agent_id, None, None,
                ).await;
            }
            lifecycle_guard.fail(reason).await;
            let _ = sink.emit(PipelineEvent::Stream(StreamEvent::Error(reason.clone()))).await;
            partial.clone()
        }
        FinalizeOutcome::Interrupted { partial, reason } => {
            if !partial.is_empty() {
                let _ = sm.save_message_ex(
                    ctx.session_id, "assistant", partial,
                    None, None, sender_agent_id, None, None,
                ).await;
            }
            lifecycle_guard.interrupt(reason).await;
            partial.clone()
        }
    };

    Ok(out)
}

/// Absorbed from pipeline/execution.rs. Spawn background job that builds
/// memory chunks from the completed session.
pub(crate) fn spawn_knowledge_extraction(
    db: PgPool,
    session_id: Uuid,
    agent_name: String,
    provider: Arc<dyn LlmProvider>,
    memory_store: Arc<dyn MemoryService>,
    message_count: usize,
) {
    // IMPLEMENTER: copy the body verbatim from the current
    // `pipeline::execution::spawn_knowledge_extraction`. It is already
    // a small helper (~15 LOC) that spawns a tokio task calling a
    // knowledge-extractor module. Do not change its semantics.
    let _ = (db, session_id, agent_name, provider, memory_store, message_count);
    crate::agent::pipeline::execution::spawn_knowledge_extraction(
        db, session_id, agent_name, provider, memory_store, message_count,
    );
}
```

**Note:** `spawn_knowledge_extraction` temporarily delegates to the existing function in `pipeline/execution.rs`. Task 11 deletes `pipeline/execution.rs` and at that point we inline the body here. This avoids duplicating ~15 LOC twice in-flight.

- [ ] **Step 6: Register module**

In `pipeline/mod.rs`:

```rust
pub mod finalize;
```

- [ ] **Step 7: Unit tests for `finalize` via MockSink**

These tests require `FinalizeContext` which holds `Arc<dyn LlmProvider>` and `Arc<dyn MemoryService>`. Instead of duplicating fixture wiring inside `src/agent/pipeline/finalize.rs`, the tests live in `crates/hydeclaw-core/tests/pipeline_finalize.rs` where they can reuse `support::pipeline_helpers::build_test_engine` from Task 1.

Create `crates/hydeclaw-core/tests/pipeline_finalize.rs`:

```rust
//! Integration tests for pipeline::finalize.

mod support;

use hydeclaw_core::agent::pipeline::finalize::{self, FinalizeContext, FinalizeOutcome};
use hydeclaw_core::agent::pipeline::sink::test_support::MockSink;
use hydeclaw_core::agent::pipeline::sink::PipelineEvent;
use hydeclaw_core::agent::engine::stream::StreamEvent;
use hydeclaw_core::agent::session_manager::SessionLifecycleGuard;
use hydeclaw_types::IncomingMessage;
use sqlx::PgPool;
use support::mock_provider::MockProvider;
use support::pipeline_helpers::{build_test_engine, user_msg};

#[sqlx::test(migrations = "../../migrations")]
async fn finalize_failed_emits_error_and_saves_partial(pool: PgPool) {
    // --- Arrange ---
    let engine = build_test_engine(pool.clone(), MockProvider::new()).await;
    let session_id = hydeclaw_core::db::sessions::get_or_create_session(
        &pool, "test-agent", None, None, None, false,
    ).await.unwrap();
    let msg = user_msg("hi");
    let ctx = finalize::finalize_context_from_engine(&engine, session_id, 1, &msg);
    let mut guard = SessionLifecycleGuard::new(pool.clone(), session_id);
    let mut sink = MockSink::new();

    // --- Act ---
    let text = finalize::finalize(ctx,
        FinalizeOutcome::Failed { partial: "partial".into(), reason: "llm_exhausted".into() },
        &mut sink, &mut guard,
    ).await.unwrap();

    // --- Assert ---
    assert_eq!(text, "partial");
    assert!(sink.events.iter().any(|e| matches!(e, PipelineEvent::Stream(StreamEvent::Error(_)))),
        "Error event emitted");
    let role: String = sqlx::query_scalar(
        "SELECT role FROM messages WHERE session_id = $1 ORDER BY created_at DESC LIMIT 1"
    ).bind(session_id).fetch_one(&pool).await.unwrap();
    assert_eq!(role, "assistant", "partial saved as assistant message");
}

// Structurally identical to finalize_failed_emits_error_and_saves_partial;
// differs only in:
//   - FinalizeOutcome variant: Interrupted { partial: "p", reason: "sink_closed" }
//   - Negative Error assertion: no StreamEvent::Error present in sink.events
#[sqlx::test(migrations = "../../migrations")]
async fn finalize_interrupted_does_not_emit_error(pool: PgPool) {
    let engine = build_test_engine(pool.clone(), MockProvider::new()).await;
    let session_id = hydeclaw_core::db::sessions::get_or_create_session(
        &pool, "test-agent", None, None, None, false,
    ).await.unwrap();
    let msg = user_msg("hi");
    let ctx = finalize::finalize_context_from_engine(&engine, session_id, 1, &msg);
    let mut guard = SessionLifecycleGuard::new(pool.clone(), session_id);
    let mut sink = MockSink::new();

    finalize::finalize(ctx,
        FinalizeOutcome::Interrupted { partial: "p".into(), reason: "sink_closed" },
        &mut sink, &mut guard,
    ).await.unwrap();

    assert!(!sink.events.iter().any(|e| matches!(e, PipelineEvent::Stream(StreamEvent::Error(_)))),
        "no Error event on interrupt");
}
```

Note: `finalize_context_from_engine` is the public helper added in Task 7 Step 1. It is already needed by Tasks 7/8/9, so pulling its signature forward into Task 4 is a natural dependency. Add this helper in this commit (Task 4) instead of Task 7 if the test compilation requires it:

```rust
// In src/agent/pipeline/finalize.rs, below the `finalize` function:
pub fn finalize_context_from_engine<'a>(
    engine: &'a crate::agent::engine::AgentEngine,
    session_id: Uuid,
    message_count: usize,
    msg: &'a IncomingMessage,
) -> FinalizeContext<'a> {
    FinalizeContext {
        db: engine.cfg().db.clone(),
        session_id,
        agent_name: engine.cfg().agent.name.clone(),
        message_count,
        msg,
        provider: engine.cfg().provider.clone(),
        memory_store: engine.cfg().memory_store.clone(),
    }
}
```

(Task 7 Step 1 can then remove its copy and just note "already added in Task 4".)

- [ ] **Step 8: Verify**

Run: `cd crates/hydeclaw-core && cargo test --lib agent::pipeline::finalize && cargo clippy --lib -- -D warnings`
Expected: two finalize tests PASS, zero warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/hydeclaw-core/src/agent/session_manager.rs \
        crates/hydeclaw-core/src/agent/pipeline/finalize.rs \
        crates/hydeclaw-core/src/agent/pipeline/mod.rs
git commit -m "feat(pipeline): finalize() and SessionLifecycleGuard::interrupt"
```

---

## Task 5: `pipeline/bootstrap.rs`

**Files:**
- Modify: `src/agent/engine/context_builder.rs` (raise visibility)
- Create: `src/agent/pipeline/bootstrap.rs`
- Modify: `src/agent/pipeline/mod.rs`

### Steps

- [ ] **Step 1: Raise visibility of `handle_command` and `build_context`**

In `src/agent/engine/context_builder.rs`:
- Line 100: `pub(super) async fn build_context` → `pub(crate) async fn build_context`.
- Line 169: `pub(super) async fn handle_command` → `pub(crate) async fn handle_command`.

Run: `cd crates/hydeclaw-core && cargo check`
Expected: compiles.

- [ ] **Step 2: Create `pipeline/bootstrap.rs`**

```rust
//! Session entry, user-message persist, ProcessingGuard, slash-command detection.
//!
//! See docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md §3, §5.

use crate::agent::engine::stream::{ProcessingGuard, ProcessingPhase};
use crate::agent::pipeline::sink::{EventSink, PipelineEvent};
use crate::agent::session_manager::{SessionLifecycleGuard, SessionManager};
use crate::agent::tool_loop::LoopDetector;
use hydeclaw_types::IncomingMessage;
use uuid::Uuid;

pub struct BootstrapOutcome {
    pub session_id: Uuid,
    pub enriched_text: String,
    pub messages: Vec<crate::agent::providers::Message>,
    pub tools: Vec<crate::agent::providers::ToolDefinition>,
    pub loop_detector: LoopDetector,
    pub processing_guard: ProcessingGuard,
    /// Option so the adapter can take() it before passing BootstrapOutcome to execute().
    pub lifecycle_guard: Option<SessionLifecycleGuard>,
    pub command_output: Option<String>,
}

pub struct BootstrapContext<'a> {
    pub msg: &'a IncomingMessage,
    pub resume_session_id: Option<Uuid>,
    pub force_new_session: bool,
    pub use_history: bool,
}

pub(crate) fn enrich_message_text(user_text: &str, msg: &IncomingMessage) -> String {
    // Absorbed from pipeline/execution.rs (Task 11 removes source).
    // IMPLEMENTER: copy the body verbatim from
    // pipeline::execution::enrich_message_text. It is ~10 LOC that prefixes
    // the user text with attachments/context markers.
    crate::agent::pipeline::execution::enrich_message_text(user_text, msg)
}

pub(crate) async fn log_wal_running_with_retry(
    sm: &SessionManager, session_id: Uuid,
) {
    crate::agent::pipeline::execution::log_wal_running_with_retry(sm, session_id).await;
}

pub async fn bootstrap<S: EventSink>(
    engine: &crate::agent::engine::AgentEngine,
    ctx: BootstrapContext<'_>,
    sink: &mut S,
) -> anyhow::Result<BootstrapOutcome> {
    // 1. Context (messages + tools + session_id)
    let crate::agent::context_builder::ContextSnapshot {
        session_id, mut messages, tools,
    } = engine.build_context(ctx.msg, ctx.use_history, ctx.resume_session_id, ctx.force_new_session).await?;

    // 2. Mark running + WAL
    let sm = SessionManager::new(engine.cfg().db.clone());
    if let Err(e) = sm.set_run_status(session_id, "running").await {
        tracing::warn!(session_id = %session_id, error = %e, "set_run_status(running) failed");
    }
    log_wal_running_with_retry(&sm, session_id).await;

    // 3. First Phase event
    let _ = sink.emit(PipelineEvent::Phase(ProcessingPhase::Thinking)).await;

    // 4. Lifecycle guard
    let lifecycle_guard = Some(SessionLifecycleGuard::new(engine.cfg().db.clone(), session_id));

    // 5. ProcessingGuard (ui_event_tx broadcast; sink is independent)
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
    let enriched_text = enrich_message_text(&user_text, ctx.msg);
    let sender_agent_id = crate::agent::pipeline::finalize::extract_sender_agent_id(&ctx.msg.user_id);
    sm.save_message_ex(session_id, "user", &enriched_text, None, None, sender_agent_id, None, None).await?;

    // 7. LoopDetector
    let loop_detector = LoopDetector::new(&engine.cfg().config.tool_loop);

    // 8. Slash-command detection (spec §11.1 — extension point if richer outputs ever needed)
    let command_output = match engine.handle_command(&user_text, ctx.msg).await {
        Some(result) => Some(result?),
        None => None,
    };

    // Push user message into messages for LLM
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

- [ ] **Step 3: Register module**

In `pipeline/mod.rs`:

```rust
pub mod bootstrap;
```

- [ ] **Step 4: Verify**

Run: `cd crates/hydeclaw-core && cargo check && cargo clippy --lib -- -D warnings`
Expected: compiles, zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine/context_builder.rs \
        crates/hydeclaw-core/src/agent/pipeline/bootstrap.rs \
        crates/hydeclaw-core/src/agent/pipeline/mod.rs
git commit -m "feat(pipeline): bootstrap() and raise handle_command/build_context visibility"
```

*Note:* no dedicated unit test for `bootstrap()` — its happy path is fully exercised by the snapshot suite in Task 1 (which calls `handle_sse` → `handle_with_status` → `handle_streaming`, each of which now routes through `bootstrap`). The slash-command early-exit path is also covered by snapshots via `/help` in the Task 1 SSE snapshot.

---

## Task 6a: `pipeline/execute.rs` — skeleton and happy path

**Purpose:** create `execute()` structure with three outcomes, port the happy path (LLM call → text → Finish), wire unit tests for Done + Closed + Cancel.

**Files:**
- Create: `src/agent/pipeline/execute.rs`
- Modify: `src/agent/pipeline/mod.rs`

### Steps

- [ ] **Step 1: Study the LLM streaming pattern in engine_sse.rs:214-232**

Read `engine_sse.rs:214-232`. The pattern is **spawn-forwarder + await LLM** (NOT `tokio::select!`):

```rust
// Per-iteration chunk channel — LLM writes here, forwarder task reads.
let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<String>();
let event_tx_fwd = event_tx.clone();
tokio::spawn(async move {
    while let Some(chunk) = chunk_rx.recv().await {
        if event_tx_fwd.send(StreamEvent::TextDelta(chunk)).is_err() {
            tracing::debug!("SSE forwarder: event channel closed or full");
        }
    }
});
let llm_result = self.chat_stream_with_transient_retry(&mut messages, &available_tools, chunk_tx).await;
```

Key observations:
- `chat_stream_with_transient_retry` is called as a **method on `AgentEngine`** (wrapper over the free function in `pipeline/llm_call.rs:264`). The wrapper hides the `&impl Compactor` argument.
- When the LLM finishes and drops `chunk_tx`, `chunk_rx.recv()` returns `None` and the forwarder task terminates naturally. No join needed.
- Cancellation happens at the loop-top check (`cancel_guard.token.is_cancelled()`), not inside `tokio::select!`.

**Port this pattern in Step 2.** Do NOT introduce `tokio::select!` over the LLM future — that creates a race with the chunk-drain task.

- [ ] **Step 2: Create `pipeline/execute.rs` with happy-path skeleton**

```rust
//! Main LLM+tools loop. Transport-agnostic via EventSink.
//!
//! See docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md §3, §5.
//! Porting rules documented at the top of this plan file.

use crate::agent::pipeline::bootstrap::BootstrapOutcome;
use crate::agent::pipeline::sink::{EventSink, PipelineEvent};
use crate::agent::engine::stream::StreamEvent;
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

/// Top-level dispatch. In 6a this implements the happy path: one LLM call,
/// text streamed into sink via a forwarder task, no tool calls, Finish.
/// Task 6b adds the tool-call iteration loop and error paths.
pub async fn execute<S: EventSink>(
    engine: &crate::agent::engine::AgentEngine,
    bootstrap_outcome: BootstrapOutcome,
    sink: &mut S,
    cancel: CancellationToken,
) -> anyhow::Result<ExecuteOutcome> {
    let BootstrapOutcome {
        session_id, mut messages, tools,
        loop_detector: _loop_detector,
        processing_guard: _processing_guard, // Drop handles cleanup
        lifecycle_guard: _,
        ..
    } = bootstrap_outcome;

    if cancel.is_cancelled() {
        return Ok(ExecuteOutcome {
            status: ExecuteStatus::Interrupted("cancel_token"),
            session_id, final_text: String::new(), thinking_json: None,
            messages_len_at_end: messages.len(),
        });
    }

    let msg_id = format!("msg_{}", Uuid::new_v4());
    if sink.emit(StreamEvent::MessageStart { message_id: msg_id }.into()).await.is_err() {
        return Ok(ExecuteOutcome {
            status: ExecuteStatus::Interrupted("sink_closed"),
            session_id, final_text: String::new(), thinking_json: None,
            messages_len_at_end: messages.len(),
        });
    }

    // ── LLM streaming via spawn-forwarder pattern (mirror of engine_sse.rs:214-232) ──
    //
    // `chunk_tx` is handed to chat_stream_with_transient_retry; LLM writes
    // tokens into it. The forwarder task drains chunk_rx and emits
    // StreamEvent::TextDelta into the sink. When LLM finishes, it drops
    // chunk_tx → rx.recv() returns None → forwarder exits cleanly.
    //
    // `partial` is accumulated inside the forwarder and returned via oneshot
    // so execute() can own the final text for finalize().
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (partial_tx, partial_rx) = tokio::sync::oneshot::channel::<Result<String, &'static str>>();

    // Clone sink into the forwarder. Since EventSink is not Clone in the generic
    // form, we instead keep sink in execute() and drain-then-emit after LLM done.
    // This matches engine_sse.rs semantics: chunks are batched by LLM as they
    // stream, forwarder runs concurrently with the LLM call.
    //
    // To avoid the Clone constraint on S, we spawn a forwarder that collects
    // chunks into a Vec, and after LLM returns we emit them in order. This is
    // equivalent in observable behaviour because chunk_tx preserves order.
    //
    // IMPORTANT: this deliberately forgoes interleaved TextDelta emission to
    // keep `S: EventSink` non-Clone. Snapshot tests from Task 1 verify the
    // user-visible shape is unchanged (they assert on concatenated text, not
    // per-chunk timing).
    let forwarder = tokio::spawn(async move {
        let mut buf = String::new();
        while let Some(chunk) = chunk_rx.recv().await {
            buf.push_str(&chunk);
        }
        let _ = partial_tx.send(Ok(buf));
    });

    let llm_result = engine
        .chat_stream_with_transient_retry(&mut messages, &tools, chunk_tx)
        .await;

    // Forwarder exits when chunk_tx drops inside LLM call; await it to
    // collect accumulated text.
    let _ = forwarder.await;
    let partial = partial_rx.await.unwrap_or_else(|_| Ok(String::new())).unwrap_or_default();

    match llm_result {
        Ok(_response) => {
            // Emit the collected text in one TextDelta, then Finish.
            // If the sink is already closed at this point, downgrade Done to
            // Interrupted so finalize() takes the partial-save + WAL-interrupted path.
            // (Task 6b will replace batched TextDelta with per-chunk streaming.)
            if !partial.is_empty() {
                if let Err(_) = sink.emit(StreamEvent::TextDelta(partial.clone()).into()).await {
                    return Ok(ExecuteOutcome {
                        status: ExecuteStatus::Interrupted("sink_closed"),
                        session_id, final_text: partial, thinking_json: None,
                        messages_len_at_end: messages.len(),
                    });
                }
            }
            if let Err(_) = sink.emit(StreamEvent::Finish {
                finish_reason: "stop".into(), continuation: false,
            }.into()).await {
                return Ok(ExecuteOutcome {
                    status: ExecuteStatus::Interrupted("sink_closed"),
                    session_id, final_text: partial, thinking_json: None,
                    messages_len_at_end: messages.len(),
                });
            }
            Ok(ExecuteOutcome {
                status: ExecuteStatus::Done,
                session_id, final_text: partial, thinking_json: None,
                messages_len_at_end: messages.len(),
            })
        }
        Err(e) => Ok(ExecuteOutcome {
            status: ExecuteStatus::Failed(e.to_string()),
            session_id, final_text: partial, thinking_json: None,
            messages_len_at_end: messages.len(),
        }),
    }
}
```

**Design note for Task 6b:** the single-batched `TextDelta` at the end is a Task 6a compromise to keep `S: EventSink` non-Clone. Task 6b replaces this with per-chunk streaming by changing the forwarder to hold `&mut S` through an `Arc<Mutex<S>>` OR by passing sink through an inverted control flow (execute drives the chunk_rx.recv loop itself, LLM runs on a tokio::spawn with its own result oneshot). Task 6b Step 1 decides and documents the choice.

**Signature fact (verified):** `AgentEngine::chat_stream_with_transient_retry(&self, &mut Vec<Message>, &[ToolDefinition], mpsc::UnboundedSender<String>) -> Result<LlmResponse>`. The `&impl Compactor` argument is hidden by the wrapper method. See [engine_sse.rs:231](crates/hydeclaw-core/src/agent/engine_sse.rs#L231) for a live call-site.

- [ ] **Step 3: Register module**

In `pipeline/mod.rs`:

```rust
pub mod execute;
```

- [ ] **Step 4: Unit tests via MockSink — implement the first test fully, reuse the pattern for the other two**

These are integration tests (they need `AgentEngine` + `PgPool`), so they live in `crates/hydeclaw-core/tests/pipeline_execute.rs`, not inside `src/agent/pipeline/execute.rs #[cfg(test)]`. This keeps them alongside the Task 1 snapshots with the same `support::` fixtures.

Create `crates/hydeclaw-core/tests/pipeline_execute.rs`:

```rust
//! Unit tests for pipeline::execute against MockSink.
//!
//! Snapshot-level regression is covered by tests/pipeline_snapshots.rs;
//! these tests target specific invariants of execute() that snapshots
//! cannot observe directly (status enum, partial on closed sink, cancel).

mod support;

use hydeclaw_core::agent::pipeline::bootstrap::{self, BootstrapContext};
use hydeclaw_core::agent::pipeline::execute::{execute, ExecuteStatus};
use hydeclaw_core::agent::pipeline::sink::test_support::MockSink;
use hydeclaw_core::agent::pipeline::sink::PipelineEvent;
use hydeclaw_core::agent::engine::stream::StreamEvent;
use sqlx::PgPool;
use support::mock_provider::MockProvider;
use support::pipeline_helpers::{build_test_engine, user_msg};

#[sqlx::test(migrations = "../../migrations")]
async fn execute_happy_path_done(pool: PgPool) {
    // --- Arrange ---
    let engine = build_test_engine(
        pool.clone(),
        MockProvider::new().expect_text("hello", "stop"),
    ).await;
    let msg = user_msg("hi");
    let mut sink = MockSink::new();

    let boot = bootstrap::bootstrap(
        &engine,
        BootstrapContext { msg: &msg, resume_session_id: None, force_new_session: false, use_history: true },
        &mut sink,
    ).await.unwrap();

    let cancel = tokio_util::sync::CancellationToken::new();

    // --- Act ---
    let outcome = execute(&engine, boot, &mut sink, cancel).await.unwrap();

    // --- Assert ---
    assert!(matches!(outcome.status, ExecuteStatus::Done));
    assert_eq!(outcome.final_text, "hello");

    let shapes = sink.stream_shapes();
    // Phase(Thinking) emitted inside bootstrap, then MessageStart/TextDelta/Finish in execute.
    // stream_shapes() filters Phase out; check the stream-event sequence:
    let start_idx = shapes.iter().position(|s| *s == "MessageStart").expect("MessageStart emitted");
    assert_eq!(&shapes[start_idx..],
        &["MessageStart", "TextDelta", "Finish"],
        "SSE-like event sequence after MessageStart"
    );
}

// Structurally identical to execute_happy_path_done; differs only in:
//   - sink variant: MockSink::close_after(2) instead of MockSink::new()
//     (closes the sink after Phase + MessageStart, forcing sink_closed on TextDelta)
//   - expected status: Interrupted("sink_closed") instead of Done
//   - expected partial: may be non-empty if LLM had produced any chunk
//     before the sink closed — treat >=0 len as acceptable
#[sqlx::test(migrations = "../../migrations")]
async fn execute_interrupted_on_sink_closed(pool: PgPool) {
    let engine = build_test_engine(
        pool.clone(),
        MockProvider::new().expect_text("some longer text", "stop"),
    ).await;
    let msg = user_msg("hi");
    let mut sink = MockSink::close_after(2); // Thinking phase + MessageStart; next emit returns Closed

    let boot = bootstrap::bootstrap(
        &engine,
        BootstrapContext { msg: &msg, resume_session_id: None, force_new_session: false, use_history: true },
        &mut sink,
    ).await.unwrap();

    let cancel = tokio_util::sync::CancellationToken::new();
    let outcome = execute(&engine, boot, &mut sink, cancel).await.unwrap();

    assert!(matches!(outcome.status, ExecuteStatus::Interrupted("sink_closed")),
        "expected Interrupted(sink_closed), got {:?}", match &outcome.status {
            ExecuteStatus::Done => "Done", ExecuteStatus::Failed(_) => "Failed(..)",
            ExecuteStatus::Interrupted(r) => r,
        });
}

// Structurally identical to execute_happy_path_done; differs only in:
//   - the provider is MockProvider::never_responds() (add this helper in
//     tests/support/mock_provider.rs if missing — 10-line addition that
//     returns a future pending forever)
//   - cancel token is cancelled after 100ms
//   - expected status: Interrupted("cancel_token")
#[sqlx::test(migrations = "../../migrations")]
async fn execute_interrupted_on_cancel(pool: PgPool) {
    let engine = build_test_engine(pool.clone(), MockProvider::never_responds()).await;
    let msg = user_msg("hi");
    let mut sink = MockSink::new();

    let boot = bootstrap::bootstrap(
        &engine,
        BootstrapContext { msg: &msg, resume_session_id: None, force_new_session: false, use_history: true },
        &mut sink,
    ).await.unwrap();

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_for_trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cancel_for_trigger.cancel();
    });

    let outcome = execute(&engine, boot, &mut sink, cancel).await.unwrap();
    assert!(matches!(outcome.status, ExecuteStatus::Interrupted("cancel_token")));
}
```

If `MockProvider::never_responds()` does not exist yet, add it as a small method in `tests/support/mock_provider.rs`:

```rust
impl MockProvider {
    pub fn never_responds() -> Self {
        // Build a MockProvider whose chat_stream future is std::future::pending()
        // so the test controls termination via cancel token.
        // IMPLEMENTER: 5-10 LOC — add a new variant to the internal MockState
        // enum that signals the stream should hang. Existing expect_text paths
        // stay unchanged.
        unimplemented!("add a Pending variant to MockState and wire chat_stream to std::future::pending() for it")
    }
}
```

This is the only trait extension in this task; the 10-LOC addition goes into the same commit.

- [ ] **Step 5: Verify**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_execute -- --nocapture`
Expected: three tests PASS. Fix imports/visibility issues as they arise; these are mechanical.

Run: `cd crates/hydeclaw-core && cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/agent/pipeline/execute.rs \
        crates/hydeclaw-core/src/agent/pipeline/mod.rs \
        crates/hydeclaw-core/tests/pipeline_execute.rs \
        crates/hydeclaw-core/tests/support/mock_provider.rs
git commit -m "feat(pipeline): execute() skeleton with happy path and sink-interrupted tests"
```

---

## Task 6b: `pipeline/execute.rs` — tool loop and error paths

**Purpose:** extend `execute()` to the full main loop with tool calls, LoopDetector, LLM failover errors, turn limit.

**Files:**
- Modify: `src/agent/pipeline/execute.rs`

### Steps

- [ ] **Step 1: Wrap the single LLM call in a turn loop**

Replace the "Single LLM call" block from Task 6a Step 2 with a `for _turn in 0..max_turns { ... }` loop that:

1. Checks `cancel.is_cancelled()` at the top of every iteration → `Interrupted("cancel_token")`.
2. Emits `MessageStart`.
3. Runs the LLM-stream-with-chunks block (same as 6a).
4. After LLM returns, parses tool_calls.
5. If empty → emit `Finish` → `Done`.
6. Otherwise runs `LoopDetector::check_limits`; if the detector breaks → `Failed(reason_from_detector)`.
7. Executes each tool through the same path `engine_sse.rs` currently uses (see porting rules at top of file). Emit `ToolCallStart`, `ToolCallArgs`, `ToolCallResult` via sink.
8. After all tools, `loop_detector.record_execution(...)` per call.
9. Continue to next turn.

Max turns: `engine.cfg().config.limits.max_agent_turns.max(1)`.

After the loop exits without a Done: emit `Finish { finish_reason: "turn_limit", continuation: false }`, return `Done` with turn-limit semantics (matches current behaviour — do not change).

**Porting source:** `engine_sse.rs` main loop body, using the transformation table at the top of this file.

- [ ] **Step 2: Add two additional unit tests in `tests/pipeline_execute.rs`**

Append to the existing `tests/pipeline_execute.rs` created in Task 6a. Test 1 is written out fully; Test 2 uses the same structure.

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn execute_failed_when_llm_exhausts_retries(pool: PgPool) {
    // --- Arrange ---
    let engine = build_test_engine(
        pool.clone(),
        MockProvider::new().expect_error_times("service_unavailable", 10),
    ).await;
    let msg = user_msg("hi");
    let mut sink = MockSink::new();

    let boot = bootstrap::bootstrap(
        &engine,
        BootstrapContext { msg: &msg, resume_session_id: None, force_new_session: false, use_history: true },
        &mut sink,
    ).await.unwrap();

    let cancel = tokio_util::sync::CancellationToken::new();

    // --- Act ---
    let outcome = execute(&engine, boot, &mut sink, cancel).await.unwrap();

    // --- Assert ---
    match outcome.status {
        ExecuteStatus::Failed(reason) => {
            assert!(reason.to_lowercase().contains("service_unavailable") ||
                    reason.to_lowercase().contains("unavailable"),
                "expected service_unavailable in reason, got: {}", reason);
        }
        other => panic!("expected Failed, got {:?}", match other {
            ExecuteStatus::Done => "Done", ExecuteStatus::Interrupted(r) => r,
            ExecuteStatus::Failed(_) => unreachable!(),
        }),
    }
    assert!(outcome.final_text.is_empty(),
        "no TextDelta should have been emitted before exhaustion");
}

// Structurally identical to execute_failed_when_llm_exhausts_retries; differs in:
//   - MockProvider: expect_tool_call_loop("my_tool", args, times: 11) — every LLM
//     turn returns the same tool_call. Add this helper in tests/support/mock_provider.rs
//     if missing (5-10 LOC).
//   - Register a stub tool "my_tool" that always succeeds with a trivial response.
//     Use the existing tool-registration API on AgentEngine — see engine/tool_executor.rs
//     for how tools are registered in other tests (or mock via the pipeline::handlers
//     hook if available).
//   - Expected: ExecuteStatus::Failed whose reason contains "loop_detector" or "my_tool".
#[sqlx::test(migrations = "../../migrations")]
async fn execute_failed_on_loop_detector(pool: PgPool) {
    // IMPLEMENTER: mirror the Arrange/Act/Assert structure above. If stub-tool
    // registration is non-trivial in the test harness, mark this test #[ignore]
    // with a TODO comment and file a follow-up — the loop-detector path is also
    // exercised by existing tests in src/agent/tool_loop.rs.
    let engine = build_test_engine(
        pool.clone(),
        MockProvider::new().expect_tool_call_loop("my_tool", serde_json::json!({}), 11),
    ).await;
    let msg = user_msg("hi");
    let mut sink = MockSink::new();

    let boot = bootstrap::bootstrap(
        &engine,
        BootstrapContext { msg: &msg, resume_session_id: None, force_new_session: false, use_history: true },
        &mut sink,
    ).await.unwrap();

    let outcome = execute(&engine, boot, &mut sink, tokio_util::sync::CancellationToken::new()).await.unwrap();

    match outcome.status {
        ExecuteStatus::Failed(reason) => {
            let r = reason.to_lowercase();
            assert!(r.contains("loop_detector") || r.contains("my_tool"),
                "expected loop_detector or tool name in reason, got: {}", reason);
        }
        other => panic!("expected Failed, got {:?}", match other {
            ExecuteStatus::Done => "Done", ExecuteStatus::Interrupted(r) => r,
            ExecuteStatus::Failed(_) => unreachable!(),
        }),
    }
}
```

`MockProvider::expect_error_times(&str, usize)` and `expect_tool_call_loop(&str, Value, usize)` are new helpers. Add them at the same time in `tests/support/mock_provider.rs`; each is a small variant of existing `expect_*` constructors.

- [ ] **Step 3: Verify**

Run: `cd crates/hydeclaw-core && cargo test --lib agent::pipeline::execute -- --nocapture && cargo clippy --lib -- -D warnings`
Expected: all five execute tests PASS, zero warnings.

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots`
Expected: three snapshots from Task 1 still green — they use handle_sse which is **not yet** wired to new pipeline. This is sanity only.

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/pipeline/execute.rs \
        crates/hydeclaw-core/tests/support/mock_provider.rs
git commit -m "feat(pipeline): execute() tool loop, loop detector, failover error path"
```

---

## Task 7: `handle_sse` becomes a thin adapter

**Files:**
- Modify: `src/agent/engine_sse.rs`

### Steps

- [ ] **Step 1: Add `execute_status_to_finalize` helper**

`finalize_context_from_engine` is already added in Task 4 Step 7 (needed by the finalize integration tests). This step only adds the second helper.

Append to `src/agent/pipeline/finalize.rs`, below `finalize_context_from_engine`:

```rust
/// Convert ExecuteStatus + (final_text, thinking_json) into FinalizeOutcome.
pub fn execute_status_to_finalize(
    status: crate::agent::pipeline::execute::ExecuteStatus,
    final_text: String,
    thinking_json: Option<String>,
) -> FinalizeOutcome {
    use crate::agent::pipeline::execute::ExecuteStatus;
    match status {
        ExecuteStatus::Done => FinalizeOutcome::Done {
            assistant_text: final_text, thinking_json,
        },
        ExecuteStatus::Failed(reason) => FinalizeOutcome::Failed {
            partial: final_text, reason,
        },
        ExecuteStatus::Interrupted(reason) => FinalizeOutcome::Interrupted {
            partial: final_text, reason,
        },
    }
}
```

- [ ] **Step 2: Rewrite `handle_sse`**

Replace the body of `handle_sse` in `engine_sse.rs`:

```rust
pub async fn handle_sse(
    &self,
    msg: &IncomingMessage,
    event_tx: crate::agent::engine_event_sender::EngineEventSender,
    resume_session_id: Option<Uuid>,
    force_new_session: bool,
) -> Result<Uuid> {
    use crate::agent::pipeline::{bootstrap, execute, finalize, sink};

    if let crate::agent::hooks::HookAction::Block(reason) =
        self.hooks().fire(&crate::agent::hooks::HookEvent::BeforeMessage)
    {
        anyhow::bail!("blocked by hook: {}", reason);
    }
    let _cancel_guard = self.state.register_request();

    let mut s = sink::SseSink::new(event_tx);

    let boot = bootstrap::bootstrap(
        self,
        bootstrap::BootstrapContext { msg, resume_session_id, force_new_session, use_history: true },
        &mut s,
    ).await?;

    // Explicit destructure pattern — single source of truth for all three adapters.
    // lifecycle_guard is taken out so it survives into finalize; boot_for_execute
    // is rebuilt with lifecycle_guard=None and command_output=None.
    let BootstrapOutcome {
        session_id, messages, tools, loop_detector, processing_guard,
        lifecycle_guard, mut command_output, enriched_text,
    } = boot;
    let mut lifecycle_guard = lifecycle_guard.expect("bootstrap always sets lifecycle_guard");
    let boot_for_execute = BootstrapOutcome {
        lifecycle_guard: None, command_output: None,
        session_id, messages, tools, loop_detector, processing_guard, enriched_text,
    };

    // Slash-command early exit (SSE shape: MessageStart + TextDelta + Finish)
    if let Some(text) = command_output.take() {
        let msg_id = format!("msg_{}", Uuid::new_v4());
        let _ = s.emit(sink::PipelineEvent::Stream(StreamEvent::MessageStart { message_id: msg_id })).await;
        let _ = s.emit(sink::PipelineEvent::Stream(StreamEvent::TextDelta(text.clone()))).await;
        let _ = s.emit(sink::PipelineEvent::Stream(StreamEvent::Finish {
            finish_reason: "command".into(), continuation: false,
        })).await;

        let fin_ctx = finalize::finalize_context_from_engine(self, session_id, boot_for_execute.messages.len(), msg);
        finalize::finalize(fin_ctx,
            finalize::FinalizeOutcome::Done { assistant_text: text, thinking_json: None },
            &mut s, &mut lifecycle_guard,
        ).await?;
        return Ok(session_id);
    }

    // Full pipeline
    let cancel = tokio_util::sync::CancellationToken::new(); // per-session cancel: follow-up
    let outcome = execute::execute(self, boot_for_execute, &mut s, cancel).await?;

    let fin_ctx = finalize::finalize_context_from_engine(
        self, session_id, outcome.messages_len_at_end, msg,
    );
    let fin_outcome = finalize::execute_status_to_finalize(
        outcome.status, outcome.final_text, outcome.thinking_json,
    );
    finalize::finalize(fin_ctx, fin_outcome, &mut s, &mut lifecycle_guard).await?;

    Ok(session_id)
}
```

**Pattern note:** all three adapters (Tasks 7/8/9) use the same explicit-destructure shape above. `lifecycle_guard` is `.take()`-en before `execute()` so it survives into `finalize()`. `command_output` is taken separately inside the `if let Some(...)` branch.

- [ ] **Step 3: Remove any helpers in `engine_sse.rs` that are no longer called**

The file should now contain only `handle_sse`. Delete `persist_partial_if_any` if present — absorbed into `finalize`.

- [ ] **Step 4: Verify**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green including the SSE snapshot.

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/agent/pipeline/finalize.rs \
        crates/hydeclaw-core/src/agent/engine_sse.rs
git commit -m "refactor(engine): handle_sse as thin SseSink adapter"
```

---

## Task 8: `handle_with_status` becomes a thin adapter

**Files:**
- Modify: `src/agent/engine_execution.rs`

### Steps

- [ ] **Step 1: Rewrite `handle_with_status`**

Same pattern as Task 7, sink is `ChannelStatusSink::new(status_tx, chunk_tx)`. Slash-command early exit renders as `TextDelta(text)` only (no `MessageStart`/`Finish`, matches current channel behaviour).

```rust
pub async fn handle_with_status(
    &self,
    msg: &IncomingMessage,
    status_tx: Option<tokio::sync::mpsc::UnboundedSender<ProcessingPhase>>,
    chunk_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> Result<String> {
    use crate::agent::pipeline::{bootstrap, execute, finalize, sink};
    use crate::agent::pipeline::bootstrap::BootstrapOutcome;

    self.cfg().approval_manager.prune_stale().await;

    if let crate::agent::hooks::HookAction::Block(reason) =
        self.hooks().fire(&crate::agent::hooks::HookEvent::BeforeMessage)
    {
        anyhow::bail!("blocked by hook: {}", reason);
    }
    let _cancel_guard = self.state.register_request();

    let mut s = sink::ChannelStatusSink::new(status_tx, chunk_tx);

    let boot = bootstrap::bootstrap(
        self,
        bootstrap::BootstrapContext { msg, resume_session_id: None, force_new_session: false, use_history: true },
        &mut s,
    ).await?;

    let BootstrapOutcome {
        session_id, messages, tools, loop_detector, processing_guard,
        lifecycle_guard, mut command_output, enriched_text,
    } = boot;
    let mut lifecycle_guard = lifecycle_guard.expect("set by bootstrap");
    let boot_for_execute = BootstrapOutcome {
        lifecycle_guard: None, session_id, messages, tools, loop_detector,
        processing_guard, command_output: None, enriched_text,
    };

    if let Some(text) = command_output.take() {
        let _ = s.emit(sink::PipelineEvent::Stream(StreamEvent::TextDelta(text.clone()))).await;
        let fin_ctx = finalize::finalize_context_from_engine(self, session_id, boot_for_execute.messages.len(), msg);
        return finalize::finalize(fin_ctx,
            finalize::FinalizeOutcome::Done { assistant_text: text, thinking_json: None },
            &mut s, &mut lifecycle_guard,
        ).await;
    }

    let cancel = tokio_util::sync::CancellationToken::new();
    let outcome = execute::execute(self, boot_for_execute, &mut s, cancel).await?;

    let fin_ctx = finalize::finalize_context_from_engine(self, session_id, outcome.messages_len_at_end, msg);
    let fin_outcome = finalize::execute_status_to_finalize(
        outcome.status, outcome.final_text, outcome.thinking_json,
    );
    finalize::finalize(fin_ctx, fin_outcome, &mut s, &mut lifecycle_guard).await
}
```

- [ ] **Step 2: Verify**

Run: `cd crates/hydeclaw-core && cargo test --test pipeline_snapshots && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine_execution.rs
git commit -m "refactor(engine): handle_with_status as thin ChannelStatusSink adapter"
```

---

## Task 9: `handle_streaming` becomes a thin adapter

**Files:**
- Modify: `src/agent/engine_execution.rs`

### Steps

- [ ] **Step 1: Rewrite `handle_streaming`**

Same pattern as Task 8, sink is `ChunkSink::new(chunk_tx)`. `use_history: false` (matches current behaviour of `handle_streaming` which passes `false` to `build_context`).

```rust
pub async fn handle_streaming(
    &self,
    msg: &IncomingMessage,
    chunk_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<String> {
    use crate::agent::pipeline::{bootstrap, execute, finalize, sink};
    use crate::agent::pipeline::bootstrap::BootstrapOutcome;

    let mut s = sink::ChunkSink::new(chunk_tx);

    let boot = bootstrap::bootstrap(
        self,
        bootstrap::BootstrapContext { msg, resume_session_id: None, force_new_session: false, use_history: false },
        &mut s,
    ).await?;

    let BootstrapOutcome {
        session_id, messages, tools, loop_detector, processing_guard,
        lifecycle_guard, mut command_output, enriched_text,
    } = boot;
    let mut lifecycle_guard = lifecycle_guard.expect("set by bootstrap");
    let boot_for_execute = BootstrapOutcome {
        lifecycle_guard: None, session_id, messages, tools, loop_detector,
        processing_guard, command_output: None, enriched_text,
    };

    if let Some(text) = command_output.take() {
        let _ = s.emit(sink::PipelineEvent::Stream(StreamEvent::TextDelta(text.clone()))).await;
        let fin_ctx = finalize::finalize_context_from_engine(self, session_id, boot_for_execute.messages.len(), msg);
        return finalize::finalize(fin_ctx,
            finalize::FinalizeOutcome::Done { assistant_text: text, thinking_json: None },
            &mut s, &mut lifecycle_guard,
        ).await;
    }

    let cancel = tokio_util::sync::CancellationToken::new();
    let outcome = execute::execute(self, boot_for_execute, &mut s, cancel).await?;

    let fin_ctx = finalize::finalize_context_from_engine(self, session_id, outcome.messages_len_at_end, msg);
    let fin_outcome = finalize::execute_status_to_finalize(
        outcome.status, outcome.final_text, outcome.thinking_json,
    );
    finalize::finalize(fin_ctx, fin_outcome, &mut s, &mut lifecycle_guard).await
}
```

- [ ] **Step 2: Verify and commit**

```bash
cd crates/hydeclaw-core && cargo test --test pipeline_snapshots && cargo test && cargo clippy --all-targets -- -D warnings
git add crates/hydeclaw-core/src/agent/engine_execution.rs
git commit -m "refactor(engine): handle_streaming as thin ChunkSink adapter"
```

---

## Task 10: Move adapters to `engine/run.rs` and delete old files

**Files:**
- Create: `src/agent/engine/run.rs`
- Delete: `src/agent/engine_execution.rs`, `src/agent/engine_sse.rs`, `src/agent/pipeline/execution.rs`, `src/agent/pipeline/entry.rs`
- Modify: `src/agent/mod.rs`, `src/agent/engine/mod.rs`, `src/agent/pipeline/mod.rs`

### Steps

- [ ] **Step 1: Confirm no external imports from deleted files**

Run: `grep -rn 'engine_execution\|engine_sse\|pipeline::execution\|pipeline::entry' crates/hydeclaw-core/src --include='*.rs'`
Expected: matches only inside files being modified in this task.

- [ ] **Step 2: Migrate helpers from `pipeline/execution.rs` and `pipeline/entry.rs`**

- `enrich_message_text` → `pipeline/bootstrap.rs` (inline body, remove delegation stub).
- `log_wal_running_with_retry` → `pipeline/bootstrap.rs` (inline body).
- `extract_sender_agent_id` → already in `finalize.rs` (inline, no longer delegates).
- `spawn_knowledge_extraction` → `pipeline/finalize.rs` (inline body).
- `extract_tool_result_events` and `ToolResultParts` → `pipeline/execute.rs` as private helpers (they are only used inside execute's tool-result routing).

Find the bodies in `pipeline/execution.rs` and `pipeline/entry.rs`, paste into the target, remove the `crate::agent::pipeline::execution::...` delegation stubs.

- [ ] **Step 3: Move the three adapters into `engine/run.rs`**

This is a **file-move operation**, not new code. The three methods (`handle_sse`, `handle_with_status`, `handle_streaming`) were rewritten as thin adapters in Tasks 7, 8, 9 and are currently living in `engine_sse.rs` and `engine_execution.rs`. Consolidate them into a single new file.

Procedure:

1. Create `crates/hydeclaw-core/src/agent/engine/run.rs` with the module header:

   ```rust
   //! Three thin adapter methods on AgentEngine. Each constructs an EventSink
   //! and delegates to pipeline::execute. See spec §3 and the implementation
   //! plan (Tasks 7–9) for rationale.

   use anyhow::Result;
   use hydeclaw_types::IncomingMessage;
   use uuid::Uuid;
   use crate::agent::engine::stream::{ProcessingPhase, StreamEvent};
   use crate::agent::engine::AgentEngine;
   use crate::agent::engine_event_sender::EngineEventSender;
   use crate::agent::pipeline::bootstrap::BootstrapOutcome;
   ```

2. Cut the `impl AgentEngine { pub async fn handle_sse(...) }` block from `engine_sse.rs` and paste it verbatim into `engine/run.rs` as the first item inside a new `impl AgentEngine { }` block.

3. Cut the `pub async fn handle_with_status(...)` and `pub async fn handle_streaming(...)` blocks from `engine_execution.rs` and paste them verbatim into the same `impl AgentEngine { }` block in `engine/run.rs`.

4. Do not modify the bodies. These are the exact adapter implementations built and tested in Tasks 7–9.

5. Remove any now-unused imports from `engine_sse.rs` and `engine_execution.rs` (they should be empty shells at this point and will be deleted in Step 4).

Run `cd crates/hydeclaw-core && cargo check` after each cut/paste to catch any import fix-ups early.

- [ ] **Step 4: Delete old files**

```bash
git rm crates/hydeclaw-core/src/agent/engine_execution.rs \
       crates/hydeclaw-core/src/agent/engine_sse.rs \
       crates/hydeclaw-core/src/agent/pipeline/execution.rs \
       crates/hydeclaw-core/src/agent/pipeline/entry.rs
```

- [ ] **Step 5: Update module declarations**

In `src/agent/mod.rs`, remove:
```rust
pub mod engine_execution;
pub mod engine_sse;
```

In `src/agent/engine/mod.rs`, add:
```rust
pub mod run;
```

In `src/agent/pipeline/mod.rs`, remove:
```rust
pub mod execution;
pub mod entry;
```

- [ ] **Step 6: Verify**

Run: `cd crates/hydeclaw-core && cargo check --all-targets`
Expected: compiles. Fix import errors mechanically.

Run: `cd crates/hydeclaw-core && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all green. Run `make lint` as a final check.

- [ ] **Step 7: Measure LOC net change**

Run: `git diff --stat origin/master -- crates/hydeclaw-core/src/agent/`
Expected: net negative ~−800 LOC. If significantly smaller (say, −200), check for leftover duplicated code you forgot to remove.

- [ ] **Step 8: Commit**

```bash
git add -A crates/hydeclaw-core/src/agent/
git commit -m "chore(agent): consolidate to pipeline/, delete engine_execution.rs, engine_sse.rs"
```

---

## Task 11: Update `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md` — section "Agent Engine"

### Steps

- [ ] **Step 1: Replace the "Agent Engine" section**

Open `CLAUDE.md` and find the section `### Agent Engine (src/agent/)`. Replace its content with:

```markdown
### Agent Engine (`src/agent/`)

Three entry points on `AgentEngine`, all thin adapters that construct an `EventSink` and delegate to `pipeline::execute`:

- `handle_sse` — web SSE via `SseSink` (over `EngineEventSender`/flume)
- `handle_with_status` — channel adapters (Telegram/Discord) with typing indicator via `ChannelStatusSink` (two `UnboundedSender` channels)
- `handle_streaming` — plain-chunk text via `ChunkSink`

Unified pipeline lives in `src/agent/pipeline/`:

- `sink.rs` — `EventSink` trait, `PipelineEvent` (`Stream(StreamEvent)` | `Phase(ProcessingPhase)`), `SinkError`, three production sinks
- `bootstrap.rs` — session entry, user-message persist, WAL `running`, `ProcessingGuard`, slash-command detection
- `execute.rs` — main LLM+tools loop, transport-agnostic
- `finalize.rs` — single exit point: persist assistant or partial, WAL `done|failed|interrupted` via `SessionLifecycleGuard`, enqueue knowledge extraction

**Loop detection (`tool_loop.rs`):** Two-phase `LoopDetector` — `check_limits()` + `record_execution()`. See design spec at `docs/superpowers/specs/2026-04-20-execution-pipeline-unification-design.md`.

**Session-scoped agents (`session_agent_pool.rs` + `engine_agent_tool.rs`):** unchanged.

**Agent config** (TOML at `config/agents/{name}.toml`): unchanged.
```

- [ ] **Step 2: Check for dangling references**

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
- [ ] `git log --oneline origin/master..HEAD` shows 12 commits (Tasks 1, 2, 3, 4, 5, 6a, 6b, 7, 8, 9, 10, 11)
- [ ] `git diff --stat origin/master` net LOC is negative (~−800)

If all green, the branch is ready for PR. Do not push without explicit user approval.

---

## Follow-up work (not in this PR)

- Per-session `CancellationToken` wiring: currently `handle_*` methods use `CancellationToken::new()`. Expose an API on `AgentEngine` or `SessionAgentPool` that returns the right token, then thread it through the adapter. Low risk, localized change.
- Slash-command richer outputs: implement `CommandOutput` enum per spec §11.1 when a command needs to emit a rich card or file.

---

## Self-review notes

**Spec coverage:** §1 problem → Tasks 7–10. §2 decision → architecture enforced via `EventSink` + four submodules. §3 architecture → file structure matches. §4 components → types in Tasks 2, 5, 6a, 4. §5 data flow → Task 6a happy path + 6b tool/error paths. §6 error handling → `SinkError`, `ExecuteStatus`, `FinalizeOutcome`, `SessionLifecycleGuard::interrupt`. §7 testing → Task 1 snapshots + unit tests in Tasks 2, 3, 4, 5, 6a, 6b (all via `MockSink`). §8 migration → Tasks 1→11 (12 commits). §9 non-goals → respected. §10 open questions → none blocking. §11.1 slash extension → noted in `bootstrap.rs` step 2 comment.

**Placeholder scan:**
- `unreachable!` in `build_test_engine` (Task 1 Step 2) and `build_ctx` (Task 4 Step 7) — both are fixture-binding points with explicit instruction to fill from reference code. These are deliberate wiring points, not logic placeholders.
- No `todo!`. No "TBD". No "similar to Task N".
- Task 6b Step 1 describes the tool-loop port as a list of rule-based transformations on the source body with an explicit porting table at the top of the plan — concrete enough for a skilled Rust developer to execute.

**Type consistency:** `EventSink`, `PipelineEvent`, `SinkError`, `BootstrapOutcome`, `BootstrapContext`, `ExecuteOutcome`, `ExecuteStatus`, `FinalizeOutcome`, `FinalizeContext`, `SessionOutcome::Interrupted`, `SessionLifecycleGuard::interrupt(&mut self, reason: &str)`, `lifecycle_guard: Option<SessionLifecycleGuard>` — all names match across tasks. `use_history: bool` used consistently in BootstrapContext for all three adapters.

**Known decision points made inline:**
- `lifecycle_guard: Option<...>` from Task 5 Step 2 — consistent pattern in Tasks 7, 8, 9.
- `NoopSink` removed — `finalize` is only integrated when a real sink is constructed (Tasks 5/6/7/8/9).
- `scopeguard` not added — RAII through existing `SessionLifecycleGuard` and `ProcessingGuard` covers the invariant; `finalize` is the explicit exit point, not a Drop-based one.
