---
phase: 45-cleanup
plan: 01
subsystem: ui
tags: [zustand, typescript, chat-store, refactor, cleanup]

# Dependency graph
requires:
  - phase: 44-ux-polish
    provides: "Final UX polish — chat store at stable state for cleanup"
provides:
  - "chat-store.ts with no deprecated fields (CLN-01) and no module-scope globals (CLN-02)"
  - "ConnectionPhase and isActivePhase as sole stream-state authorities"
  - "streamGeneration per-agent counter inside AgentState"
  - "AbortController and reconnect timers behind encapsulated Map helpers"
affects: [any consumer of chat-store AgentState shape, tests mocking chat-store]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "ConnectionPhase as sole FSM authority — no parallel streamStatus field"
    - "Non-serializable browser objects (AbortController, setTimeout) in private Maps behind accessor helpers, not in Zustand/Immer state"
    - "Serializable counters (streamGeneration) live in AgentState, non-serializable handles live outside"

key-files:
  created: []
  modified:
    - ui/src/stores/chat-store.ts
    - ui/src/__tests__/chat-store.test.ts
    - ui/src/__tests__/chat-store-extended.test.ts
    - ui/src/__tests__/chat-input.test.tsx
    - ui/src/__tests__/pages-smoke.test.tsx
    - ui/src/__tests__/session-management.test.tsx
    - ui/src/__tests__/message-list.test.tsx
    - ui/src/__tests__/multi-agent-identity.test.tsx
    - ui/src/__tests__/sse-stream.test.ts

key-decisions:
  - "CLN-01: StreamStatus type and isActiveStream function removed entirely — ConnectionPhase and isActivePhase are the sole authorities"
  - "CLN-01: thinkingSessionId removed — activeSessionIds array already covers the use case"
  - "CLN-01: sessionStorage hydeclaw.streaming flags removed from finish/sync handlers — no longer needed with ConnectionPhase FSM"
  - "CLN-01: viewMode backward-compat in saveUiState replaced with connectionPhase — backend ui_state carries connectionPhase only"
  - "CLN-02: AbortController kept in private Map (_abortControllers) not AgentState — Immer cannot proxy non-plain objects"
  - "CLN-02: reconnectTimers kept in private Map (_reconnectTimers) not AgentState — same serialization constraint"
  - "CLN-02: streamGeneration moved to AgentState as plain number — serializable, Immer-safe"

patterns-established:
  - "Non-serializable handles: use private Map + accessor helpers, not module-scope Records and not Immer state"
  - "Serializable per-agent counters: use AgentState field with update() helper"

requirements-completed: [CLN-01, CLN-02]

# Metrics
duration: 25min
completed: 2026-04-09
---

# Phase 45 Plan 01: Chat Store Deprecated Fields Cleanup Summary

**Removed StreamStatus/isActiveStream (CLN-01) and module-scope mutable globals (CLN-02) from chat-store.ts, making ConnectionPhase the sole streaming-state authority and encapsulating non-serializable browser objects behind private Map helpers**

## Performance

- **Duration:** 25 min
- **Started:** 2026-04-09T17:00:00Z
- **Completed:** 2026-04-09T17:08:00Z
- **Tasks:** 2
- **Files modified:** 9

## Accomplishments
- Removed `StreamStatus` type, `isActiveStream` function, `streamStatus` field, `thinkingSessionId` field — ConnectionPhase/isActivePhase are the sole stream-state authorities
- Removed all `sessionStorage.removeItem("hydeclaw.streaming.*")` calls (3 locations) and `viewMode` backward-compat serialization from `saveUiState`
- Removed bare module-scope `agentAbortControllers`, `streamGenerations`, and `reconnectTimers` Records — replaced with encapsulated `Map` helpers and per-agent `streamGeneration` in AgentState
- All 411 vitest tests pass, zero TypeScript errors, Next.js production build succeeds

## Task Commits

1. **Task 1: Remove deprecated fields and StreamStatus (CLN-01)** - `2efc776` (refactor)
2. **Task 2: Move module-scope globals into AgentState (CLN-02)** - `0cd15f2` (refactor)

## Files Created/Modified
- `ui/src/stores/chat-store.ts` - Removed StreamStatus/isActiveStream/streamStatus/thinkingSessionId; encapsulated abort/timer globals; moved streamGeneration to AgentState
- `ui/src/__tests__/chat-store.test.ts` - Replaced isActiveStream describe block with isActivePhase tests
- `ui/src/__tests__/chat-store-extended.test.ts` - Removed streamStatus/thinkingSessionId from mock AgentState; added streamGeneration field
- `ui/src/__tests__/chat-input.test.tsx` - Removed isActiveStream export, streamStatus from mock
- `ui/src/__tests__/pages-smoke.test.tsx` - Removed isActiveStream export, streamStatus from mock
- `ui/src/__tests__/session-management.test.tsx` - Replaced streamStatus with connectionPhase in mini-store tests
- `ui/src/__tests__/message-list.test.tsx` - Removed isActiveStream export, streamStatus from mock
- `ui/src/__tests__/multi-agent-identity.test.tsx` - Removed isActiveStream export, streamStatus from mock
- `ui/src/__tests__/sse-stream.test.ts` - Replaced st.streamStatus assertions with st.connectionPhase

## Decisions Made
- AbortController stays outside Zustand state (in private `_abortControllers` Map) — Immer's produce will fail to proxy non-plain objects like AbortController in development mode
- `streamGeneration` counter goes into AgentState because it's a plain `number` (Immer-safe and serializable)
- `reconnectTimers` similarly stays in private `_reconnectTimers` Map — ReturnType<typeof setTimeout> is a Node.js/browser handle, not a plain value

## Deviations from Plan

None - plan executed exactly as written. The fallback encapsulation pattern for AbortController was anticipated in the plan ("if Immer causes issues, use private helper functions") and applied as designed.

## Issues Encountered
- TypeScript error in chat-store-extended.test.ts after adding `streamGeneration` to AgentState — the mock object needed `streamGeneration: 0` added. Fixed inline as part of Task 2.

## Known Stubs

None.

## Next Phase Readiness
- chat-store.ts is fully cleaned up — no dead code paths remain
- v0.12.0 Chat Redesign milestone is complete
- ConnectionPhase is the single source of truth for stream state throughout the codebase

---
*Phase: 45-cleanup*
*Completed: 2026-04-09*
