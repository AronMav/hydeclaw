---
phase: 52-citations-generative-ui
plan: 01
subsystem: ui
tags: [react, rich-card, error-boundary, registry-pattern, generative-ui]

requires: []
provides:
  - "CARD_REGISTRY: static Map<string, CardComponent> for rich-card type dispatch"
  - "GenerativeUISlot: wrapper with ErrorBoundary + JSON fallback for unknown types"
  - "CardErrorBoundary: class component catching broken card renders"
  - "Widened cardType from union to string for extensible backend card types"
affects: [52-02, 52-03]

tech-stack:
  added: []
  patterns: ["Registry pattern for card type -> component dispatch", "ErrorBoundary per-card isolation"]

key-files:
  created:
    - ui/src/components/ui/card-registry.tsx
    - ui/src/__tests__/card-registry.test.tsx
  modified:
    - ui/src/components/ui/rich-card.tsx
    - ui/src/stores/chat-store.ts
    - ui/src/stores/sse-events.ts
    - ui/src/app/(authenticated)/chat/ChatThread.tsx
    - ui/src/__tests__/chat-input.test.tsx
    - ui/src/__tests__/message-list.test.tsx
    - ui/src/__tests__/multi-agent-identity.test.tsx
    - ui/src/__tests__/session-management.test.tsx

key-decisions:
  - "Registry is a mutable Map (allows runtime registration for tests and future plugins)"
  - "content-visibility CSS moved from old RichCard to GenerativeUISlot wrapper"

patterns-established:
  - "CARD_REGISTRY pattern: new card types added by Map.set(), no conditionals"
  - "CardErrorBoundary with resetKey prop for per-card error isolation"

requirements-completed: [GENUI-01, GENUI-02]

duration: 4min
completed: 2026-04-09
---

# Phase 52 Plan 01: Card Registry & Generative UI Slot Summary

**Static CARD_REGISTRY mapping card type strings to React components, with ErrorBoundary isolation and JSON fallback for unknown types**

## Performance

- **Duration:** 4 min
- **Started:** 2026-04-09T19:41:39Z
- **Completed:** 2026-04-09T19:45:40Z
- **Tasks:** 2
- **Files modified:** 10

## Accomplishments
- Created card-registry.tsx with CARD_REGISTRY Map, GenerativeUISlot, and CardErrorBoundary
- Exported MetricCard from rich-card.tsx (was private) for registry use
- Widened RichCardPart.cardType from "table"|"metric"|"agent-turn" union to string
- Wired GenerativeUISlot into ChatThread replacing direct RichCard usage
- All 418 tests pass including 7 new card-registry tests

## Task Commits

Each task was committed atomically:

1. **Task 1: CARD_REGISTRY, GenerativeUISlot, ErrorBoundary + tests** - `3941b81` (feat, TDD)
2. **Task 2: Wire GenerativeUISlot into chat rendering + widen cardType** - `d0382a9` (feat)

## Files Created/Modified
- `ui/src/components/ui/card-registry.tsx` - CARD_REGISTRY Map, GenerativeUISlot, CardErrorBoundary
- `ui/src/__tests__/card-registry.test.tsx` - 7 tests for registry lookup, JSON fallback, error boundary
- `ui/src/components/ui/rich-card.tsx` - Exported MetricCard (was private)
- `ui/src/stores/chat-store.ts` - Widened RichCardPart.cardType to string
- `ui/src/stores/sse-events.ts` - Widened rich-card SSE event cardType to string, safer parsing
- `ui/src/app/(authenticated)/chat/ChatThread.tsx` - Replaced RichCard with GenerativeUISlot
- `ui/src/__tests__/{chat-input,message-list,multi-agent-identity,session-management}.test.tsx` - Updated mocks with TableCard/MetricCard exports

## Decisions Made
- Registry is a mutable Map (not frozen object) to allow runtime registration in tests and future plugin scenarios
- content-visibility CSS property moved from old RichCard wrapper to GenerativeUISlot wrapper

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Updated 4 test file mocks to export TableCard and MetricCard**
- **Found during:** Task 2
- **Issue:** Existing test mocks for rich-card.tsx only exported RichCard. After card-registry.tsx imports TableCard/MetricCard from rich-card.tsx, the mock needed those exports too.
- **Fix:** Added TableCard and MetricCard stub exports to all 4 test file mocks
- **Files modified:** chat-input.test.tsx, message-list.test.tsx, multi-agent-identity.test.tsx, session-management.test.tsx
- **Verification:** All 418 tests pass
- **Committed in:** d0382a9 (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Essential for test compatibility. No scope creep.

## Issues Encountered
None

## Known Stubs
None - all card types are fully wired through the registry.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- CARD_REGISTRY is extensible: Plan 02/03 can register new card types by importing the Map
- GenerativeUISlot handles any string cardType gracefully
- RichCard export in rich-card.tsx is still present but no longer imported anywhere -- can be removed in cleanup

---
*Phase: 52-citations-generative-ui*
*Completed: 2026-04-09*
