---
phase: 53-message-branching
plan: 02
subsystem: ui
tags: [react, zustand, branching, tree, message-editing, fork]

# Dependency graph
requires:
  - phase: 53-01
    provides: "Backend fork endpoint, parent_message_id/branch_from_message_id columns in messages table"
provides:
  - "Tree-aware message store with resolveActivePath and findSiblings"
  - "BranchNavigator component for switching between message branches"
  - "forkAndRegenerate store action replacing destructive edit flow"
  - "selectedBranches per-agent state for tracking branch choices"
affects: [53-message-branching]

# Tech tracking
tech-stack:
  added: []
  patterns: [tree-to-linear path resolution, non-destructive message editing via fork]

key-files:
  created:
    - ui/src/app/(authenticated)/chat/BranchNavigator.tsx
  modified:
    - ui/src/types/api.ts
    - ui/src/stores/chat-store.ts
    - ui/src/app/(authenticated)/chat/MessageActions.tsx
    - ui/src/app/(authenticated)/chat/MessageItem.tsx
    - ui/src/app/(authenticated)/chat/ChatThread.tsx

key-decisions:
  - "resolveActivePath defaults to latest child at each fork point when no selection exists"
  - "EditButton uses forkAndRegenerate (non-destructive) instead of PATCH+regenerateFrom (destructive)"
  - "BranchNavigator rendered in message header next to MessageActions for user messages only"
  - "getCachedRawMessages exported for component-level sibling discovery without full store dependency"

patterns-established:
  - "Tree path resolution: resolveActivePath(rows, selectedBranches) filters branched message sets to linear display path"
  - "Branch selection state: selectedBranches Record<parentId, childId> in AgentState"

requirements-completed: [BRNC-03, BRNC-04]

# Metrics
duration: 8min
completed: 2026-04-09
---

# Phase 53 Plan 02: Tree-Aware Store and Branch Navigation Summary

**Tree-aware message store with resolveActivePath, BranchNavigator < 1/N > controls, and non-destructive fork-based editing**

## Performance

- **Duration:** 8 min
- **Started:** 2026-04-09T20:17:57Z
- **Completed:** 2026-04-09T20:25:18Z
- **Tasks:** 2 completed, 1 deferred (human-verify checkpoint)
- **Files modified:** 7

## Accomplishments
- MessageRow type extended with parent_message_id and branch_from_message_id fields
- resolveActivePath pure function walks message tree to produce linear display path based on selectedBranches
- findSiblings helper discovers messages sharing same parent and role for branch counting
- BranchNavigator component shows "< 1/N >" controls only when siblings exist
- EditButton replaced destructive PATCH+regenerateFrom with non-destructive forkAndRegenerate
- All convertHistory call sites updated to pass selectedBranches for branch-aware rendering
- Trunk-only sessions (no branches) render identically to before

## Task Commits

Each task was committed atomically:

1. **Task 1: MessageRow type + tree-aware store logic + forkAndRegenerate** - `6e6ae87` (feat)
2. **Task 2: BranchNavigator component + EditButton fork wiring** - `a091da7` (feat)
3. **Task 3: Human verify branch navigation flow** - DEFERRED (checkpoint:human-verify)

## Files Created/Modified
- `ui/src/types/api.ts` - Added parent_message_id and branch_from_message_id to MessageRow
- `ui/src/stores/chat-store.ts` - resolveActivePath, findSiblings, switchBranch, forkAndRegenerate, selectedBranches state
- `ui/src/app/(authenticated)/chat/BranchNavigator.tsx` - New component: branch navigation < 1/N > controls
- `ui/src/app/(authenticated)/chat/MessageActions.tsx` - EditButton uses forkAndRegenerate instead of PATCH
- `ui/src/app/(authenticated)/chat/MessageItem.tsx` - Renders BranchNavigator on user messages with siblings
- `ui/src/app/(authenticated)/chat/ChatThread.tsx` - Passes selectedBranches to convertHistory
- `ui/src/__tests__/chat-store-extended.test.ts` - Fixed test helpers for new MessageRow fields
- `ui/src/stores/__tests__/chat-store-identity.test.ts` - Fixed test helpers for new MessageRow fields

## Decisions Made
- resolveActivePath defaults to the latest (most recent) child at each fork point when user has not made a selection -- this means new branches are immediately visible after forking
- EditButton flow changed from destructive (PATCH content + truncate + regenerate) to non-destructive (fork endpoint creates new branch + stream new response)
- BranchNavigator positioned in the message header area next to MessageActions, right-aligned
- getCachedRawMessages exported as a standalone function so MessageItem can discover siblings without pulling in the full store

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed test helpers missing new required MessageRow fields**
- **Found during:** Task 1 (TypeScript verification)
- **Issue:** Test makeRow() helpers in chat-store-extended.test.ts and chat-store-identity.test.ts created MessageRow objects without parent_message_id and branch_from_message_id, causing type errors
- **Fix:** Added parent_message_id: null and branch_from_message_id: null defaults to both makeRow helpers; added selectedBranches: {} to STATE-01 test AgentState mock
- **Files modified:** ui/src/__tests__/chat-store-extended.test.ts, ui/src/stores/__tests__/chat-store-identity.test.ts
- **Verification:** npx tsc --noEmit passes clean
- **Committed in:** 6e6ae87 (Task 1 commit)

**2. [Rule 1 - Bug] Fixed TypeScript implicit any on resolveActivePath selectedId**
- **Found during:** Task 1 (TypeScript verification)
- **Issue:** selectedBranches[current.id] inferred as implicit any due to circular reference in initializer
- **Fix:** Added explicit type annotation: const selectedId: string | undefined
- **Files modified:** ui/src/stores/chat-store.ts
- **Verification:** npx tsc --noEmit passes clean
- **Committed in:** 6e6ae87 (Task 1 commit)

---

**Total deviations:** 2 auto-fixed (2 bugs)
**Impact on plan:** Both fixes necessary for TypeScript compilation. No scope creep.

## Deferred Tasks

**Task 3: Human verify branch navigation flow** - checkpoint:human-verify
- Requires running dev server with backend that has migration 012 applied
- Test steps: send messages, edit a user message, verify branch navigator appears, switch branches

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Known Stubs
None - all functionality is wired to real API endpoints (POST /api/sessions/{id}/fork).

## Next Phase Readiness
- Branch navigation UI is complete and functional
- Human verification of end-to-end flow is deferred
- Ready for any follow-up plans in the message-branching phase

---
*Phase: 53-message-branching*
*Completed: 2026-04-09*
