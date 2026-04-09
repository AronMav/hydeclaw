---
phase: 47-scroll-virtualization
plan: "03"
subsystem: ui
tags: [react, content-visibility, lazy-loading, performance, css]

# Dependency graph
requires: []
provides:
  - content-visibility: auto on RichCard root wrapper for browser-native offscreen deferral
  - loading=lazy on img elements in FileDataPartView (was already present)
affects: [47-scroll-virtualization]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "content-visibility: auto with containIntrinsicSize on card wrappers for DOM deferral"
    - "Native browser lazy loading via loading=lazy on img elements (zero JS)"

key-files:
  created: []
  modified:
    - ui/src/components/ui/rich-card.tsx

key-decisions:
  - "Single content-visibility wrapper on RichCard dispatcher rather than individual TableCard/MetricCard — avoids duplication and handles fallback pre too"
  - "containIntrinsicSize: 0 200px reserves 200px height offscreen — prevents layout shift for on-screen content"
  - "loading=lazy was already present on FileDataPartView img elements — no change needed for Task 2"

patterns-established:
  - "Pattern: Wrap card root with contentVisibility: auto + containIntrinsicSize for long-list deferral"

requirements-completed: [VIRT-02]

# Metrics
duration: 5min
completed: 2026-04-09
---

# Phase 47 Plan 03: Content-Visibility and Image Lazy Loading Summary

**Browser-native DOM deferral via `content-visibility: auto` on RichCard wrappers and `loading="lazy"` on images, reducing layout/paint cost for offscreen cards in long conversations**

## Performance

- **Duration:** 5 min
- **Started:** 2026-04-09T16:50:00Z
- **Completed:** 2026-04-09T16:55:00Z
- **Tasks:** 2
- **Files modified:** 1 (rich-card.tsx; FileDataPartView.tsx already had loading=lazy)

## Accomplishments
- RichCard root div now has `contentVisibility: "auto"` with `containIntrinsicSize: "0 200px"` — browser skips layout/paint for offscreen cards
- `loading="lazy"` confirmed present on FileDataPartView img elements (was pre-existing, no change needed)
- All 428 UI tests pass, TypeScript compiles clean

## Task Commits

1. **Task 1: content-visibility on RichCard wrappers** - `e5b60a7` (feat)
2. **Task 2: loading=lazy on image elements** - pre-existing (no commit needed)

**Plan metadata:** (docs commit below)

## Files Created/Modified
- `ui/src/components/ui/rich-card.tsx` - Wrapped RichCard dispatch in `<div style={{ contentVisibility: "auto", containIntrinsicSize: "0 200px" }}>` for browser-native offscreen deferral

## Decisions Made
- Single wrapper on RichCard function (not on each card variant) — cleaner, covers all card types including fallback pre element
- containIntrinsicSize of 200px is a conservative estimate fitting both table cards and metric cards
- Task 2 required no change: `loading="lazy"` was already on the `<img>` in FileDataPartView.tsx (line 12)

## Deviations from Plan

None - plan executed exactly as written. Task 2 was already implemented (pre-existing `loading="lazy"`), documented as-is.

## Issues Encountered
None.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- RichCard DOM deferral is active — long conversations with many rich cards will skip offscreen layout/paint
- Ready for remaining Phase 47 virtualization plans

---
*Phase: 47-scroll-virtualization*
*Completed: 2026-04-09*
