---
phase: 51-human-in-the-loop
plan: 03
subsystem: ui
tags: [approval-card, countdown, codemirror, i18n, chat-ui]

requires:
  - phase: 51-human-in-the-loop plan 02
    provides: ApprovalPart type, SSE handlers, decideApproval API helper
provides:
  - ApprovalCard component with 5 interactive states (pending, editing, approved, rejected, timed_out)
  - ApprovalCountdown timer with M:SS format and color transition at 30s
  - ApprovalArgsEditor with CodeMirror JSON editing and validation
  - MessageItem renderPart wiring for approval type
  - i18n keys in en.json and ru.json (13 keys each)
affects: []

tech-stack:
  added: []
  patterns: [approval-card-state-machine, countdown-timer-with-aria, codemirror-json-editor-reuse]

key-files:
  created:
    - ui/src/components/chat/ApprovalCard.tsx
    - ui/src/components/chat/ApprovalCountdown.tsx
    - ui/src/components/chat/ApprovalArgsEditor.tsx
  modified:
    - ui/src/app/(authenticated)/chat/MessageItem.tsx
    - ui/src/i18n/locales/en.json
    - ui/src/i18n/locales/ru.json

key-decisions:
  - "ApprovalCard does not locally track status — reacts to part.status prop changes from SSE events"
  - "Countdown aria-live updates every 30s to avoid screen reader noise; visual display updates every 1s"
  - "Args editor reuses CodeEditor component from workspace with json language mode"

patterns-established:
  - "Approval card follows same border/dot/label pattern as ToolCallPartView for visual consistency"
  - "Resolved states collapse to single-line summary with role=status for accessibility"

requirements-completed: [HITL-01, HITL-02]

duration: 3min
completed: 2026-04-09
---

# Phase 51 Plan 03: Inline Approval UI Components Summary

**ApprovalCard with 5 interactive states, ApprovalCountdown timer, ApprovalArgsEditor with CodeMirror JSON validation, MessageItem wiring, and full en/ru i18n**

## Performance

- **Duration:** 3 min
- **Started:** 2026-04-09T19:23:23Z
- **Completed:** 2026-04-09T19:26:26Z
- **Tasks:** 2
- **Files created:** 3
- **Files modified:** 3

## Accomplishments
- ApprovalCard renders inline in chat feed with 5 states per UI-SPEC: pending (interactive with approve/reject/edit-args buttons), editing (CodeMirror JSON editor), approved (collapsed single-line), rejected (collapsed), timed_out (collapsed)
- ApprovalCountdown shows M:SS format countdown, switches from text-warning to text-destructive at 30 seconds remaining
- ApprovalArgsEditor wraps CodeMirror with JSON validation, Submit modified disabled on invalid JSON, Escape key cancels
- MessageItem renderPart switch now handles "approval" case, routing to ApprovalCard
- 13 i18n keys added to both en.json and ru.json matching UI-SPEC copywriting contract

## Task Commits

Each task was committed atomically:

1. **Task 1: Create ApprovalCountdown, ApprovalArgsEditor, and ApprovalCard** - `f107cd9` (feat)
2. **Task 2: Wire ApprovalCard into MessageItem and add i18n** - `d2ff8e6` (feat)

## Files Created/Modified
- `ui/src/components/chat/ApprovalCard.tsx` - Main approval card with pending/editing/resolved states, decideApproval API calls
- `ui/src/components/chat/ApprovalCountdown.tsx` - Countdown timer with M:SS format, 1s interval, aria-live polite updates every 30s
- `ui/src/components/chat/ApprovalArgsEditor.tsx` - CodeMirror JSON editor with validation, submit/cancel buttons
- `ui/src/app/(authenticated)/chat/MessageItem.tsx` - Added approval case to renderPart, imported ApprovalCard
- `ui/src/i18n/locales/en.json` - 13 approval-related i18n keys
- `ui/src/i18n/locales/ru.json` - 13 approval-related Russian translations

## Decisions Made
- ApprovalCard does not locally update status on approve/reject — the SSE tool-approval-resolved event updates ApprovalPart.status via the store, and the card re-renders from the updated prop
- Countdown uses split aria pattern: visible text updates every 1s (aria-hidden), screen reader text updates every 30s (aria-live polite) to avoid noise
- Reused existing CodeEditor component rather than creating a new CodeMirror instance for consistency

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None.

## Known Stubs
None - all components are fully wired to real data sources and API calls.

## User Setup Required
None.

## Next Phase Readiness
- All approval UI components ready for end-to-end testing with backend SSE events
- No blockers

---
*Phase: 51-human-in-the-loop*
*Completed: 2026-04-09*
