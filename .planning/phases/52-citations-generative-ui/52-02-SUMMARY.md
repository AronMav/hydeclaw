---
phase: 52-citations-generative-ui
plan: 02
subsystem: ui
tags: [react, markdown, footnotes, tooltip, radix, remark-gfm]

# Dependency graph
requires: []
provides:
  - CitationRef component for hoverable footnote superscripts with tooltip
  - CitationSection component for visually hidden footnote definitions
  - extractFootnotes utility for parsing footnote definitions from markdown
  - FootnoteContext/FootnoteProvider for passing footnote data to nested components
  - createFootnoteComponents for react-markdown sup/section overrides
affects: [52-citations-generative-ui]

# Tech tracking
tech-stack:
  added: []
  patterns: [React context for cross-component footnote data, component override pattern for react-markdown]

key-files:
  created: [ui/src/components/ui/citation-tooltip.tsx, ui/src/__tests__/citation-tooltip.test.tsx]
  modified: [ui/src/components/ui/markdown.tsx]

key-decisions:
  - "Used React context (FootnoteContext) to pass footnote definitions from Markdown wrapper to CitationRef components"
  - "Used regex extraction of footnote defs from raw markdown rather than parsing rendered AST"
  - "Footnote section hidden via sr-only (accessible to screen readers) rather than display:none"

patterns-established:
  - "Footnote override pattern: createFootnoteComponents() returns sup/section overrides merged into INITIAL_COMPONENTS"
  - "FootnoteProvider wrapping: only applied when extractFootnotes returns non-empty map (zero overhead for non-footnote markdown)"

requirements-completed: [CITE-01]

# Metrics
duration: 5min
completed: 2026-04-09
---

# Phase 52 Plan 02: Citation Tooltip Summary

**Footnote references render as hoverable superscript badges with source-text tooltips via React context and react-markdown component overrides**

## Performance

- **Duration:** 5 min
- **Started:** 2026-04-09T19:41:29Z
- **Completed:** 2026-04-09T19:46:26Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- CitationRef renders [^N] footnote references as styled superscript with Radix tooltip showing definition text
- CitationSection hides footnote definitions section with sr-only (accessible to screen readers)
- FootnoteProvider + extractFootnotes provide footnote data to deeply nested CitationRef components
- Zero overhead for markdown without footnotes (no wrapper, no context provider)
- All 418 tests pass including 7 new citation-tooltip tests, production build succeeds

## Task Commits

Each task was committed atomically:

1. **Task 1: CitationRef tooltip component + tests** - `5226c39` (feat) [TDD]
2. **Task 2: Wire footnote components into Markdown renderer** - `222ba17` (feat)

## Files Created/Modified
- `ui/src/components/ui/citation-tooltip.tsx` - CitationRef, CitationSection, extractFootnotes, FootnoteContext, createFootnoteComponents
- `ui/src/__tests__/citation-tooltip.test.tsx` - 7 tests for extraction, rendering, tooltip structure, graceful degradation
- `ui/src/components/ui/markdown.tsx` - Import footnote components, merge overrides into INITIAL_COMPONENTS, wrap with FootnoteProvider

## Decisions Made
- Used React context (FootnoteContext) rather than prop drilling to pass footnote definitions from Markdown wrapper to CitationRef - allows footnote data to reach deeply nested react-markdown components
- Used regex extraction of `[^N]: text` patterns from raw markdown rather than parsing the rendered AST - simpler and works reliably with remark-gfm's output
- Footnote section hidden via sr-only (not display:none) for accessibility compliance
- No user-event dependency added for tooltip hover test - used data-slot attribute verification instead since Radix tooltips require pointer events not supported in jsdom

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
- ResizeObserver not defined in jsdom environment when testing tooltip hover - solved by mocking ResizeObserver and testing tooltip structure via data-slot attributes instead of simulating full hover interaction

## User Setup Required

None - no external service configuration required.

## Known Stubs

None - all components are fully wired with data flowing from markdown text through extractFootnotes to FootnoteContext to CitationRef tooltips.

## Next Phase Readiness
- Citation tooltip system ready for use by any markdown content
- Component overrides pattern established for future react-markdown customizations

## Self-Check: PASSED

- [x] ui/src/components/ui/citation-tooltip.tsx - FOUND
- [x] ui/src/__tests__/citation-tooltip.test.tsx - FOUND
- [x] ui/src/components/ui/markdown.tsx - FOUND
- [x] Commit 5226c39 - FOUND
- [x] Commit 222ba17 - FOUND

---
*Phase: 52-citations-generative-ui*
*Completed: 2026-04-09*
