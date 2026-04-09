---
phase: 48-optimistic-responsive-ui
plan: "02"
subsystem: ui-chat
tags: [reconciliation, content-hash, live-to-history, optimistic-ui]
dependency_graph:
  requires: []
  provides: [contentHash, reconcileLiveWithHistory, delayed-history-transition]
  affects: [chat-store, ChatThread-sourceMessages]
tech_stack:
  added: []
  patterns: [djb2-hash, delayed-transition, content-fingerprint]
key_files:
  created:
    - ui/src/__tests__/opti-reconciliation.test.ts
  modified:
    - ui/src/stores/chat-store.ts
decisions:
  - "djb2-style hash for message fingerprinting — fast, no crypto needed for render optimization"
  - "600ms delay for live-to-history transition — enough for React Query invalidation + fetch"
  - "No transition if cache not populated — live messages stay visible, no blank flash"
  - "User message UUID-to-DB-ID swap accepted as clean DOM swap (identical content, no flicker)"
metrics:
  duration: 2m29s
  completed: "2026-04-09"
---

# Phase 48 Plan 02: Content Hash Reconciliation Summary

Content hash djb2 fingerprint + reconcileLiveWithHistory for flicker-free live-to-history transition with 600ms delayed mode switch

## What Was Done

### Task 1: Content hash utility + reconciliation (TDD)

**RED:** 8 failing tests covering contentHash stability/uniqueness/timestamp-invariance and reconcileLiveWithHistory identical-skip/extra-msgs/content-diff/ID-mismatch scenarios.

**GREEN:** Implemented `contentHash()` (djb2-style hash ignoring timestamps) and `reconcileLiveWithHistory()` (returns null when identical, history array when different). Updated stream finalization to use delayed 600ms transition with cache population guard.

**Key changes:**
- `contentHash(messages)` — fast fingerprint of id+role+parts content, ignores createdAt
- `reconcileLiveWithHistory(live, history)` — null = skip re-render, array = use history
- Stream finalization: setTimeout(600ms) checks if React Query cache has populated before transitioning messageSource from "live" to "history"
- Guard: only transitions if still in live mode for same session (handles user navigation away)
- Guard: does NOT transition if cachedData is not yet available (prevents blank flash)

## Deviations from Plan

None - plan executed exactly as written.

## Commits

| Hash | Type | Description |
|------|------|-------------|
| b398862 | test | Add failing tests for content hash reconciliation (8 tests) |
| c430e86 | feat | Content hash reconciliation for live-to-history transition |

## Known Stubs

None - all functions are fully implemented and wired.

## Verification

- All 8 tests pass (vitest exit code 0)
- `npm run build` succeeds with no type errors
- `contentHash` and `reconcileLiveWithHistory` exported and used in finalization block

## Self-Check: PASSED

- FOUND: ui/src/__tests__/opti-reconciliation.test.ts
- FOUND: ui/src/stores/chat-store.ts
- FOUND: b398862
- FOUND: c430e86
