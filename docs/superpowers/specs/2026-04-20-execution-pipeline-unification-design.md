# Execution Pipeline Unification — Design Spec

**Date:** 2026-04-20
**Author:** brainstorming session with Claude
**Status:** Draft, awaiting user review
**Scope:** `crates/hydeclaw-core/src/agent/` — unification of three agent-execution entry points into a single pipeline behind an `EventSink` abstraction.

---

## 1. Problem statement

### Current state (verified against code)

Three public methods on `AgentEngine` execute an agent turn:

- `handle_sse(msg: &IncomingMessage, event_tx: EngineEventSender, resume_session_id: Option<Uuid>, force_new_session: bool) -> Result<Uuid>` — used by web UI via `gateway/handlers/chat.rs` and `sessions.rs`. `EngineEventSender` is a wrapper over a flume channel of `StreamEvent` with explicit text-vs-non-text delivery semantics ([engine_event_sender.rs](crates/hydeclaw-core/src/agent/engine_event_sender.rs)).
- `handle_with_status(msg: &IncomingMessage, status_tx: Option<UnboundedSender<ProcessingPhase>>, chunk_tx: Option<UnboundedSender<String>>) -> Result<String>` — used by `gateway/handlers/channel_ws.rs` for Telegram/Discord with typing indicator.
- `handle_streaming(msg: &IncomingMessage, chunk_tx: UnboundedSender<String>) -> Result<String>` — used by `gateway/handlers/channel_ws.rs` for plain-text chunk delivery.

They live in two files, `engine_execution.rs` (691 LOC) and `engine_sse.rs` (945 LOC), and share roughly 120 lines of near-identical initialization logic: `SessionManager` entry, WAL `running` event, `enrich_message_text`, `save_message_ex`, `LoopDetector::new`, `ProcessingGuard` creation. Tail logic (partial persistence, WAL done/failed, knowledge extraction) is also duplicated with minor divergences.

Earlier decomposition into `pipeline/` and `engine/` subdirectories has extracted helpers (`pipeline/execution.rs`, `pipeline/llm_call.rs`, `pipeline/handlers.rs`, `engine/stream.rs`) but the main lifecycle still duplicates.

### Observations verified

| Claim | Verdict | Notes |
|-------|---------|-------|
| Macro-level duplication between SSE and non-SSE | Confirmed | ~120 LOC of setup + tail logic duplicated. |
| `ModelOverride` and `UnconfiguredProvider` abstractions exist in `providers.rs` | Confirmed | Not targeted by this refactor. |
| `chat_stream_with_transient_retry` is a shared helper | Confirmed | Located in `pipeline/llm_call.rs:264`, not in `providers.rs` as originally suspected. Retries are transparent to `LoopDetector` — no conflict. |
| `handle_command` duplicated | Partial | Function is single, in `engine/context_builder.rs:169`. What duplicates is the **rendering** of its result (execution returns `Result<String>`, SSE emits `MessageStart`/`TextDelta`/`Finish` manually). |
| `ProcessingGuard` risk of being forgotten | False alarm | It is RAII (`Drop`-based); forgetting it causes no runtime issue. Move to pipeline is an ergonomic improvement, not a reliability fix. |

### Why refactor

- A change to any lifecycle step (new WAL event, new hook, new loop-detector metric) currently requires touching both files and keeping them in sync.
- The tail-logic divergence is the kind of drift that produces subtle bugs (e.g., WAL `done` emitted in one path but not another after a small refactor).
- Net code reduction is real: two large files collapse into a set of small focused modules.

---

## 2. Decision

Adopt **option C1** from brainstorming: introduce an `EventSink` trait and a single private `pipeline::execute<S: EventSink>` function. The three public methods remain as thin adapters that construct the appropriate sink and delegate.

### Rationale over alternatives

- **Over C2 (builder pattern):** only three call-sites, builder does not earn its complexity.
- **Over C3 (StreamEvent + external adapters):** would push duplication from the core to three adapter modules — moves the problem, does not solve it.
- **Over C1 with `dyn EventSink`:** generic `<S: EventSink>` is zero-cost, compile-checked, and all three sinks are monomorphized once each.

### Seven refinements adopted

1. Generic `S: EventSink`, not `Box<dyn EventSink>`.
2. `async fn` in trait (Rust 2024 edition — no `async_trait` crate).
3. Typed `SinkError { Closed, Full, Fatal }` with semantic meaning per variant.
4. Finalization centralized in a single `pipeline::finalize` function reached by every exit path.
5. `ProcessingGuard` created inside `pipeline::bootstrap`, not at call-site.
6. Module split: `pipeline/{sink,bootstrap,execute,finalize}.rs`.
7. Backward-compatible public API — `AgentEngine::{handle_sse, handle_with_status, handle_streaming}` remain and become thin adapters.

### Non-goals

- Changing `StreamEvent` variants or their wire format.
- Touching `providers.rs`, retry/failover policy, or `LoopDetector` semantics.
- Changes to `gateway/` handlers (except if unused imports appear).
- Performance tuning.
- Property-based tests (deferred, YAGNI for this PR).
- Coverage thresholds (infrastructure, not part of this PR).

---

## 3. Architecture

```
┌─────────────────┐   ┌─────────────────┐   ┌─────────────────────────┐
│  gateway/chat   │   │ gateway/sessions│   │  gateway/channel_ws     │
└────────┬────────┘   └────────┬────────┘   └──┬────────────┬─────────┘
         │handle_sse           │handle_sse     │status+chunk│chunk_only
         ▼                     ▼               ▼            ▼
   ┌──────────────────────────────────────────────────────────────┐
   │  AgentEngine (thin wrappers, ~20 LOC each)                    │
   │    handle_sse → SseSink        handle_with_status →           │
   │                                  ChannelStatusSink             │
   │                                handle_streaming → ChunkSink   │
   └─────────────────────────┬────────────────────────────────────┘
                             │ pipeline::execute<S: EventSink>(ctx, sink)
                             ▼
   ┌──────────────────────────────────────────────────────────────┐
   │                         pipeline/                             │
   │  ┌──────────┐  ┌────────────┐  ┌──────────┐  ┌─────────────┐ │
   │  │bootstrap │→ │  execute   │→ │ finalize │  │    sink     │ │
   │  │ (init)   │  │(LLM+tools) │  │ (WAL+DB) │  │  (trait)    │ │
   │  └──────────┘  └────────────┘  └──────────┘  └─────────────┘ │
   └──────────────────────────────────────────────────────────────┘
                             │
              emits StreamEvent via sink.emit(..)
```

### Module responsibilities

- `pipeline/sink.rs` — `EventSink` trait, `SinkError`, three production sinks (`SseSink`, `ChannelStatusSink`, `ChunkSink`) plus `MockSink` under `#[cfg(test)]`.
- `pipeline/bootstrap.rs` — `SessionManager::enter`, `enrich_message_text`, `save_message_ex` for the user message, WAL `running`, `LoopDetector::new`, `ProcessingGuard` creation, and early-exit for slash commands.
- `pipeline/execute.rs` — main loop: LLM call via `chat_stream_with_transient_retry` (unchanged), tool parsing and execution, loop detection, child-agent invocation.
- `pipeline/finalize.rs` — single exit point: persist assistant message or partial, WAL `done|failed|interrupted`, enqueue knowledge extraction, drop `ProcessingGuard`.

### Boundary contracts

- Into `execute`: `ExecutionContext { msg, session_id, force_new_session, sender_agent_id, cancel: CancellationToken }` and `&mut S: EventSink`.
- Out of `execute`: `Result<ExecutionSummary, ExecutionError>` where `ExecutionSummary { status, session_id, final_text, tool_calls }`. `SinkError::Closed` is caught **inside** `execute` and transformed into `status = Interrupted`; it is never propagated as an error.
- Sink never touches DB, WAL, or knowledge extraction — only the transport.

### Architectural invariants

1. Every return from `pipeline::execute` passes through `finalize`.
2. `ProcessingGuard` is constructed in `bootstrap` and lives until `finalize` returns.
3. The sink is the only component that knows about the transport (flume/mpsc/WebSocket).

---

## 4. Components and types

### `EventSink` trait

```rust
// pipeline/sink.rs
use crate::agent::engine::stream::StreamEvent;

#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("sink closed (client disconnected)")]
    Closed,
    #[error("sink full (backpressure, backlog dropped)")]
    Full,
    #[error(transparent)]
    Fatal(#[from] anyhow::Error),
}

pub trait EventSink: Send {
    async fn emit(&mut self, ev: StreamEvent) -> Result<(), SinkError>;

    /// Graceful flush before drop (for collector-style sinks).
    /// Default: no-op.
    async fn close(&mut self) -> Result<(), SinkError> { Ok(()) }
}
```

### Production sinks

```rust
pub struct SseSink { tx: flume::Sender<StreamEvent> }

impl EventSink for SseSink {
    async fn emit(&mut self, ev: StreamEvent) -> Result<(), SinkError> {
        self.tx.send_async(ev).await.map_err(|_| SinkError::Closed)
    }
}

pub struct ChannelStatusSink {
    status_tx: Option<mpsc::UnboundedSender<ProcessingPhase>>,
    chunk_tx:  mpsc::UnboundedSender<String>,
    buffer:    String,
}

impl EventSink for ChannelStatusSink {
    async fn emit(&mut self, ev: StreamEvent) -> Result<(), SinkError> {
        match ev {
            StreamEvent::Phase(p) => {
                if let Some(tx) = &self.status_tx { let _ = tx.send(p); }
            }
            StreamEvent::TextDelta(s) => {
                self.buffer.push_str(&s);
                self.chunk_tx.send(s).map_err(|_| SinkError::Closed)?;
            }
            StreamEvent::Error(_) => {} // logged by pipeline; stream terminates next tick
            _ => {}                      // tool/file/card events not relevant to channel transport
        }
        Ok(())
    }
}

pub struct ChunkSink { tx: mpsc::UnboundedSender<String> }
// filters TextDelta only
```

### Context and result types

```rust
pub struct ExecutionContext {
    pub msg: IncomingMessage,
    pub session_id: Option<Uuid>,
    pub force_new_session: bool,
    pub sender_agent_id: Option<String>,
    pub cancel: CancellationToken,
}

pub enum ExecutionStatus { Done, Failed, Interrupted }

pub struct ExecutionSummary {
    pub status: ExecutionStatus,
    pub session_id: Uuid,
    pub final_text: String,
    pub tool_calls: Vec<ToolCallRecord>,
}
```

Wrapper methods on `AgentEngine` unpack `ExecutionSummary` into the legacy return shape: `Result<String>` for `handle_with_status` and `handle_streaming` (using `final_text`), `Result<Uuid>` for `handle_sse` (using `session_id`).

### Test sink

```rust
#[derive(Default)]
pub struct MockSink { pub events: Vec<StreamEvent> }
impl EventSink for MockSink {
    async fn emit(&mut self, ev: StreamEvent) -> Result<(), SinkError> {
        self.events.push(ev);
        Ok(())
    }
}
```

### Unchanged

- `StreamEvent` variants.
- `ProcessingPhase`, `ProcessingGuard` (still in `engine/stream.rs`).
- `engine_event_sender.rs` helpers (their logging utilities may be reused inside sinks).

---

## 5. Data flow

```
┌─ bootstrap ──────────────────────────────────────────────────────────┐
│ 1. SessionManager::enter(session_id, force_new) → session_id           │
│ 2. WAL::log(running, sender_agent_id)                                  │
│ 3. sink.emit(ProcessingPhase::Thinking)                                │
│ 4. enrich_message_text(msg) → enriched                                 │
│ 5. save_message_ex(enriched, session_id)                               │
│ 6. LoopDetector::new(session)                                          │
│ 7. ProcessingGuard::new(tracker, session_id)                           │
│ 8. handle_command(enriched.text)?:                                     │
│      Some(result) → render_command → jump to finalize(Done)            │
│      None         → continue                                           │
└────────────────────────────────────────────────────────────────────────┘
              │
              ▼
┌─ execute (main loop, max N iterations) ──────────────────────────────┐
│ loop:                                                                  │
│   sink.emit(MessageStart{id})                                          │
│   stream = chat_stream_with_transient_retry(ctx, cancel)              │
│   while let Some(chunk) = stream.next().await:                         │
│     if cancel.is_cancelled(): outcome = Interrupted; break outer       │
│     sink.emit(TextDelta | ToolInputDelta | ...) ?                      │
│         Closed → outcome = Interrupted; break outer                    │
│         Fatal  → outcome = Failed(e);   break outer                    │
│     partial.push_str(...)                                              │
│                                                                        │
│   tool_calls = parse(stream.assistant_message)                         │
│   if tool_calls.is_empty():                                            │
│     sink.emit(Finish); outcome = Done; break                           │
│                                                                        │
│   LoopDetector::check_limits(&tool_calls)?                             │
│   for call in tool_calls:                                              │
│     sink.emit(ToolCallStart/Input/Output)                              │
│     result = execute_tool_call(call)                                   │
│     LoopDetector::record_execution(call, result)                       │
│   continue                                                             │
└────────────────────────────────────────────────────────────────────────┘
              │
              ▼
┌─ finalize (single exit point) ───────────────────────────────────────┐
│ match outcome:                                                         │
│   Done        → save_assistant_message; WAL(done); extract.enqueue()   │
│   Failed(e)   → save_partial_if_any; WAL(failed); sink.emit(Error)     │
│   Interrupted → save_partial_if_any; WAL(interrupted); no emit         │
│ ProcessingGuard::drop → sink.emit(ProcessingPhase::Idle)               │
│ return ExecutionSummary                                                │
└────────────────────────────────────────────────────────────────────────┘
```

### Invariants

- `partial: String` is owned by `execute` locally — not inside the sink. When the sink is closed or faulted, partial must still be persisted; the state machine in `execute` is the source of truth.
- Single `finalize` call per pipeline invocation — enforced via `scopeguard::defer!` or an explicit tail match.
- `cancel` checked at three points: start of each loop iteration, inside `tokio::select!` on stream consumption, and inside `execute_tool_call` (already present).
- Slash-command path never calls the LLM or touches `LoopDetector` — it renders through `render_command(result, &mut sink)` and exits via `finalize(Done)`.
- Retries performed by `chat_stream_with_transient_retry` are transparent to both sink and `LoopDetector`.

---

## 6. Error handling, cancellation, disconnect

### Outcome taxonomy

| Outcome | Triggers | WAL | Partial saved? | Sink `Error`? | Knowledge extract? |
|---------|----------|-----|----------------|---------------|--------------------|
| `Done` | LLM response without tool_calls; slash command | `done` | — | no | yes |
| `Failed` | Provider exhausted retry+failover; LoopDetector; tool panic; DB fatal | `failed` | yes if any | yes | no |
| `Interrupted` | `SinkError::Closed`; `cancel.cancelled()`; graceful shutdown | `interrupted` | yes if any | no | no |

`Interrupted` is a normal termination, not an error — gateway receives `Ok(ExecutionSummary)`.

### Sink error semantics

```rust
match sink.emit(ev).await {
    Ok(()) => {}
    Err(SinkError::Closed)   => { outcome = Interrupted; break }
    Err(SinkError::Full)     => { /* per-sink policy (see below) */ }
    Err(SinkError::Fatal(e)) => { outcome = Failed(e.context("sink fatal")); break }
}
```

Per-sink `Full` policy:

- `SseSink`: unreachable (`flume::send_async` blocks; `EngineEventSender` handles text-vs-non-text semantics internally).
- `ChannelStatusSink`: unreachable in practice — underlying channels are `UnboundedSender`. Policy is defined for robustness if the channel is later made bounded: log warning, drop event, return `Ok(())`.
- `ChunkSink`: same as `ChannelStatusSink`.

### Cancellation

Single `CancellationToken` in `ExecutionContext`. Sources:

- Global `AgentEngine::shutdown()` on SIGTERM.
- Per-session kill via `DELETE /api/sessions/{id}/run` through `SessionAgentPool`.
- Client disconnect on SSE — gateway detects and triggers cancel.

### Disconnect per transport

| Transport | Disconnect signal | Pipeline response |
|-----------|-------------------|-------------------|
| Web SSE | `flume::send_async` → `SendError` | `SinkError::Closed` → `Interrupted` |
| Telegram/Discord | `mpsc::Sender::send` → `SendError` | `SinkError::Closed` → `Interrupted` |
| Plain chunks | same | same |

In every case partial is persisted through `finalize`. Session resume is a separate feature in `gateway/sessions.rs`.

### Panic safety

- `ProcessingGuard` is RAII — `Drop` runs on unwind.
- `finalize` is reached via `scopeguard::defer!` (or equivalent) — runs on panic, writes WAL `failed` with backtrace.
- LLM stream and tool execution already run in `tokio::spawn`-isolated tasks in `pipeline/handlers.rs` — panic in one does not propagate.

### Retry vs LoopDetector boundary

- **Retry** (`pipeline/llm_call.rs:264`): provider-level transient errors (5xx, timeouts, 429 Retry-After). Up to 5 attempts, exponential backoff. Transparent to sink.
- **Failover** (`RoutingProvider`): one provider down, try next route. Transparent.
- **LoopDetector** (`tool_loop.rs`): repeated tool_calls or 3 consecutive errors on the same tool. Does not retry — breaks session with `Failed`.

`LoopDetector` never observes retries — it sees only successfully completed tool_calls. Invariant preserved by construction.

### DB errors

- `bootstrap`: user message save failure → `Failed`, `Error` emitted, session exists in DB.
- `finalize`: assistant message save failure is logged; `finalize` continues (WAL + guard still execute). Trade-off: DB degraded means the session is already lost, but torn state is avoided.

### Primary reliability guarantee

> For any return path from `pipeline::execute`: exactly one WAL event (`done|failed|interrupted`) is written, `ProcessingGuard` is dropped, and the sink has been either closed cleanly or is already gone. The DB never remains in a half-state: either the assistant message is saved, or partial is saved, or nothing is (when user-message save itself failed).

Checked by unit tests with `MockSink` + in-memory SQLite.

---

## 7. Testing strategy

### Coverage matrix

| Level | Location | Purpose | Speed |
|-------|----------|---------|-------|
| Unit — pipeline | `pipeline/execute.rs #[cfg(test)]` | All outcomes and invariants via `MockSink` + stub `SessionManager` | ms |
| Unit — sinks | `pipeline/sink.rs #[cfg(test)]` | `StreamEvent → transport` mapping per sink | ms |
| Integration | `crates/hydeclaw-core/tests/pipeline_integration.rs` | Full `bootstrap → execute → finalize` with in-memory SQLite + `FakeLlmProvider` | seconds |
| E2E gateway | `crates/hydeclaw-core/tests/gateway_*.rs` (existing) | Three wrappers produce the same result as before refactor | seconds |

### Mandatory unit test matrix

Each test = one invariant file, runs against `MockSink`:

1. `done_emits_finish_and_saves_assistant_message` — happy path, events in order `Thinking → MessageStart → TextDelta* → Finish → Idle`, DB has assistant message, WAL `running + done`.
2. `failed_llm_emits_error_and_saves_partial` — fake provider errors after 3 deltas; partial saved, WAL `failed`, `Error` emitted.
3. `interrupted_by_closed_sink_saves_partial_no_error_event` — `MockSink` returns `Closed` after 2 deltas; partial in DB, WAL `interrupted`, no `Error`.
4. `interrupted_by_cancel_token` — token cancelled mid-stream; partial saved, outcome `Interrupted`.
5. `slash_command_emits_three_events_and_done` — `/memory status`; exactly `MessageStart → TextDelta(result) → Finish`, no LLM call, WAL `done`.
6. `loop_detector_breaks_with_failed` — stub tool repeats 11 times; detector fires → `Failed`, WAL `failed`.
7. `processing_guard_emits_idle_on_every_exit_path` — parameterized over all outcomes; first event `Thinking`, last `Idle`.
8. `db_error_on_user_save_fails_gracefully` — DB mock errors on user insert → `Failed`, `Error` emitted, no torn state.
9. `partial_empty_on_interrupt_before_any_delta` — cancel before first chunk; user saved, assistant not, WAL `interrupted`.
10. `retry_transparent_to_sink` — provider fails twice with 5xx, retry recovers; sink sees one `MessageStart → ...` only.

### Sink unit tests

- `SseSink`: `emit` delivers to flume receiver; `Closed` on receiver drop.
- `ChannelStatusSink`: `Phase → status_tx`, `TextDelta → chunk_tx`, tool events ignored; `Full` on chunk_tx → `Ok(())` + warn (verified via `tracing_test`).
- `ChunkSink`: only `TextDelta` passes; rest silently ignored.
- `MockSink`: its own usage in pipeline tests is its verification.

### Integration test

`tests/pipeline_integration.rs`:

- In-memory SQLite with full migrations.
- `FakeLlmProvider` (exists in providers tests).
- Build `AgentEngine` via public API.
- Call `handle_sse(msg, flume_tx, ...)`, drain receiver in parallel.
- Assert event sequence and final DB state.

### Regression guard

Step 1 of migration plan writes snapshot tests against the current code for all three entry points. Those snapshots **must remain green** throughout the refactor. If an intentional behaviour change is required, the snapshot is updated in the same commit with explicit reasoning.

### Not tested here

- Real LLM providers (fakes only).
- Docker containers in unit tests.
- Telegram/Discord live APIs (mocks only).
- Performance benchmarks.

### Deferred

- Property-based tests via `proptest` — only if the unit matrix proves insufficient.
- Coverage thresholds — separate infrastructure task.

---

## 8. Migration plan

Eleven commits on one PR branch. After each commit: `make check && make test && make lint` green. No feature flag — migration is local to `agent/`.

1. **`test(pipeline): regression snapshots for three entry points`** — integration tests against the current `handle_sse`/`handle_with_status`/`handle_streaming` with `FakeLlmProvider` and in-memory SQLite. These snapshots are the contract for the rest of the refactor.
2. **`feat(pipeline): add EventSink trait + MockSink`** — `pipeline/sink.rs`. No call-site changes.
3. **`feat(pipeline): SseSink/ChannelStatusSink/ChunkSink implementations`** — production sinks plus unit tests. Still unused.
4. **`refactor(pipeline): extract finalize()`** — new `pipeline/finalize.rs`; both `engine_execution.rs` and `engine_sse.rs` call it instead of their own tails. Snapshots green.
5. **`refactor(pipeline): extract bootstrap()`** — new `pipeline/bootstrap.rs`; `ProcessingGuard` creation moves here. Both entry points call it. Snapshots green.
6. **`refactor(pipeline): extract execute() main loop`** — new `pipeline/execute.rs`; both entry points shrink to ~50 LOC sink adapters. Snapshots green.
7. **`refactor(engine): handle_sse uses SseSink + pipeline::execute`** — thin wrapper. SSE snapshot green.
8. **`refactor(engine): handle_with_status uses ChannelStatusSink`** — thin wrapper. channel_ws snapshot green.
9. **`refactor(engine): handle_streaming uses ChunkSink`** — thin wrapper. streaming snapshot green.
10. **`chore(agent): delete engine_execution.rs and engine_sse.rs`** — move the three wrappers to `agent/engine/run.rs` (~60 LOC). Verify no external imports of private symbols.
11. **`docs(agent): update CLAUDE.md pipeline architecture section`** — references to the new structure.

### Commit readiness checklist

- `make check && make test && make lint` locally.
- All eleven snapshot tests from step 1 green.
- New unit tests for the module under change included in the same commit.
- Diff under ~500 LOC where possible; step 6 is the largest at roughly ~800 LOC and is logically indivisible.

### Rollback strategy

- Steps 1–3 are pure additions — trivially revertable.
- Steps 4–6 substitute functions — a snapshot regression points to a single commit.
- Steps 7–9 migrate one transport at a time — a regression in one does not block the others.
- Step 10 is deletion — revert restores the files.

### Scope budget

- 11 commits, ~1500–1800 LOC diff, ~800 LOC new production code, ~700 LOC tests.
- Deletion: ~1600 LOC (both files + duplicated helpers).
- Net: approximately **−800 LOC** at the same feature level.

---

## 9. Out of scope

- `StreamEvent` variants or their wire format.
- `providers.rs`, retry policy, `RoutingProvider`.
- `LoopDetector` semantics.
- `gateway/` handlers except incidental import fixes.
- Performance optimization.
- Property-based testing.
- Coverage infrastructure.

---

## 10. Open questions

None at specification time. Any discovery during implementation that contradicts this spec must be documented in the implementation plan with a decision record.

---

## Appendix A — Mapping of current code to new modules

| Current location | New location | Notes |
|------------------|--------------|-------|
| `engine_execution.rs:11-~180` (bootstrap part of `handle_with_status`) | `pipeline/bootstrap.rs` | Deduplicated with SSE counterpart. |
| `engine_sse.rs:19-~200` (bootstrap part of `handle_sse`) | `pipeline/bootstrap.rs` | Same. |
| `engine_execution.rs:~180-~550` (main loop) | `pipeline/execute.rs` | Merged with SSE loop. |
| `engine_sse.rs:~200-~700` (main loop) | `pipeline/execute.rs` | Same. |
| `engine_sse.rs:744` (`persist_partial_if_any`) | `pipeline/finalize.rs` | Now always called on failure paths. |
| `engine_execution.rs:543` (`handle_streaming`) | `agent/engine/run.rs` | Thin wrapper only. |
| `engine/stream.rs` (`ProcessingGuard`, `StreamEvent`) | unchanged | Used by `pipeline/bootstrap` and sinks. |
| `pipeline/llm_call.rs` (retry logic) | unchanged | Called from new `execute.rs`. |
| `pipeline/handlers.rs` (tool handlers) | unchanged | Called from new `execute.rs`. |
| `pipeline/execution.rs` (WAL helpers) | absorbed into `pipeline/bootstrap.rs` and `pipeline/finalize.rs` | Small file, no longer needed as a separate unit. |
| `pipeline/entry.rs` (SSE markers) | absorbed into `pipeline/sink.rs` (`SseSink`) | Moves with its only consumer. |
