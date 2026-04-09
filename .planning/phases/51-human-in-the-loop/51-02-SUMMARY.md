---
phase: 51-human-in-the-loop
plan: 02
subsystem: ui
tags: [sse, approval, zustand, chat-store, typescript]

requires:
  - phase: 51-human-in-the-loop plan 01
    provides: backend SSE approval events and /api/approvals/{id}/resolve endpoint
provides:
  - SSE event parsing for tool-approval-needed and tool-approval-resolved
  - ApprovalPart message type in chat store MessagePart union
  - Chat store SSE handlers that create and update ApprovalPart in message parts
  - decideApproval API helper for calling POST /api/approvals/{id}/resolve
affects: [51-human-in-the-loop plan 03]

tech-stack:
  added: []
  patterns: [approval-part-in-message-parts, approval-sse-event-handler]

key-files:
  created: []
  modified:
    - ui/src/stores/sse-events.ts
    - ui/src/stores/chat-store.ts
    - ui/src/lib/api.ts

key-decisions:
  - "ApprovalPart uses receivedAt: number (Date.now()) for countdown timer calculation"
  - "decideApproval returns { ok, error? } instead of throwing for caller-friendly error handling"
  - "tool-approval-resolved updates existing ApprovalPart in-place by approvalId lookup"

patterns-established:
  - "ApprovalPart pattern: SSE event creates part with pending status, resolved event updates status field"
  - "Approval API uses apiFetch internally (not raw fetch) for consistent auth headers"

requirements-completed: [HITL-01, HITL-02]

duration: 2min
completed: 2026-04-09
---

# Phase 51 Plan 02: SSE Approval Events and Chat Store Summary

**ApprovalPart message type with SSE event parsing, chat store handlers, and decideApproval API helper for inline tool approval flow**

## Performance

- **Duration:** 2 min
- **Started:** 2026-04-09T21:27:41Z
- **Completed:** 2026-04-09T21:29:47Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- SSE event parser recognizes tool-approval-needed and tool-approval-resolved with full validation
- ApprovalPart interface with status, receivedAt, timeoutMs, modifiedInput fields added to MessagePart union
- Chat store SSE handler creates pending ApprovalPart on approval-needed and updates status on resolution
- decideApproval API helper sends POST to /api/approvals/{id}/resolve with status, resolved_by, and optional modified_input

## Task Commits

Each task was committed atomically:

1. **Task 1: Add approval SSE event types and ApprovalPart to store** - `0400799` (feat)
2. **Task 2: Add decideApproval API helper** - `4870462` (feat)

## Files Created/Modified
- `ui/src/stores/sse-events.ts` - Added tool-approval-needed and tool-approval-resolved to SseEvent union and parseSseEvent cases
- `ui/src/stores/chat-store.ts` - Added ApprovalPart interface, MessagePart union member, and SSE event handler cases
- `ui/src/lib/api.ts` - Added decideApproval function using apiFetch for auth-consistent API calls

## Decisions Made
- Used `apiFetch` (internal helper) rather than raw `fetch` for `decideApproval` to get automatic auth headers and 401 handling
- `decideApproval` returns `{ ok, error? }` rather than throwing to let UI components handle errors gracefully with toast
- `ApprovalPart.receivedAt` stores `Date.now()` timestamp for countdown timer calculation in the UI component (Plan 03)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- ApprovalPart type and SSE handling ready for Plan 03 (ApprovalCard UI component)
- decideApproval API helper ready for wiring to Approve/Reject buttons
- No blockers for next plan

---
*Phase: 51-human-in-the-loop*
*Completed: 2026-04-09*
