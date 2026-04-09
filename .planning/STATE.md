---
gsd_state_version: 1.0
milestone: v0.13.0
milestone_name: Chat UX Evolution
status: executing
stopped_at: Completed 47-01-PLAN.md
last_updated: "2026-04-09T15:56:37.001Z"
last_activity: 2026-04-09
progress:
  total_phases: 14
  completed_phases: 4
  total_plans: 17
  completed_plans: 12
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-09)

**Core value:** Стабильная и безопасная AI-платформа с self-hosted фокусом
**Current focus:** Phase 47 — Scroll & Virtualization

## Current Position

Phase: 47 (Scroll & Virtualization) — EXECUTING
Plan: 2 of 4
Status: Ready to execute
Last activity: 2026-04-09

Progress bar: `░░░░░░░░░░░░░░░░░░░░` 0% (0/8 phases)

## Performance Metrics

| Metric | Value |
|--------|-------|
| Phases defined | 8 |
| Requirements mapped | 26/26 |
| Plans complete | 0 |
| Phase 46-streaming-performance P01 | 5 | 1 tasks | 1 files |
| Phase 46-streaming-performance P02 | 210 | 2 tasks | 5 files |
| Phase 47-scroll-virtualization P01 | 5 | 1 tasks | 1 files |

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
- [Roadmap v0.13.0]: Copy-to-clipboard and ResizeObserver scroll anchoring from v0.11.2 are ALREADY DONE — must verify before implementing PERF/SCRL work
- [Roadmap v0.13.0]: BRNC is isolated last — only phase requiring DB migration, highest risk
- [Roadmap v0.13.0]: HITL depends on SSE heartbeat (Phase 50) to survive nginx 60s timeout
- [Roadmap v0.13.0]: Phase 50 SSE events (ContinuationStart, HandoffMetadata, start-step/finish-step) are additive — no breaking changes to backend
- [Phase 46-01]: STREAM_THROTTLE_MS is exported from chat-store.ts — PERF-01 test imports it directly for regression guard
- [Phase 46-01]: scheduleUpdate/pushUpdate are closure-private, so PERF-01 tests replicate closure logic inline as pure unit tests
- [Phase 46-01]: PERF-02/03 use placeholder RED tests to document exact contracts Plan 02 must implement (blockKey, isStreamingCode, isUnclosedCodeBlock)
- [Phase 46-02]: isStreamingCode determined by fence detection alone (isUnclosedCodeBlock), not isStreaming flag — fence state is authoritative
- [Phase 46-02]: Two stable component objects (INITIAL_COMPONENTS, STREAMING_COMPONENTS) replace dynamic creation per block — threads isStreamingCode via closure without object churn
- [Phase 47-scroll-virtualization]: overflow-anchor applied inside ResizeObserver useEffect after querySelector (idempotent), atBottomThreshold 150→100 (SCRL-02), increaseViewportBy {top:500,bottom:200} for asymmetric media preload (VIRT-01)

### Pending Todos

- Verify which PERF/SCRL items from v0.11.2 are already done before starting Phase 46
- NET-01 Last-Event-ID needs backend support for resuming from position (verify current backend capability)

### Blockers/Concerns

- HITL (Phase 51): nginx 60-second timeout kills SSE during approval wait — Phase 50 heartbeat must land first
- BRNC (Phase 53): DB migration is the only schema change in this milestone — must be last, isolated, with rollback plan

## Session Continuity

Last session: 2026-04-09T15:56:36.997Z
Stopped at: Completed 47-01-PLAN.md
Resume with: `/gsd:plan-phase 46`
