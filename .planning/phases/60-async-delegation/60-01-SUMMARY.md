---
phase: 60-async-delegation
plan: 01
subsystem: agent-handoff
tags: [handoff, async, subagent, turn-loop]
dependency_graph:
  requires: []
  provides: [async-handoff, pending-handoffs-drain]
  affects: [chat-turn-loop, engine-handoff, agent-engine-struct]
tech_stack:
  added: []
  patterns: [async-subagent-delegation, labeled-loop-continue, oneshot-completion]
key_files:
  created: []
  modified:
    - crates/hydeclaw-core/src/agent/engine.rs
    - crates/hydeclaw-core/src/agent/engine_handoff.rs
    - crates/hydeclaw-core/src/agent/engine_subagent.rs
    - crates/hydeclaw-core/src/gateway/handlers/chat.rs
    - crates/hydeclaw-core/src/gateway/handlers/agents.rs
decisions:
  - "PendingHandoff uses Vec (ordered drain) not HashMap -- handoffs processed in dispatch order"
  - "Timeout hardcoded to 120s in turn loop -- matches default subagent timeout"
  - "parse_subagent_timeout changed to pub(crate) for cross-module access from engine_handoff"
  - "get_recent_tool_results skipped -- function does not exist in codebase, context enrichment deferred"
metrics:
  duration: 410s
  completed: "2026-04-10T21:10:44Z"
  tasks_completed: 2
  tasks_total: 2
  files_modified: 5
---

# Phase 60 Plan 01: Async Handoff Backend Summary

Rewrite handoff from blocking turn-loop-switching to async subagent delegation using existing SubagentRegistry infrastructure.

## What Changed

### Task 1: PendingHandoff struct and async handle_handoff
- **Removed:** `HandoffRequest` struct, `handoff_target` field, `take_handoff()` method
- **Added:** `PendingHandoff` struct with `subagent_id`, `target_name`, `completion_rx` (oneshot receiver)
- **Added:** `pending_handoffs: Arc<Mutex<Vec<PendingHandoff>>>` field on `AgentEngine`
- **Added:** `take_pending_handoffs()` drain method
- **Rewrote** `handle_handoff` to: resolve target engine, register in target's SubagentRegistry, spawn `run_subagent()` on target engine via `tokio::spawn`, store PendingHandoff on initiator, return immediately
- **Added** unit tests for handoff accepted format string

| Commit | Message |
|--------|---------|
| bad3b0a | feat(60-01): replace HandoffRequest with PendingHandoff, rewrite handle_handoff to spawn async subagent |

### Task 2: Turn loop rewrite for pending_handoffs drain
- **Replaced** blocking handoff routing (handoff_stack, take_handoff, AgentSwitch, RichCard) with pending_handoffs drain
- **Added** labeled `'turn_loop: loop` with `continue 'turn_loop` for correct control flow from inner for-loop
- **Injects** completed results as user messages: `[Response from {target}]\n{result}`
- **Handles** timeout (120s) and channel-closed error cases
- **Preserved** @-mention routing (Priority 2) unchanged

| Commit | Message |
|--------|---------|
| 2917c0c | feat(60-01): rewrite turn loop to drain pending_handoffs and inject results as user messages |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] get_recent_tool_results does not exist**
- **Found during:** Task 1
- **Issue:** Plan references `crate::db::sessions::get_recent_tool_results()` for context enrichment, but this function does not exist in the codebase
- **Fix:** Skipped the enrichment step. The handoff task+context is passed directly without recent tool results. This is a non-blocking simplification -- context enrichment can be added later.
- **Files modified:** crates/hydeclaw-core/src/agent/engine_handoff.rs

**2. [Rule 3 - Blocking] parse_subagent_timeout visibility**
- **Found during:** Task 1
- **Issue:** `parse_subagent_timeout` was `pub(super)` in engine_subagent.rs, inaccessible from engine_handoff.rs (sibling submodule)
- **Fix:** Changed to `pub(crate)` and accessed via `super::subagent_impl::parse_subagent_timeout`
- **Files modified:** crates/hydeclaw-core/src/agent/engine_subagent.rs

**3. [Rule 1 - Bug] IncomingMessage struct mismatch**
- **Found during:** Task 2
- **Issue:** Plan code snippets included `leaf_message_id` field on IncomingMessage, but the actual struct (in hydeclaw-types) does not have this field
- **Fix:** Omitted the field from constructed IncomingMessage instances
- **Files modified:** crates/hydeclaw-core/src/gateway/handlers/chat.rs

## Verification Results

- `cargo check` passes with zero errors (only pre-existing warnings)
- No references to `handoff_stack`, `HandoffRequest`, `take_handoff`, or `handoff_target` remain
- `PendingHandoff` struct, `pending_handoffs` field, `take_pending_handoffs` method all present
- `'turn_loop` label and `continue 'turn_loop` present in chat.rs
- `run_subagent` called inside `tokio::spawn` in engine_handoff.rs
- @-mention routing (Priority 2) preserved unchanged

## Known Stubs

None -- all functionality is fully wired.

## Self-Check: PASSED
