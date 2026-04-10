---
phase: 58-channels-bridge-errors
plan: 01
subsystem: channels
tags: [typescript, bun, error-handling, telegram, bridge]

requires: []
provides:
  - "Bridge HTTP methods throw descriptive errors on failure"
  - "Telegram @botname mention stripping in group mode"
affects: [channels, telegram-driver]

tech-stack:
  added: []
  patterns:
    - "Bridge methods throw instead of returning sentinel values (null, [], false)"

key-files:
  created: []
  modified:
    - channels/src/bridge.ts
    - channels/src/drivers/common.ts
    - channels/src/drivers/telegram.ts
    - channels/src/__tests__/bridge.test.ts

key-decisions:
  - "uploadMedia return type changed from Promise<string | null> to Promise<string>"
  - "reUploadAttachments no longer falls back to original URL on error; errors propagate to callers which already have try/catch"

patterns-established:
  - "Bridge HTTP methods: throw on failure, never return sentinel values"

requirements-completed: [STAB-01, SEC-02]

duration: 3min
completed: 2026-04-10
---

# Phase 58 Plan 01: Channels Bridge Error Propagation Summary

**Bridge HTTP methods throw descriptive errors instead of silent sentinels; @botname stripped from Telegram group messages before LLM dispatch**

## Performance

- **Duration:** 3 min
- **Started:** 2026-04-10T05:13:06Z
- **Completed:** 2026-04-10T05:16:18Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments
- listUsers, revokeUser, uploadMedia now throw descriptive errors on HTTP failures instead of returning [], false, null
- reUploadAttachments propagates upload errors to callers instead of silently falling back
- @botname text stripped from group messages before sending to LLM, preventing wasted tokens and model confusion

## Task Commits

Each task was committed atomically:

1. **Task 1: Fix bridge.ts error swallowing (RED)** - `426744f` (test)
2. **Task 1: Fix bridge.ts error swallowing (GREEN)** - `99c3106` (feat)
3. **Task 2: Strip @botname mention from text** - `d32823e` (fix)

## Files Created/Modified
- `channels/src/bridge.ts` - Removed try/catch error swallowing from listUsers, revokeUser, uploadMedia
- `channels/src/drivers/common.ts` - Updated reUploadAttachments to not fall back on null, updated type signature
- `channels/src/drivers/telegram.ts` - Added @botname stripping regex after mention detection in group mode
- `channels/src/__tests__/bridge.test.ts` - Added 8 tests for error propagation (HTTP errors + network failures + reUpload)

## Decisions Made
- Changed uploadMedia return type from `Promise<string | null>` to `Promise<string>` since it now throws on failure
- reUploadAttachments errors propagate to callers (processMessage in telegram.ts already has try/catch with error reaction)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

- Pre-existing test failure in `splitText > handles multibyte characters` (common.test.ts) - unrelated to our changes, confirmed by running tests before and after. Not in scope.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Bridge error propagation complete, callers already have error handling
- All channel tests pass (except pre-existing splitText multibyte test)

---
*Phase: 58-channels-bridge-errors*
*Completed: 2026-04-10*

## Self-Check: PASSED
