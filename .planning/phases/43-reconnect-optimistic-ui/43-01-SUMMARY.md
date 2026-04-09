---
phase: 43-reconnect-optimistic-ui
plan: 01
subsystem: ui
tags: [sse, reconnect, exponential-backoff, connection-phase, typescript, vitest]

# Dependency graph
requires:
  - phase: 40-sseconnection-extraction
    provides: SseConnection class (pure SSE transport layer)
  - phase: 41-connectionphase-fsm
    provides: ConnectionPhase FSM in chat-store
provides:
  - SseConnection with scheduleReconnect and exponential backoff (1s/2s/4s)
  - SseConnectionCallbacksWithPhase interface with onPhaseChange callback
  - setSessionId() public method for reconnect target
  - ConnectionPhase extended with "reconnecting" value
  - processSSEStream connection-drop detection via receivedFinishEvent
  - scheduleReconnect() in chat-store with per-agent timer tracking
  - isActivePhase updated to include "reconnecting"
affects: [44-optimistic-ui, 45-cleanup, chat-store-consumers]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "TDD RED-GREEN: failing tests committed before implementation"
    - "receivedFinish flag distinguishes natural stream end from connection drop"
    - "reconnectTimers Record cleared on user abort to prevent retry-after-stop"
    - "onPhaseChange optional callback pattern (SseConnectionCallbacks remains backward-compat)"

key-files:
  created: []
  modified:
    - ui/src/lib/sse-connection.ts
    - ui/src/stores/chat-store.ts
    - ui/src/__tests__/sse-connection.test.ts

key-decisions:
  - "onPhaseChange added as SseConnectionCallbacksWithPhase (extends base callbacks) — backward-compat, callers without phase tracking unaffected"
  - "receivedFinishEvent tracked per-processSSEStream invocation to detect drops vs natural end"
  - "reconnectTimers cleared in both abortActiveStream() and stopStream() — belt-and-suspenders for user abort"
  - "resumeStream accepts reconnectAttempt param so retry chain tracks depth across resumeStream hops"
  - "No reconnect in SseConnection when sessionId not set — prevents reconnect before data-session-id received"

patterns-established:
  - "Connection drop detection: done=true in reader loop without finish event + non-aborted signal = scheduleReconnect"
  - "Exponential backoff: 1000 * Math.pow(2, attempt) ms delays (1s, 2s, 4s)"
  - "Timer tracking: per-agent reconnectTimers Record cleared on any abort path"

requirements-completed: [SSE-02]

# Metrics
duration: 4min
completed: 2026-04-09
---

# Phase 43 Plan 01: Reconnect + Optimistic UI Summary

**SSE reconnect with exponential backoff (1s/2s/4s) added to SseConnection class and wired into chat-store via ConnectionPhase="reconnecting" and processSSEStream drop detection**

## Performance

- **Duration:** 4 min
- **Started:** 2026-04-09T16:27:37Z
- **Completed:** 2026-04-09T16:31:57Z
- **Tasks:** 2 (TDD: 3 commits)
- **Files modified:** 3

## Accomplishments

- SseConnection class gains reconnect capability: `scheduleReconnect()`, `retryConnect()`, `setSessionId()`, `onPhaseChange` callback, `receivedFinish` tracking
- `ConnectionPhase` type extended with `"reconnecting"` — `isActivePhase` includes it so thinking indicator stays active during retry
- `processSSEStream` detects connection drops (reader done without finish event) and schedules exponential backoff reconnect via `scheduleReconnect()`
- `reconnectTimers` Record ensures user abort (`stopStream`) always cancels pending retry
- 25 unit tests all pass, 398 total test suite passes, production build succeeds with no type errors

## Task Commits

1. **Task 1 RED: Failing tests for SseConnection reconnect** - `fb556cc` (test)
2. **Task 1 GREEN: SseConnection reconnect implementation** - `c1711d1` (feat)
3. **Task 2: Wire reconnect phases into chat-store** - `bfab9f4` (feat)

## Files Created/Modified

- `ui/src/lib/sse-connection.ts` - Added `scheduleReconnect()`, `retryConnect()`, `setSessionId()`, `SseConnectionCallbacksWithPhase`, `maxRetries` config
- `ui/src/stores/chat-store.ts` - Extended `ConnectionPhase`, added `MAX_RECONNECT_ATTEMPTS`, `RECONNECT_DELAY_BASE_MS`, `reconnectTimers`, `scheduleReconnect()`, `receivedFinishEvent` tracking, updated `resumeStream()` and `abortActiveStream()` and `stopStream()`
- `ui/src/__tests__/sse-connection.test.ts` - 10 new reconnect lifecycle tests with `vi.useFakeTimers()`

## Decisions Made

- `onPhaseChange` is on a new `SseConnectionCallbacksWithPhase` interface that extends `SseConnectionCallbacks` — callers without phase tracking pass the old interface unmodified
- `receivedFinishEvent` flag (not a "session received" flag) is the correct discriminator for drop vs natural end
- `reconnectTimers` cleared in both `abortActiveStream()` and `stopStream()` for belt-and-suspenders abort safety
- `resumeStream` reconnect attempt counter chains across retries so max-3 applies globally per stream lifetime

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## Known Stubs

None - all data paths wired through. Reconnect will fire on connection drop and `connectionPhase="reconnecting"` is visible in the store.

## Next Phase Readiness

- Phase 43-02 (optimistic UI) can now use `connectionPhase === "reconnecting"` to show a reconnect indicator in the UI
- `isActivePhase` already includes `"reconnecting"` so existing thinking indicator stays active during retry

---
*Phase: 43-reconnect-optimistic-ui*
*Completed: 2026-04-09*
