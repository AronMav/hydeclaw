---
phase: 51-human-in-the-loop
plan: 01
subsystem: api
tags: [sse, approval, streaming, human-in-the-loop, tool-approval]

requires:
  - phase: none
    provides: existing approval flow in engine_dispatch.rs

provides:
  - ApprovalNeeded and ApprovalResolved StreamEvent variants
  - SSE tool-approval-needed and tool-approval-resolved event types
  - ApprovedWithModifiedArgs approval result variant
  - modified_input support in resolve_approval API

affects: [51-02, 51-03, ui-approval-inline]

tech-stack:
  added: []
  patterns: [inline SSE approval events during streaming, modified tool args on approval]

key-files:
  created: []
  modified:
    - crates/hydeclaw-core/src/agent/engine.rs
    - crates/hydeclaw-core/src/agent/engine_dispatch.rs
    - crates/hydeclaw-core/src/gateway/mod.rs
    - crates/hydeclaw-core/src/gateway/handlers/chat.rs
    - crates/hydeclaw-core/src/gateway/handlers/agents.rs
    - crates/hydeclaw-core/src/gateway/handlers/channel_ws.rs

key-decisions:
  - "ApprovalNeeded emitted after waiter insert and before timeout wait -- UI receives event immediately"
  - "ApprovalResolved emitted from resolve_approval (for approve/reject) and from dispatch timeout handler (for timeout_rejected)"
  - "modified_input re-injects _context from original arguments before recursing into execute_tool_call"
  - "clean_input (without _context) sent in ApprovalNeeded SSE event to avoid leaking internal routing data"

patterns-established:
  - "SSE approval events: tool-approval-needed and tool-approval-resolved alongside existing stream events"
  - "Modified args flow: API -> resolve_approval -> ApprovedWithModifiedArgs -> re-enriched execute_tool_call"

requirements-completed: [HITL-01, HITL-02]

duration: 8min
completed: 2026-04-09
---

# Phase 51 Plan 01: SSE Approval Events Summary

**ApprovalNeeded/ApprovalResolved SSE streaming events with modified_input support in resolve_approval API**

## Performance

- **Duration:** 8 min
- **Started:** 2026-04-09T19:07:28Z
- **Completed:** 2026-04-09T19:15:45Z
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments
- Two new StreamEvent variants (ApprovalNeeded, ApprovalResolved) for inline approval UX in SSE chat stream
- ApprovedWithModifiedArgs variant allows the frontend to submit edited tool arguments on approval
- SSE marshalling emits tool-approval-needed and tool-approval-resolved with full approval metadata
- engine_dispatch.rs emits ApprovalNeeded when approval wait starts, ApprovalResolved on timeout
- resolve_approval API endpoint accepts optional modified_input field and passes it through the oneshot channel

## Task Commits

Each task was committed atomically:

1. **Task 1: Add StreamEvent variants, ApprovalResult variant, and emit SSE events** - `691808c` (feat)
2. **Task 2: Update resolve_approval API to accept modified_input** - `de7709e` (feat)
3. **Clippy fix: collapse nested if-let** - `304b3a0` (fix)

## Files Created/Modified
- `crates/hydeclaw-core/src/agent/engine.rs` - Added ApprovalNeeded/ApprovalResolved StreamEvent variants, ApprovedWithModifiedArgs, updated resolve_approval signature
- `crates/hydeclaw-core/src/agent/engine_dispatch.rs` - Emits ApprovalNeeded SSE event, handles ApprovedWithModifiedArgs, emits ApprovalResolved on timeout
- `crates/hydeclaw-core/src/gateway/mod.rs` - Added TOOL_APPROVAL_NEEDED and TOOL_APPROVAL_RESOLVED SSE type constants
- `crates/hydeclaw-core/src/gateway/handlers/chat.rs` - SSE marshalling for approval events
- `crates/hydeclaw-core/src/gateway/handlers/agents.rs` - Extract modified_input from request body, pass to resolve_approval
- `crates/hydeclaw-core/src/gateway/handlers/channel_ws.rs` - Updated resolve_approval call with None for modified_input

## Decisions Made
- ApprovalNeeded sends clean_input (without _context) to avoid leaking internal routing data to the frontend
- ApprovalResolved is emitted both from resolve_approval (approve/reject) and the dispatch timeout handler (timeout_rejected)
- Modified args are re-enriched with _context from original arguments before recursing into execute_tool_call
- channel_ws.rs caller passes None for modified_input since Telegram approval buttons don't support arg modification

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Updated channel_ws.rs resolve_approval caller**
- **Found during:** Task 1 (signature change)
- **Issue:** channel_ws.rs:360 calls resolve_approval with 3 args, but the updated signature requires 4
- **Fix:** Added None as modified_input parameter
- **Files modified:** crates/hydeclaw-core/src/gateway/handlers/channel_ws.rs
- **Verification:** cargo check passes
- **Committed in:** 691808c (Task 1 commit)

**2. [Rule 1 - Bug] Collapsed nested if-let per clippy**
- **Found during:** Post-Task 2 verification
- **Issue:** Clippy flagged collapsible if-let in engine_dispatch.rs modified args block
- **Fix:** Merged nested if-let into single chained condition
- **Files modified:** crates/hydeclaw-core/src/agent/engine_dispatch.rs
- **Verification:** cargo check passes
- **Committed in:** 304b3a0

---

**Total deviations:** 2 auto-fixed (1 blocking, 1 bug)
**Impact on plan:** Both auto-fixes necessary for compilation and lint compliance. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- SSE approval events are ready for frontend consumption in 51-02 (UI approval components)
- The tool-approval-needed event provides approvalId, toolName, toolInput, timeoutMs
- The tool-approval-resolved event provides approvalId, action, optional modifiedInput
- Frontend can POST to /api/approvals/{id}/resolve with optional modified_input field

---
*Phase: 51-human-in-the-loop*
*Completed: 2026-04-09*
