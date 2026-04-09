---
phase: 40-sseconnection-extraction
plan: 01
subsystem: ui
tags: [sse, typescript, zustand, react, streaming, testing]

# Dependency graph
requires: []
provides:
  - SseConnection class in ui/src/lib/sse-connection.ts with connect/stop lifecycle
  - SseConnectionConfig and SseConnectionCallbacks interfaces
  - Unit tests for SseConnection in isolation (15 tests)
  - Refactored chat-store using SseConnection instead of inline processSSEStream
affects:
  - 41-fsm-chat-state
  - 43-reconnect
  - 45-cleanup

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Transport isolation: SSE connection lifecycle owned by SseConnection class (no React/Zustand deps)"
    - "Callback-driven: SseConnection dispatches typed events via onEvent/onError/onDone callbacks"
    - "StreamContext pattern: mutable context object passed through handleSseEvent and finalizeStream"
    - "Module-global connection registry: agentConnections Record<string, SseConnection | null>"

key-files:
  created:
    - ui/src/lib/sse-connection.ts
    - ui/src/__tests__/sse-connection.test.ts
  modified:
    - ui/src/stores/chat-store.ts

key-decisions:
  - "SseConnection calls onDone only on natural completion, not on abort — finalizeStream is always natural-end path"
  - "streamGeneration counter kept as module-global (Phase 45 CLN-02 will make it per-agent)"
  - "Partial response on abort is preserved via pushUpdate() already committed during event handling, not in finalizeStream"
  - "flushText() is a no-op stub (logic handled by incrementalParser.flush) — kept for API compatibility"

patterns-established:
  - "SseConnection: transport class depends only on Web APIs and sse-events.ts, zero React/Zustand"
  - "handleSseEvent: all store mutations stay in store, transport dispatches via typed callbacks"
  - "createStreamContext: factory function creates per-stream mutable context with closures"

requirements-completed:
  - SSE-01

# Metrics
duration: 8min
completed: 2026-04-09
---

# Phase 40 Plan 01: SseConnection Extraction Summary

**Extracted ~400-line SSE transport from chat-store into standalone SseConnection class (lib/sse-connection.ts) with 15 unit tests; store becomes thin coordinator via callbacks**

## Performance

- **Duration:** 8 min
- **Started:** 2026-04-09T11:17:08Z
- **Completed:** 2026-04-09T11:24:20Z
- **Tasks:** 2 (Task 3 is checkpoint — awaiting human verify)
- **Files modified:** 3

## Accomplishments

- Created `SseConnection` class (lib/sse-connection.ts) that owns fetch + ReadableStream + abort + SSE parsing + event dispatch with zero React/Zustand dependencies
- Wrote 15 unit tests covering connect/stop/abort/204/error lifecycle independently of React
- Refactored chat-store to use SseConnection: processSSEStream deleted, agentAbortControllers replaced with agentConnections, handleSseEvent and createStreamContext extracted
- All 385 existing tests pass with zero regressions

## Task Commits

1. **Task 1: Create SseConnection class and unit tests** - `987031a` (feat)
2. **Task 2: Wire SseConnection into chat-store** - `464aa46` (feat)

**Plan metadata:** (pending final docs commit)

## Files Created/Modified

- `ui/src/lib/sse-connection.ts` - SseConnection class with connect/stop/isActive, SseConnectionConfig, SseConnectionCallbacks interfaces
- `ui/src/__tests__/sse-connection.test.ts` - 15 unit tests for SseConnection in isolation (no React/Zustand)
- `ui/src/stores/chat-store.ts` - Refactored: processSSEStream deleted, handleSseEvent extracted, agentConnections replaces agentAbortControllers

## Decisions Made

- `onDone` is only called on natural stream completion (not abort) — this simplifies `finalizeStream` to a clean natural-completion path only
- Partial response preservation on abort works because events are committed to store via `pushUpdate()` during event processing, not in the finalization callback
- `streamGeneration` module-global kept for now (per STATE.md, per-agent is Phase 45 scope)
- `flushText()` kept as a no-op stub for API shape consistency

## Deviations from Plan

None — plan executed exactly as written. All switch block logic moved verbatim into handleSseEvent, all throttle/scheduleUpdate closures preserved as StreamContext methods.

## Issues Encountered

None. The test `stopStream sets status=idle and preserves partial message` passed without additional abort-path code because push events are committed to the store during normal event processing, not deferred to finalization.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- SseConnection class is ready for Phase 43 (reconnect with exponential backoff — just extend SseConnection)
- handleSseEvent is ready for Phase 41 (FSM — replace the switch block with state machine calls)
- All 385 tests passing confirms zero regressions

## Self-Check: PASSED

- ui/src/lib/sse-connection.ts: FOUND
- ui/src/__tests__/sse-connection.test.ts: FOUND
- ui/src/stores/chat-store.ts: FOUND
- commit 987031a: FOUND
- commit 464aa46: FOUND
