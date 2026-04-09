---
gsd_state_version: 1.0
milestone: v0.12.0
milestone_name: Chat Redesign
status: verifying
stopped_at: Completed 41-02-PLAN.md tasks 1-2, checkpoint at task 3
last_updated: "2026-04-09T11:48:21.402Z"
last_activity: 2026-04-09
progress:
  total_phases: 6
  completed_phases: 2
  total_plans: 3
  completed_plans: 3
  percent: 0
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-09)

**Core value:** Стабильная и безопасная AI-платформа с self-hosted фокусом
**Current focus:** Phase 41 — ConnectionPhase FSM

## Current Position

Phase: 41 (ConnectionPhase FSM) — EXECUTING
Plan: 2 of 2
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

### Pending Todos

None.

### Blockers/Concerns

- No backend changes required for this milestone — all changes are frontend only
- `sync` event reconciliation during reconnect: verify `assistantId` reset behavior (open question from ARCHITECTURE.md)
- Optimistic user message UUID reconciliation strategy must be decided during Phase 43 planning

## Session Continuity

Last session: 2026-04-09T11:48:21.398Z
Stopped at: Completed 41-02-PLAN.md tasks 1-2, checkpoint at task 3
Resume with: `/gsd:plan-phase 40`
