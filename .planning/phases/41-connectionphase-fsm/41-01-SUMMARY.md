---
phase: 41-connectionphase-fsm
plan: "01"
subsystem: ui/chat-store
tags: [fsm, connection-phase, stream-lifecycle, incremental-parser, multi-agent]
dependency_graph:
  requires: []
  provides: [ConnectionPhase type, isActivePhase helper, IncrementalParser.reset()]
  affects: [ui/src/stores/chat-store.ts, ui/src/lib/message-parser.ts]
tech_stack:
  added: []
  patterns: [FSM enum mirroring, atomic cleanup on finish event, parser reset between turns]
key_files:
  created:
    - ui/src/__tests__/message-parser.test.ts
  modified:
    - ui/src/stores/chat-store.ts
    - ui/src/lib/message-parser.ts
decisions:
  - ConnectionPhase runs in parallel with StreamStatus (not replacing it) — Phase 45 CLN-01 removes StreamStatus
  - reset() placed between flush() and appendToLast() for logical grouping
  - sessionStorage flag cleared in finish handler (not just sync) to prevent dangling flag
metrics:
  duration_minutes: 25
  completed_date: "2026-04-09T11:41:56Z"
  tasks_completed: 2
  files_modified: 3
---

# Phase 41 Plan 01: ConnectionPhase FSM Summary

**One-liner:** ConnectionPhase enum added as authoritative FSM mirroring all streamStatus transitions, IncrementalParser.reset() added and called on finish events for multi-agent turn hygiene.

## What Was Built

### Task 1: IncrementalParser.reset() (TDD)

Added `reset()` method to `IncrementalParser` in `ui/src/lib/message-parser.ts` that clears all three internal fields: `parts`, `insideThink`, and `accum`. This prevents reasoning state from leaking between agent turns in multi-agent sessions.

Created `ui/src/__tests__/message-parser.test.ts` with 3 tests covering:
1. `insideThink` cleared — text after reset classified as text, not reasoning
2. `accum` cleared — empty delta after reset returns no parts
3. `parts` cleared — flush after reset returns empty

### Task 2: ConnectionPhase FSM in chat-store.ts

**Step 1:** Exported `ConnectionPhase = "idle" | "submitted" | "streaming" | "complete" | "error"` and `isActivePhase()` helper alongside existing `StreamStatus` (backward compat).

**Step 2:** Added `connectionPhase: ConnectionPhase` and `connectionError: string | null` to `AgentState` interface and `emptyAgentState()`.

**Step 3:** Mirrored all 22 `streamStatus` transitions to `connectionPhase` across:
- `resumeStream` (streaming, 204-idle, catch-idle)
- `abortActiveStream` (idle)
- `startStream` initial update (submitted), catch (error)
- `processSSEStream` pushUpdate (streaming)
- `sync` handler finished/error (idle/error)
- `error` handler (error)
- `finally` block (idle)
- `setCurrentAgent` carry-over, abort, reset (various)
- `selectSessionById`, `newChat`, `stopStream`, `regenerate`, `regenerateFrom` (idle)
- `deleteSession`, `deleteAllSessions` (idle)

**Step 4 (FSM-04):** Called `incrementalParser.reset()` in the `finish` handler after `pushUpdate()`.

**Step 5 (FSM-03):** Added `sessionStorage.removeItem(\`hydeclaw.streaming.\${agent}\`)` in the `finish` handler so the streaming flag is cleared atomically on natural stream completion (not just in the `sync` handler which is optional).

## Decisions Made

- **StreamStatus preserved**: ConnectionPhase runs in parallel — Phase 45 CLN-01 will remove StreamStatus. All code using StreamStatus continues to work unchanged.
- **`complete` phase available**: Added "complete" as a distinct transient state (between finish event and finalizeStream) even though it's not used yet — Phase 42 will use it for UI animations.
- **reset() placement**: Added between `flush()` and `appendToLast()` for logical flow (lifecycle methods grouped together).

## Verification Results

- `grep -c "connectionPhase" ui/src/stores/chat-store.ts` → 22 occurrences
- `grep "incrementalParser.reset" ui/src/stores/chat-store.ts` → found in finish handler
- `grep "export type ConnectionPhase" ui/src/stores/chat-store.ts` → exported
- `sessionStorage.removeItem` → 2 occurrences (sync + finish handlers)
- All 388 tests pass (0 failures, 0 regressions)

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None — all ConnectionPhase transitions are wired. The `"complete"` phase value is defined but not yet set at any transition site (transient state will be used in Phase 42 for UI animations). This is intentional — the type is available and forward-compatible.

## Self-Check: PASSED

Files created/modified:
- FOUND: ui/src/__tests__/message-parser.test.ts
- FOUND: ui/src/lib/message-parser.ts (reset() method present)
- FOUND: ui/src/stores/chat-store.ts (ConnectionPhase, connectionPhase field, incrementalParser.reset())

Commits:
- d7e4ae6: feat(41-01): add IncrementalParser.reset() with tests
- 4a61b12: feat(41-01): add ConnectionPhase FSM to AgentState, wire all transitions
