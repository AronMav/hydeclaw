---
phase: 55-gateway-sub-routers
plan: 01
subsystem: api
tags: [axum, router, refactoring, gateway]

requires: []
provides:
  - "Domain-scoped sub-routers in every handler module (pub(crate) fn routes())"
  - "Slim gateway/mod.rs with merge-only composition"
affects: [any future gateway route additions]

tech-stack:
  added: []
  patterns:
    - "Handler sub-router pattern: each handler module has pub(crate) fn routes() -> Router<AppState>"
    - "Special layers (body limits, guard middleware) applied within routes() function"

key-files:
  created: []
  modified:
    - "crates/hydeclaw-core/src/gateway/mod.rs"
    - "crates/hydeclaw-core/src/gateway/handlers/mod.rs"
    - "crates/hydeclaw-core/src/gateway/handlers/*.rs (27 handler modules)"

key-decisions:
  - "monitoring::routes() takes &AppState param for setup_guard_middleware (from_fn_with_state requires concrete state)"
  - "mcp_callback and health handlers stay in chat.rs routes() (where they are defined), not moved to separate modules"
  - "config.rs routes() includes TTS, canvas, and restart routes (co-located handlers stay together)"
  - "Channels routes() references agents::api_agent_hooks via super::agents:: qualified path"

patterns-established:
  - "Sub-router pattern: new routes go in handler module's routes() function, not mod.rs"
  - "Cross-module handler references use super::module::handler_name qualified paths"

requirements-completed: [ARCH-02]

duration: 17min
completed: 2026-04-10
---

# Phase 55 Plan 01: Gateway Sub-Routers Summary

**Decomposed 220+ route registrations in gateway/mod.rs into 27 domain-scoped sub-routers with merge-only composition**

## Performance

- **Duration:** 17 min
- **Started:** 2026-04-10T04:11:51Z
- **Completed:** 2026-04-10T04:28:24Z
- **Tasks:** 2
- **Files modified:** 29

## Accomplishments
- Every handler module now has a `pub(crate) fn routes() -> Router<AppState>` function
- gateway/mod.rs router() reduced from 220+ .route() calls to 27 .merge() calls
- All glob re-exports removed from handlers/mod.rs
- All 58 gateway tests pass, zero regressions

## Task Commits

Each task was committed atomically:

1. **Task 1: Add routes() sub-router functions to every handler module** - `ea14ab7` (feat)
2. **Task 2: Slim gateway/mod.rs to merge-only composition + clean up handlers/mod.rs** - `7ff46a7` (refactor)

## Files Created/Modified
- `crates/hydeclaw-core/src/gateway/mod.rs` - Slim router with 27 merge calls, no individual route registrations
- `crates/hydeclaw-core/src/gateway/handlers/mod.rs` - Module declarations only, no glob re-exports
- `crates/hydeclaw-core/src/gateway/handlers/*.rs` (27 files) - Each has routes() function with domain routes

## Decisions Made
- monitoring::routes() takes &AppState because setup_guard_middleware needs from_fn_with_state (all other routes() are zero-arg)
- Routes stay grouped by handler module where the function is defined, not by URL prefix domain
- api_agent_hooks route placed in channels::routes() with qualified super::agents:: reference

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] monitoring::routes() requires state parameter**
- **Found during:** Task 1
- **Issue:** setup_guard_middleware uses from_fn_with_state which needs a concrete AppState value, not available in a zero-arg routes() function
- **Fix:** Changed monitoring::routes() signature to take &AppState parameter
- **Files modified:** monitoring.rs
- **Verification:** cargo check passes
- **Committed in:** ea14ab7

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Minimal - one module has a slightly different routes() signature. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Gateway sub-router pattern established for all future route additions
- Adding new routes only requires editing the relevant handler module

---
*Phase: 55-gateway-sub-routers*
*Completed: 2026-04-10*
