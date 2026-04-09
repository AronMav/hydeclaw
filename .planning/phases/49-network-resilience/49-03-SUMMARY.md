---
phase: 49-network-resilience
plan: 03
subsystem: ui
tags: [react, reconnect, sse, accessibility, vitest]

requires:
  - phase: 49-02
    provides: "reconnectAttempt and maxReconnectAttempts fields in AgentState, connectionPhase reconnecting state"
provides:
  - "ReconnectingIndicator component with pulsating dot, attempt counter, aria attributes"
  - "ChatThread integration rendering indicator during reconnecting phase"
affects: []

tech-stack:
  added: []
  patterns: ["Inline reconnect status indicator with PulseDotLoader reuse"]

key-files:
  created:
    - ui/src/components/chat/ReconnectingIndicator.tsx
    - ui/src/__tests__/reconnecting-indicator.test.tsx
  modified:
    - ui/src/app/(authenticated)/chat/ChatThread.tsx

key-decisions:
  - "Indicator placed between MessageList and ErrorBanner in ChatThread (not inside Virtuoso list)"
  - "Test assertion for /3 uses regex match since attempt count spans parent text node"

patterns-established:
  - "ReconnectingIndicator: role=status + aria-live=polite pattern for transient connection state"

requirements-completed: [NET-02]

duration: 2min
completed: 2026-04-09
---

# Phase 49 Plan 03: Reconnecting Indicator Summary

**ReconnectingIndicator component with PulseDotLoader, attempt counter, and full accessibility attrs wired into ChatThread on reconnecting phase**

## Performance

- **Duration:** 2 min
- **Started:** 2026-04-09T17:53:54Z
- **Completed:** 2026-04-09T17:56:15Z
- **Tasks:** 1/2 (Task 2 deferred: human-verify checkpoint)
- **Files modified:** 3

## Accomplishments
- Created ReconnectingIndicator component with pulsating dot, animated ellipsis, and attempt counter
- Wired into ChatThread with store selectors for reconnectAttempt and maxReconnectAttempts
- Added 4 vitest tests covering rendering, attempt count display, aria-label, and aria-live

## Task Commits

Each task was committed atomically:

1. **Task 1: Create ReconnectingIndicator component and wire into ChatThread** - `dd15f2b` (feat)
2. **Task 2: Human verification of reconnect UX** - DEFERRED (checkpoint:human-verify)

## Files Created/Modified
- `ui/src/components/chat/ReconnectingIndicator.tsx` - Inline reconnecting status with PulseDotLoader, attempt counter, accessibility
- `ui/src/__tests__/reconnecting-indicator.test.tsx` - 4 tests for component rendering and accessibility
- `ui/src/app/(authenticated)/chat/ChatThread.tsx` - Import and render ReconnectingIndicator when connectionPhase is reconnecting

## Decisions Made
- Indicator placed between MessageList and ErrorBanner in ChatThread layout (outside Virtuoso scroll list) for simplicity and no scroll disruption
- Test for attempt count uses regex `/\/3/` since the "/3" text shares a parent node with "(attempt " prefix

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed test assertion for attempt count display**
- **Found during:** Task 1 (TDD GREEN phase)
- **Issue:** Test used `screen.getByText("/3")` but "/3" is not a standalone text node in the rendered DOM
- **Fix:** Changed to `screen.getByText(/\/3/)` regex matcher
- **Files modified:** ui/src/__tests__/reconnecting-indicator.test.tsx
- **Verification:** All 4 tests pass
- **Committed in:** dd15f2b (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Minor test assertion fix. No scope creep.

## Deferred Tasks

**Task 2: Human verification of reconnect UX** (checkpoint:human-verify)
- Requires manual browser testing: simulate network drop during streaming, verify indicator appearance
- Should be performed when full end-to-end environment is available

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- ReconnectingIndicator component complete and tested
- Human verification of full reconnect UX flow deferred
- All NET-02 code artifacts in place

---
*Phase: 49-network-resilience*
*Completed: 2026-04-09*
