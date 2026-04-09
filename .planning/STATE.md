---
gsd_state_version: 1.0
milestone: v0.12.0
milestone_name: Chat Redesign
status: verifying
stopped_at: Completed 45-01-PLAN.md
last_updated: "2026-04-09T13:08:13.883Z"
last_activity: 2026-04-09
progress:
  total_phases: 6
  completed_phases: 3
  total_plans: 10
  completed_plans: 7
  percent: 0
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-09)

**Core value:** Стабильная и безопасная AI-платформа с self-hosted фокусом
**Current focus:** Phase 45 — Cleanup

## Current Position

Phase: 45
Plan: Not started
Status: Phase complete — ready for verification
Last activity: 2026-04-09

Progress: ░░░░░░░░░░ 0% (0/6 phases)

## Performance Metrics

| Metric               | Value |
| -------------------- | ----- |
| Phases total         | 6     |
| Phases complete      | 0     |
| Requirements mapped  | 15/15 |
| Coverage             | 100%  |
| Phase 40-sseconnection-extraction P01 | 8 | 2 tasks | 3 files |
| Phase 41-connectionphase-fsm P01 | 25 | 2 tasks | 3 files |
| Phase 41-connectionphase-fsm P02 | 8 | 2 tasks | 4 files |
| Phase 42-history-messagesource P01 | 25 | 2 tasks | 11 files |
| Phase 43-reconnect-optimistic-ui P01 | 4 | 2 tasks | 3 files |
| Phase 44-ux-polish P01 | 3 | 2 tasks | 3 files |
| Phase 45-cleanup P01 | 25 | 2 tasks | 9 files |

## Accumulated Context

### Decisions

- [v0.11.2]: Virtual Merging, IncrementalParser, Robust Thinking Indicator added but introduced regressions
- [Bug]: ThinkingMessage appears on new empty session (sessionStorage persistence never clears)
- [Bug]: ThinkingMessage stays after stream completion (no cleanup path)
- [Bug]: After F5 history doesn't restore, only thinking avatar shows
- [Architecture]: No XState — custom `chatStateMachine` pure function inside Zustand (zero deps, ~80 lines)
- [Architecture]: Keep `fetch` + `ReadableStream`, add exponential-backoff reconnect inline (no @microsoft/fetch-event-source)
- [Architecture]: React 19 `useOptimistic` for user message bubble; Zustand+Immer stays for live stream mutations
- [Architecture]: Keep `react-virtuoso` free `Virtuoso` — no upgrade to paid `VirtuosoMessageList`
- [Architecture]: `SseConnection` class (not hook/inline) — survives React re-renders, independently testable
- [Architecture]: `MessageSource` discriminated union replaces `viewMode + liveMessages` duality
- [Architecture]: `streamGeneration` must become per-agent — module-scope counter silently kills concurrent streams
- [Phase 40-sseconnection-extraction]: SseConnection.onDone called only on natural completion; finalizeStream is always natural-end path
- [Phase 40-sseconnection-extraction]: streamGeneration kept as module-global (per-agent is Phase 45 CLN-02 scope)
- [Phase 41]: ConnectionPhase runs in parallel with StreamStatus (Phase 45 CLN-01 removes it)
- [Phase 41]: IncrementalParser.reset() called on finish events to prevent reasoning state leaking between agent turns
- [Phase 41]: sessionStorage streaming flag cleared atomically in finish handler (not just sync)
- [Phase 41]: showThinking = connectionPhase === submitted || engineRunning (removed 4-signal expression and sessionStorage fallback)
- [Phase 42]: MessageSource discriminated union (new-chat|live|history) replaces viewMode+liveMessages dual-semantics in AgentState
- [Phase 42]: Per-agent streamGenerations[agent] Record — module-scope counter replacement prevents concurrent stream killing when switching agents
- [Phase 43]: onPhaseChange added as SseConnectionCallbacksWithPhase (extends base callbacks) — backward-compat, callers without phase tracking unaffected
- [Phase 43]: receivedFinishEvent flag distinguishes natural stream end from connection drop in processSSEStream
- [Phase 43]: reconnectTimers cleared in both abortActiveStream() and stopStream() — user abort must not trigger reconnect
- [Phase 44-ux-polish]: saveDraft(agent, '') removes the key — no stale localStorage entries
- [Phase 44-ux-polish]: totalPartsCount Stage 3 Fix useEffect removed — followOutput callback is single scroll authority
- [Phase 45-cleanup]: CLN-01: StreamStatus/isActiveStream removed — ConnectionPhase/isActivePhase are sole stream-state authorities
- [Phase 45-cleanup]: CLN-02: AbortController/timers in private Maps not Immer state; streamGeneration moved to AgentState as plain number

### Pending Todos

None.

### Blockers/Concerns

- No backend changes required for this milestone — all changes are frontend only
- `sync` event reconciliation during reconnect: verify `assistantId` reset behavior (open question from ARCHITECTURE.md)
- Optimistic user message UUID reconciliation strategy must be decided during Phase 43 planning

## Session Continuity

Last session: 2026-04-09T13:07:39.887Z
Stopped at: Completed 45-01-PLAN.md
Resume with: `/gsd:plan-phase 40`
