---
phase: 42-history-messagesource
plan: "01"
subsystem: ui
tags: [zustand, typescript, react, chat-store, sse]

# Dependency graph
requires:
  - phase: 41-connectionphase-fsm
    provides: ConnectionPhase FSM, IncrementalParser, streamGeneration as module-global

provides:
  - MessageSource discriminated union type (new-chat | live | history) in chat-store.ts
  - Per-agent streamGenerations Record replacing module-scope counter
  - All UI components migrated from viewMode/liveMessages to messageSource

affects:
  - 42-02 (F5 restore plan depends on messageSource for atomic history/live transitions)
  - Any future phase touching chat state management

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "MessageSource discriminated union encodes all three chat view states atomically"
    - "Per-agent generation counter: streamGenerations[agent] prevents cross-agent stream killing"
    - "getLiveMessages(source) helper extracts messages without if/else scattered in consumers"

key-files:
  created: []
  modified:
    - ui/src/stores/chat-store.ts
    - ui/src/app/(authenticated)/chat/ChatThread.tsx
    - ui/src/app/(authenticated)/chat/page.tsx
    - ui/src/app/(authenticated)/chat/MessageActions.tsx
    - ui/src/__tests__/sse-stream.test.ts
    - ui/src/__tests__/chat-store-extended.test.ts
    - ui/src/__tests__/session-management.test.tsx
    - ui/src/__tests__/chat-input.test.tsx
    - ui/src/__tests__/message-list.test.tsx
    - ui/src/__tests__/multi-agent-identity.test.tsx
    - ui/src/__tests__/pages-smoke.test.tsx

key-decisions:
  - "MessageSource { mode: 'new-chat' | 'live' (+ messages) | 'history' (+ sessionId) } eliminates liveMessages+viewMode dual-semantics"
  - "Per-agent streamGenerations[agent] — switching agents no longer resets the global counter and kills parallel streams"
  - "saveUiState backward-compat: serializes viewMode as 'history'|'live' for backend UI state API"
  - "getLiveMessages() helper added to reduce messageSource.mode checks scattered across store actions"

patterns-established:
  - "messageSource replaces viewMode+liveMessages across all Zustand store actions and UI selectors"
  - "startStream atomically sets { mode: 'live', messages: [...seedMessages, userMsg] } — no empty live transition"

requirements-completed: [HIST-02, HIST-03]

# Metrics
duration: 25min
completed: 2026-04-09
---

# Phase 42 Plan 01: MessageSource + Per-Agent StreamGenerations Summary

**MessageSource discriminated union (new-chat|live|history) replaces viewMode+liveMessages in AgentState, and per-agent streamGenerations Record eliminates concurrent stream-killing**

## Performance

- **Duration:** ~25 min
- **Started:** 2026-04-09T16:00:00Z
- **Completed:** 2026-04-09T16:15:00Z
- **Tasks:** 2
- **Files modified:** 11

## Accomplishments
- Added `MessageSource` discriminated union type replacing dual-semantics `viewMode + liveMessages` fields in `AgentState`
- Replaced module-scope `let streamGeneration = 0` with `const streamGenerations: Record<string, number> = {}` so switching agents no longer resets the counter and kills a parallel stream
- Migrated all 15+ store action sites (startStream, resumeStream, selectSession, newChat, sendMessage, regenerate, regenerateFrom, deleteSession, deleteAllSessions, deleteMessage, exportSession, setCurrentAgent, selectSessionById, pushUpdate, sync event handler)
- Migrated all 3 component files (ChatThread.tsx, page.tsx, MessageActions.tsx) from `viewMode` selectors to `messageSource` selectors
- Updated all 7 test files to use `messageSource` in mock state objects

## Task Commits

1. **Task 1: Add MessageSource type and per-agent streamGenerations; migrate store internals** - `38fb96c` (feat)
2. **Task 2: Migrate ChatThread.tsx, page.tsx, MessageActions.tsx from viewMode to messageSource** - `53e8a0b` (feat)

## Files Created/Modified
- `ui/src/stores/chat-store.ts` - MessageSource type, getLiveMessages helper, AgentState migration, per-agent streamGenerations
- `ui/src/app/(authenticated)/chat/ChatThread.tsx` - messageSource selector replaces viewMode+liveMessages in ChatComposer and ChatThread functions
- `ui/src/app/(authenticated)/chat/page.tsx` - messageSource replaces viewMode in session restore effect and WS handler
- `ui/src/app/(authenticated)/chat/MessageActions.tsx` - messageSource replaces viewMode for delete button visibility
- `ui/src/__tests__/sse-stream.test.ts` - Access st.messageSource.messages instead of st.liveMessages
- `ui/src/__tests__/chat-store-extended.test.ts` - Updated STATE-01 tests to use messageSource; fixed static analysis test slice to use implementation block not interface
- `ui/src/__tests__/session-management.test.tsx` - Updated local test store types and assertions to messageSource
- Test mocks in chat-input, message-list, multi-agent-identity, pages-smoke - messageSource replaces viewMode+liveMessages

## Decisions Made
- `saveUiState` backward-compat: serializes `viewMode: st.messageSource.mode === "history" ? "history" : "live"` to preserve the backend UI state API contract without schema change
- `getLiveMessages(source)` helper added internally (not exported) to reduce boilerplate in store actions
- `data-session-id` event guard was missed (referenced old `streamGeneration`), fixed inline as [Rule 1 - Bug]

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] data-session-id guard still referenced old streamGeneration variable**
- **Found during:** Task 1 verification (test run)
- **Issue:** The `data-session-id` SSE event handler had `if (sid && generation === streamGeneration)` — the old module-scope variable that was removed, causing ReferenceError
- **Fix:** Changed to `generation === streamGenerations[agent]` consistent with all other guard sites
- **Files modified:** ui/src/stores/chat-store.ts
- **Verification:** All sse-stream.test.ts tests pass after fix
- **Committed in:** 38fb96c (part of Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug in missed reference during variable rename)
**Impact on plan:** Critical fix — without it, data-session-id events would always fail the guard after store rename.

## Issues Encountered
- Static analysis test in chat-store-extended.test.ts was slicing from `"sendMessage: (text: string)"` which matched the interface declaration before the implementation. Fixed by searching for `"sendMessage: (text: string) => {"` (the implementation signature with function body).

## Next Phase Readiness
- MessageSource type and per-agent streamGenerations are in place as prerequisites for plan 42-02 (F5 restore)
- No regressions: 388/388 tests pass, build clean

---
*Phase: 42-history-messagesource*
*Completed: 2026-04-09*
