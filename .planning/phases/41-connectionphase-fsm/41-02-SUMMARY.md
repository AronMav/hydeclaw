---
phase: 41-connectionphase-fsm
plan: "02"
subsystem: ui/chat
tags: [fsm, connection-phase, thinking-indicator, session-storage-cleanup]
dependency_graph:
  requires: [ConnectionPhase type from 41-01]
  provides: [showThinking driven by connectionPhase, isActivePhase in UI components]
  affects:
    - ui/src/app/(authenticated)/chat/ChatThread.tsx
    - ui/src/app/(authenticated)/chat/page.tsx
tech_stack:
  added: []
  patterns: [single-source showThinking from FSM, isActivePhase replaces isActiveStream at component boundaries]
key_files:
  created: []
  modified:
    - ui/src/app/(authenticated)/chat/ChatThread.tsx
    - ui/src/app/(authenticated)/chat/page.tsx
    - ui/src/__tests__/chat-input.test.tsx
    - ui/src/__tests__/pages-smoke.test.tsx
decisions:
  - showThinking = connectionPhase === "submitted" || engineRunning (removed 4-signal expression)
  - isPersistedStreaming state and sessionStorage.getItem fallback completely removed from ChatThread
  - Test mocks updated to export isActivePhase alongside isActiveStream for backward compat
metrics:
  duration_minutes: 8
  completed_date: "2026-04-09T11:47:40Z"
  tasks_completed: 2
  files_modified: 4
---

# Phase 41 Plan 02: ConnectionPhase FSM Summary

**One-liner:** showThinking replaced with single-source `connectionPhase === "submitted" || engineRunning`, removing the sessionStorage fallback and 4-signal expression from ChatThread.

## What Was Built

### Task 1: Replace showThinking with connectionPhase-derived value (ChatThread.tsx)

**Before:**
```typescript
const showThinking = streamStatus === "submitted"
  || engineRunning
  || (isStreaming && !!pendingTarget)
  || (isPersistedStreaming && !historyLoading);
```

**After:**
```typescript
const showThinking = connectionPhase === "submitted" || engineRunning;
```

Changes:
- `streamStatus` selector replaced with `connectionPhase` selector (reads `AgentState.connectionPhase`)
- `isActiveStream` replaced with `isActivePhase` (imported from `@/stores/chat-store`)
- `engineRunning` updated to use `!isActivePhase(connectionPhase)`
- `useSessionMessages` guard updated to `isActivePhase(connectionPhase) ? null : activeSessionId`
- `isStreaming` derived as `isActivePhase(connectionPhase)` instead of string comparison
- `isPersistedStreaming` state + `useEffect` reading `sessionStorage.getItem` removed entirely
- `pendingTarget` selector removed (no longer used in showThinking)
- `ChatComposer` function's `streamStatus` selector also replaced with `connectionPhase`

### Task 2: Update page.tsx to use connectionPhase

- Line 89: `isStreaming` derived using `isActivePhase(connectionPhase)` instead of `isActiveStream(streamStatus)`
- Line 124: Session restore guard uses `isActivePhase(agentState?.connectionPhase)`
- Line 193: History view refresh guard uses `!isActivePhase(agentState.connectionPhase)`
- `isActiveStream` import removed, `isActivePhase` added to import

## Decisions Made

- **isActiveStream kept in store exports**: The function is still exported from `chat-store.ts` for external callers; removed only from ChatThread and page.tsx component usage
- **Test mocks updated**: Both `chat-input.test.tsx` and `pages-smoke.test.tsx` mocks now export `isActivePhase: () => false` alongside `isActiveStream` (auto-fixed per Rule 1)

## Verification Results

- `grep -c "isPersistedStreaming" ui/src/app/(authenticated)/chat/ChatThread.tsx` → 0
- `grep -c "sessionStorage.getItem" ui/src/app/(authenticated)/chat/ChatThread.tsx` → 0
- `grep "showThinking" ...ChatThread.tsx` → `connectionPhase === "submitted" || engineRunning`
- `grep -c "connectionPhase" ...page.tsx` → 3
- `grep -c "isActivePhase" ...page.tsx` → 4
- `grep -c "isActiveStream" ...page.tsx` → 0
- Build passes, all 388 tests pass

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Test mocks missing isActivePhase export**
- **Found during:** Task 2 build/test verification
- **Issue:** `chat-input.test.tsx` and `pages-smoke.test.tsx` both mock `@/stores/chat-store` and exported `isActiveStream` but not `isActivePhase`. After replacing the import in `page.tsx`, vitest threw "No isActivePhase export is defined on the mock".
- **Fix:** Added `isActivePhase: () => false` to both test mocks, alongside the existing `isActiveStream` entry.
- **Files modified:** `ui/src/__tests__/chat-input.test.tsx`, `ui/src/__tests__/pages-smoke.test.tsx`
- **Commit:** e645c0a

## Known Stubs

None — ThinkingMessage is driven by live connectionPhase state with no placeholder logic.

## Self-Check: PASSED

Files modified:
- FOUND: ui/src/app/(authenticated)/chat/ChatThread.tsx (connectionPhase, isActivePhase, showThinking single-source)
- FOUND: ui/src/app/(authenticated)/chat/page.tsx (isActivePhase, connectionPhase)
- FOUND: ui/src/__tests__/chat-input.test.tsx (isActivePhase mock)
- FOUND: ui/src/__tests__/pages-smoke.test.tsx (isActivePhase mock)

Commits:
- 3e04f68: feat(41-02): replace 4-signal showThinking with connectionPhase-derived value
- e645c0a: feat(41-02): update page.tsx to use connectionPhase and isActivePhase
