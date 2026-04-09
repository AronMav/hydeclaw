---
phase: 44-ux-polish
plan: "01"
subsystem: ui
tags: [react, localStorage, virtuoso, scroll, draft-persistence]

requires:
  - phase: 43-reconnect-optimistic-ui
    provides: SseConnection with phase callbacks and optimistic UI via useOptimistic

provides:
  - Draft persistence per agent via localStorage with hydeclaw.draft.{agent} keys
  - saveDraft/loadDraft/clearDraft exported helpers in ChatThread.tsx
  - Single followOutput scroll authority in MessageList (totalPartsCount effect removed)

affects: [44-02-ux-polish, future chat UI phases]

tech-stack:
  added: []
  patterns:
    - "Draft persistence via localStorage.setItem/removeItem with DRAFT_PREFIX + agent key"
    - "Uncontrolled textarea draft restore via HTMLTextAreaElement.prototype.value setter + input event dispatch"
    - "Virtuoso followOutput as single scroll authority — no competing useEffect on content count"

key-files:
  created:
    - ui/src/__tests__/draft-persistence.test.ts
  modified:
    - ui/src/app/(authenticated)/chat/ChatThread.tsx
    - ui/src/app/(authenticated)/chat/MessageList.tsx

key-decisions:
  - "saveDraft(agent, '') removes the key instead of storing empty string — no stale localStorage entries"
  - "Draft restore uses HTMLTextAreaElement.prototype.value native setter + bubbled input event to trigger React synthetic handlers"
  - "totalPartsCount useMemo and its Stage 3 Fix useEffect removed entirely — followOutput callback already handles all scroll cases correctly"

patterns-established:
  - "Draft helpers exported as named functions for direct unit testing without component mounting"

requirements-completed: [UX-01, UX-02]

duration: 3min
completed: "2026-04-09"
---

# Phase 44 Plan 01: UX Polish — Draft Persistence + Scroll Authority Summary

**Draft text persists per-agent in localStorage (hydeclaw.draft.{agent}) and Virtuoso followOutput is now the sole scroll authority after removing the competing totalPartsCount effect.**

## Performance

- **Duration:** 3 min
- **Started:** 2026-04-09T12:44:03Z
- **Completed:** 2026-04-09T12:47:05Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments

- Added `saveDraft`, `loadDraft`, `clearDraft` pure helper functions exported from ChatThread.tsx
- Wired draft save/restore into ChatComposer: saves on input, restores on mount/agent-switch, clears on submit
- Removed `totalPartsCount` useMemo + Stage 3 Fix useEffect from MessageList — followOutput is the single scroll authority
- 6 new unit tests for draft helpers, all passing; 411 total tests pass with no regressions

## Task Commits

Each task was committed atomically:

1. **test(44-01) — RED: failing draft tests** - `15e42b9` (test)
2. **Task 1: Draft persistence in ChatComposer** - `4c5c7d2` (feat)
3. **Task 2: Consolidate scroll to single followOutput authority** - `fbad294` (feat)

_Note: Task 1 used TDD — test commit (RED) then implementation commit (GREEN)._

## Files Created/Modified

- `ui/src/__tests__/draft-persistence.test.ts` — 6 unit tests for saveDraft/loadDraft/clearDraft
- `ui/src/app/(authenticated)/chat/ChatThread.tsx` — draft helpers + ChatComposer useEffect/input/submit wiring
- `ui/src/app/(authenticated)/chat/MessageList.tsx` — removed totalPartsCount useMemo and its competing useEffect

## Decisions Made

- Used `HTMLTextAreaElement.prototype.value` native setter for draft restore (same pattern already used for mention insertion in the same component) — consistent, React-compatible
- Clearing textarea on agent-switch even when no draft exists prevents stale text showing briefly
- `saveDraft(agent, "")` calls `removeItem` instead of storing empty string — keeps localStorage clean

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Draft persistence and scroll consolidation complete
- Phase 44-02 (error UI polish) is independent and can proceed
- No blockers

---
*Phase: 44-ux-polish*
*Completed: 2026-04-09*
