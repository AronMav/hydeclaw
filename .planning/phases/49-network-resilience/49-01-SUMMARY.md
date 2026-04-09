---
phase: 49-network-resilience
plan: 01
subsystem: api
tags: [sse, event-id, stream-resume, keepalive, axum]

requires:
  - phase: none
    provides: existing StreamRegistry and chat SSE handler
provides:
  - Monotonic u64 event IDs on all SSE events
  - replay_from method for partial stream resume via Last-Event-ID
  - 410 GONE with X-Stream-Expired header for expired streams
  - 30s heartbeat KeepAlive on all SSE endpoints
affects: [49-02, 49-03, frontend-sse-reconnect]

tech-stack:
  added: []
  patterns: [event-id-tagged-sse, last-event-id-resume, 410-stream-expiry]

key-files:
  created: []
  modified:
    - crates/hydeclaw-core/src/gateway/stream_registry.rs
    - crates/hydeclaw-core/src/gateway/handlers/chat.rs

key-decisions:
  - "Event IDs are per-stream monotonic u64 starting at 0, assigned in push_event under per-stream mutex"
  - "replay_from filters events with id > last_event_id (exclusive per SSE spec)"
  - "410 GONE returned only when Last-Event-ID present but stream not found and no DB job"
  - "Live phase dedup uses event ID comparison instead of skip count for correctness"

patterns-established:
  - "SSE event ID pattern: push_event returns u64, caller sets Event::id()"
  - "Stream resume pattern: Last-Event-ID header -> replay_from -> partial replay + live"

requirements-completed: [NET-01]

duration: 12min
completed: 2026-04-09
---

# Phase 49 Plan 01: SSE Event IDs and Stream Resume Summary

**Monotonic u64 event IDs on SSE streams with Last-Event-ID partial replay and 410 expiry signaling**

## Performance

- **Duration:** 12 min
- **Started:** 2026-04-09T17:17:50Z
- **Completed:** 2026-04-09T17:29:50Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- StreamRegistry stores (u64, String) tuples with monotonic counter, push_event returns assigned ID
- New replay_from method enables partial replay from any event ID position
- SSE events carry id: field via send_and_buffer macro for browser-native Last-Event-ID support
- Resume handler parses Last-Event-ID header and uses replay_from for efficient reconnection
- Expired streams return 410 GONE with X-Stream-Expired header for frontend disconnect detection
- All 4 KeepAlive instances upgraded to 30s interval with "heartbeat" text

## Task Commits

Each task was committed atomically:

1. **Task 1: Extend StreamRegistry with event IDs and replay_from** - `da5f9d3` (feat)
2. **Task 2: Add Event::id() to SSE output, Last-Event-ID parsing, 410, heartbeat** - `e709b46` (feat)

## Files Created/Modified
- `crates/hydeclaw-core/src/gateway/stream_registry.rs` - Added next_event_id counter, (u64,String) tuple storage, replay_from method
- `crates/hydeclaw-core/src/gateway/handlers/chat.rs` - Event::id() on SSE events, Last-Event-ID parsing, 410 response, 30s heartbeat

## Decisions Made
- Event IDs are per-stream monotonic u64 starting at 0 -- simple, no global coordination needed
- replay_from uses > comparison (exclusive) per SSE spec: Last-Event-ID is the last successfully processed event
- Live phase dedup switched from skip_remaining count to event ID comparison -- more robust against lag scenarios
- 410 GONE only returned when client explicitly sends Last-Event-ID but stream is gone -- preserves backward compat for clients without resume support

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Backend SSE infrastructure ready for frontend SseConnection to implement auto-reconnect with Last-Event-ID
- Plan 49-02 can implement frontend reconnection logic using these event IDs
- Plan 49-03 can add offline queue and sync using the 410 expiry signal

---
*Phase: 49-network-resilience*
*Completed: 2026-04-09*
