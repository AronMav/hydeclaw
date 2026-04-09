# Roadmap: HydeClaw

## Milestones

- ✅ **v0.2.0–v0.11.0** — Core platform, Chat UI Polish, Engine Dispatcher (Phases 1–39, shipped)
- ✅ **v0.12.0 Chat Redesign** — Phases 40–45 (completed)
- 🚧 **v0.13.0 Chat UX Evolution** — Phase 52 (in progress)

## Phases

<details>
<summary>✅ v0.2.0–v0.11.0 (Phases 1–39) — SHIPPED</summary>

Covered: core platform stability, providers, channels, memory, tools, orchestrator, architecture cleanup, Chat UI Polish, Engine Dispatcher + Security Hardening. See git history and previous milestone artifacts.

</details>

### 🚧 v0.12.0 Chat Redesign (In Progress)

**Milestone Goal:** ChatGPT-level chat UX — fix all known streaming bugs, extract SseConnection, implement state machine, restore history reliably, add reconnect and optimistic UI, clean up dead code.

- [x] **Phase 40: SseConnection Extraction** - Extract SSE transport into a standalone testable class (completed 2026-04-09)
- [x] **Phase 41: ConnectionPhase FSM** - Replace 4-signal thinking indicator with single state machine (completed 2026-04-09)
- [x] **Phase 42: History & MessageSource** - Fix F5 restore; replace viewMode with discriminated union (completed 2026-04-09)
- [x] **Phase 43: Reconnect & Optimistic UI** - Exponential backoff reconnect + React 19 useOptimistic (completed 2026-04-09)
- [x] **Phase 44: UX Polish** - Draft persistence, scroll behavior, error state UI (completed 2026-04-09)
- [x] **Phase 45: Cleanup** - Remove deprecated flags, move module-globals into AgentState (completed 2026-04-09)

## Phase Details

### Phase 40: SseConnection Extraction
**Goal**: SSE transport logic lives in an isolated, testable class — no behavior change for the user
**Depends on**: Nothing (first phase of this milestone)
**Requirements**: SSE-01
**Success Criteria** (what must be TRUE):
  1. A `SseConnection` class exists in `lib/sse-connection.ts` and can be unit-tested without React or Zustand
  2. Chat sends messages and receives streaming responses exactly as before (no regressions)
  3. The class accepts `onEvent` and `onPhaseChange` callbacks; the store's `startStream` creates an instance and delegates to it
  4. Existing SSE parsing tests (if any) pass against the extracted class
**Plans:** 1/1 plans complete
Plans:
- [x] 40-01-PLAN.md — Extract SseConnection class, wire into chat-store, verify zero regression
**UI hint**: yes

### Phase 41: ConnectionPhase FSM
**Goal**: A single `ConnectionPhase` enum replaces 4 conflicting boolean signals; all three thinking indicator bugs are fixed
**Depends on**: Phase 40
**Requirements**: FSM-01, FSM-02, FSM-03, FSM-04
**Success Criteria** (what must be TRUE):
  1. Opening a new empty chat shows no thinking indicator — the spinner never appears before a message is sent
  2. Sending a message shows the thinking indicator; it disappears as soon as the first assistant text token arrives
  3. After the stream completes (finish event), the thinking indicator is gone and the full response is visible — no residual spinner
  4. In a multi-agent session, switching between agent turns does not leave an orphaned thinking indicator from the previous turn
  5. `IncrementalParser` resets between agent turns — second agent's text is not misclassified as reasoning
**Plans:** 2/2 plans complete
Plans:
- [x] 41-01-PLAN.md — Add ConnectionPhase enum to AgentState, wire transitions, add IncrementalParser.reset()
- [x] 41-02-PLAN.md — Replace 4-signal showThinking with connectionPhase, remove sessionStorage flag, human verify
**UI hint**: yes

### Phase 42: History & MessageSource
**Goal**: F5 page reload shows history immediately with no ghost avatars; `viewMode` dual-semantics are eliminated
**Depends on**: Phase 41
**Requirements**: HIST-01, HIST-02, HIST-03
**Success Criteria** (what must be TRUE):
  1. Pressing F5 during an idle session loads the full conversation history instantly — no blank screen, no ghost thinking avatar
  2. Pressing F5 while a stream is actively running shows the history seed and then continues showing live tokens as they arrive
  3. Switching agents while agent A is streaming does not kill agent A's stream — both agents' state is independent
  4. Selecting a completed session from the sidebar shows the correct history without duplicate user messages
**Plans:** 1/2 plans complete
Plans:
- [x] 42-01-PLAN.md — Replace viewMode+liveMessages with MessageSource union; per-agent streamGeneration
- [ ] 42-02-PLAN.md — Fix F5 history restore; seed resumeStream from cache; human verify
**UI hint**: yes

### Phase 43: Reconnect & Optimistic UI
**Goal**: Network drops retry automatically; user messages appear instantly with rollback on failure
**Depends on**: Phase 40
**Requirements**: SSE-02, SSE-03
**Success Criteria** (what must be TRUE):
  1. When the SSE connection drops mid-stream (simulated network blip), the chat automatically reconnects using exponential backoff (1 s, 2 s, 4 s…) without user action
  2. After max retries are exhausted, the UI shows an explicit connection-lost error with a retry button — no silent failure
  3. Sending a message shows the user bubble immediately before the server confirms receipt
  4. If the send fails (server error or network timeout), the optimistic user bubble is rolled back and an error indicator appears inline
**Plans:** 1/2 plans complete
Plans:
- [x] 43-01-PLAN.md — Add reconnect with exponential backoff to SseConnection and wire into chat-store
- [ ] 43-02-PLAN.md — Optimistic user message status tracking with error rollback UI
**UI hint**: yes

### Phase 44: UX Polish
**Goal**: Input drafts survive reloads; scroll behaves predictably; errors are actionable
**Depends on**: Nothing (independent of architecture phases)
**Requirements**: UX-01, UX-02, UX-03
**Success Criteria** (what must be TRUE):
  1. Typing a message, navigating away, and returning restores the draft text in the input field
  2. During streaming, the message list auto-scrolls to follow new tokens; when the user scrolls up manually, auto-scroll pauses and resumes when the user scrolls back to the bottom
  3. Connection lost, API error, and timeout each show a distinct error UI with a labeled retry button — no generic "Stream failed" banner
**Plans:** 1/2 plans complete
Plans:
- [x] 44-01-PLAN.md — Draft persistence in localStorage + consolidate scroll to single followOutput authority
- [ ] 44-02-PLAN.md — Classified error state UI with distinct icons, labels, and retry actions
**UI hint**: yes

### Phase 45: Cleanup
**Goal**: Dead code and deprecated state fields are removed; module-scope globals are gone
**Depends on**: Phase 41, Phase 42, Phase 43
**Requirements**: CLN-01, CLN-02
**Success Criteria** (what must be TRUE):
  1. `viewMode`, `sessionStorage` streaming flag (`hydeclaw.streaming.*`), and `thinkingSessionId` are absent from the codebase — searches return no results
  2. `agentAbortControllers` and `streamGeneration` module-scope globals are removed; their responsibilities live inside `AgentState` or `SseConnection`
  3. Full chat regression passes: send, stream, stop, F5 restore, agent switch, error recovery all work correctly
**Plans:** 1/1 plans complete
Plans:
- [x] 45-01-PLAN.md — Remove deprecated fields (CLN-01) and module-scope globals (CLN-02)
**UI hint**: yes

### Phase 52: Citations & Generative UI
**Goal**: Rich-card rendering via extensible CARD_REGISTRY with ErrorBoundary isolation and citation footnotes
**Depends on**: Phase 45
**Requirements**: GENUI-01, GENUI-02
**Success Criteria** (what must be TRUE):
  1. CARD_REGISTRY maps card type strings to React components; new types added by registering, not editing conditionals
  2. Unknown card types render as formatted JSON without crashing
  3. CardErrorBoundary wraps each GenerativeUISlot -- broken cards show fallback
**Plans:** 1/3 plans complete
Plans:
- [x] 52-01-PLAN.md -- CARD_REGISTRY, GenerativeUISlot, CardErrorBoundary + wiring
- [ ] 52-02-PLAN.md
- [ ] 52-03-PLAN.md
**UI hint**: yes

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 40. SseConnection Extraction | v0.12.0 | 1/1 | Complete    | 2026-04-09 |
| 41. ConnectionPhase FSM | v0.12.0 | 2/2 | Complete    | 2026-04-09 |
| 42. History & MessageSource | v0.12.0 | 1/2 | Complete    | 2026-04-09 |
| 43. Reconnect & Optimistic UI | v0.12.0 | 1/2 | Complete    | 2026-04-09 |
| 44. UX Polish | v0.12.0 | 1/2 | Complete    | 2026-04-09 |
| 45. Cleanup | v0.12.0 | 1/1 | Complete    | 2026-04-09 |
| 52. Citations & Generative UI | v0.13.0 | 1/3 | In Progress | — |
