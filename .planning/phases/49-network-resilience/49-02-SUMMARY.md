---
phase: 49-network-resilience
plan: 02
subsystem: ui
tags: [sse, last-event-id, reconnect, dedup, event-id]

# Dependency graph
requires:
  - phase: 49-01
    provides: "Backend SSE event ID emission and Last-Event-ID header support"
provides:
  - "extractSseEventId helper in sse-events.ts"
  - "SseConnection lastEventId tracking and Last-Event-ID header on retry"
  - "chat-store per-agent lastEventId tracking, dedup, Last-Event-ID header on resume"
  - "410 response handling in both SseConnection and chat-store"
  - "reconnectAttempt and maxReconnectAttempts in AgentState for UI indicator"
affects: [49-03-reconnect-ui-indicator]

# Tech tracking
tech-stack:
  added: []
  patterns: ["SSE event ID tracking via module-scope Map (matches agentAbortControllers pattern)", "skipNextData dedup flag for id-then-data SSE pairs"]

key-files:
  created: []
  modified:
    - ui/src/stores/sse-events.ts
    - ui/src/stores/chat-store.ts
    - ui/src/lib/sse-connection.ts
    - ui/src/__tests__/sse-connection.test.ts
    - ui/src/stores/__tests__/sse-parsing.test.ts

key-decisions:
  - "extractSseEventId as standalone helper (not inline) for testability and reuse"
  - "agentLastEventIds as module-scope Map (matches CLN-02 pattern for non-serializable state)"
  - "Dedup uses parseInt comparison (monotonic integer IDs from backend)"
  - "410 response treated as natural completion (history fallback), not error"

patterns-established:
  - "SSE event ID dedup: id: line sets currentEventId, skipNextData flag skips stale data: lines"
  - "Last-Event-ID header sent on both resumeStream and SseConnection.retryConnect"

requirements-completed: [NET-01, NET-02]

# Metrics
duration: 5min
completed: 2026-04-09
---

# Phase 49 Plan 02: Frontend SSE Last-Event-ID Tracking Summary

**Frontend SSE id: line extraction, per-agent lastEventId tracking, Last-Event-ID header on reconnect, event dedup, 410 handling, and reconnectAttempt in AgentState**

## Performance

- **Duration:** 5 min
- **Started:** 2026-04-09T17:45:30Z
- **Completed:** 2026-04-09T17:50:30Z
- **Tasks:** 1 (TDD: RED + GREEN)
- **Files modified:** 5

## Accomplishments
- Added `extractSseEventId` helper to parse SSE `id:` lines with full test coverage
- SseConnection class tracks lastEventId from stream, sends Last-Event-ID header on retryConnect, handles 410 as natural end
- chat-store tracks lastEventId per agent via module-scope Map, sends Last-Event-ID on resumeStream
- Event dedup: events with id <= lastReceivedId are silently skipped via skipNextData flag
- AgentState exposes reconnectAttempt and maxReconnectAttempts for UI reconnect indicator (NET-02)

## Task Commits

Each task was committed atomically (TDD):

1. **Task 1 RED: Failing tests** - `9ae6f53` (test)
2. **Task 1 GREEN: Implementation** - `99ea53d` (feat)

## Files Created/Modified
- `ui/src/stores/sse-events.ts` - Added extractSseEventId helper function
- `ui/src/stores/chat-store.ts` - lastEventId tracking, Last-Event-ID header, dedup, 410 handling, reconnectAttempt in AgentState
- `ui/src/lib/sse-connection.ts` - lastEventId field, Last-Event-ID header on retryConnect, 410 handling, id: line extraction in both connect and retryConnect loops
- `ui/src/__tests__/sse-connection.test.ts` - 4 new tests for lastEventId, Last-Event-ID header, 410 handling
- `ui/src/stores/__tests__/sse-parsing.test.ts` - 7 new tests for extractSseEventId

## Decisions Made
- Used `extractSseEventId` as a standalone exported function rather than inline parsing -- keeps sse-events.ts as the single source of SSE parsing logic
- `agentLastEventIds` follows the CLN-02 pattern of module-scope Maps for non-serializable state (like `_abortControllers` and `_reconnectTimers`)
- Dedup uses `parseInt(eid, 10) <= parseInt(lastId, 10)` because backend emits monotonic integer event IDs
- 410 response treated as natural completion with history fallback (not error) -- stream simply expired, no retry needed

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None.

## Known Stubs
None - all data paths are wired.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- reconnectAttempt and maxReconnectAttempts are exposed in AgentState, ready for Plan 03 to build the UI reconnect indicator
- Last-Event-ID header is sent on all reconnect paths (both chat-store resumeStream and SseConnection retryConnect)

---
*Phase: 49-network-resilience*
*Completed: 2026-04-09*
