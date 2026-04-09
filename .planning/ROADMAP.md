# Roadmap: HydeClaw

## Milestones

- ✅ **v0.2.0–v0.11.0** — Core platform, Chat UI Polish, Engine Dispatcher (Phases 1–39, shipped)
- ✅ **v0.12.0 Chat Redesign** — Phases 40–45 (completed 2026-04-09)
- 🚧 **v0.13.0 Chat UX Evolution** — Phases 46–53 (in progress)

## Phases

<details>
<summary>✅ v0.2.0–v0.11.0 (Phases 1–39) — SHIPPED</summary>

Covered: core platform stability, providers, channels, memory, tools, orchestrator, architecture cleanup, Chat UI Polish, Engine Dispatcher + Security Hardening. See git history and previous milestone artifacts.

</details>

<details>
<summary>✅ v0.12.0 Chat Redesign (Phases 40–45) — COMPLETED</summary>

- [x] **Phase 40: SseConnection Extraction** - Extract SSE transport into a standalone testable class (completed 2026-04-09)
- [x] **Phase 41: ConnectionPhase FSM** - Replace 4-signal thinking indicator with single state machine (completed 2026-04-09)
- [x] **Phase 42: History & MessageSource** - Fix F5 restore; replace viewMode with discriminated union (completed 2026-04-09)
- [x] **Phase 43: Reconnect & Optimistic UI** - Exponential backoff reconnect + React 19 useOptimistic (completed 2026-04-09)
- [x] **Phase 44: UX Polish** - Draft persistence, scroll behavior, error state UI (completed 2026-04-09)
- [x] **Phase 45: Cleanup** - Remove deprecated flags, move module-globals into AgentState (completed 2026-04-09)

</details>

### 🚧 v0.13.0 Chat UX Evolution (In Progress)

**Milestone Goal:** Advanced UX patterns — continuations, branching, human-in-the-loop, generative UI, streaming performance, scroll anchoring, quick wins.

- [ ] **Phase 46: Streaming Performance** - rAF-throttled rendering, incremental markdown, deferred syntax highlighting
- [ ] **Phase 47: Scroll & Virtualization** - CSS overflow-anchor, smart sticky logic, floating scroll button, viewport-aware DOM capping
- [ ] **Phase 48: Optimistic & Responsive UI** - Instant thinking indicator, agent-switch skeletons, live-to-history hash sync, reference stability
- [ ] **Phase 49: Network Resilience** - Last-Event-ID resume on reconnect, reconnecting phase UI
- [ ] **Phase 50: SSE Protocol Extensions** - Automatic continuations, step grouping events, agent handoff mid-stream
- [ ] **Phase 51: Human-in-the-Loop** - Inline approve/reject with SSE heartbeat, tool args editor
- [ ] **Phase 52: Citations & Generative UI** - Source footnote tooltips, CARD_REGISTRY, first registered components
- [ ] **Phase 53: Message Branching** - DB migration, fork endpoint, MessageTree store, branch navigation UI

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

### Phase 46: Streaming Performance
**Goal**: Token rendering is smooth and non-blocking — rAF-throttled, incrementally parsed, with deferred syntax highlighting
**Depends on**: Phase 45
**Requirements**: PERF-01, PERF-02, PERF-03
**Success Criteria** (what must be TRUE):
  1. Streaming 500+ token responses shows no perceptible UI jank — browser frame rate stays above 50fps during active streaming
  2. The markdown renderer updates incrementally as tokens arrive without tearing or full DOM rebuilds — existing rendered text does not repaint during new token appends
  3. Code blocks inside streaming responses do not trigger syntax highlighting mid-stream — highlighting fires only after the closing fence token arrives or the stream ends
  4. Toggling streaming off and on again shows no regression in any of the above behaviors
**Plans**: 3 plans
Plans:
- [ ] 46-01-PLAN.md — Test scaffold: PERF-01 regression tests (green) + PERF-02/03 stubs (red until plan 02)
- [ ] 46-02-PLAN.md — Stable block keys (PERF-02) + deferred syntax highlighting via isStreaming prop thread (PERF-03)
- [ ] 46-03-PLAN.md — Human verify: browser streaming UX check
**UI hint**: yes

### Phase 47: Scroll & Virtualization
**Goal**: The chat list scrolls predictably, stays anchored during streaming, and degrades gracefully for long conversations
**Depends on**: Phase 46
**Requirements**: SCRL-01, SCRL-02, SCRL-03, VIRT-01, VIRT-02
**Success Criteria** (what must be TRUE):
  1. During streaming, new tokens push the list down without the viewport jumping — the user's reading position stays locked (`overflow-anchor: auto`)
  2. When the user scrolls up more than 100px, auto-scroll pauses; scrolling back to the bottom resumes auto-scroll automatically
  3. A floating "scroll to bottom" button appears when the user is not at the bottom and shows a badge with the count of new tokens received while scrolled away
  4. Media-heavy messages (images, rich cards) outside the visible viewport load lazily without triggering layout shifts for on-screen content
  5. Rich cards and iframes that scroll out of view are replaced with lightweight placeholders — DOM node count stays bounded for conversations exceeding 200 messages
**Plans**: TBD
**UI hint**: yes

### Phase 48: Optimistic & Responsive UI
**Goal**: Every user action responds instantly — no perceived latency before the first SSE byte arrives
**Depends on**: Phase 45
**Requirements**: OPTI-01, OPTI-02, OPTI-03, OPTI-04
**Success Criteria** (what must be TRUE):
  1. The thinking indicator animation starts the moment the user taps Send — before any SSE event is received from the backend
  2. Switching agents shows a shape-matched skeleton preview for the expected message layout before history loads
  3. When the stream ends and history replaces live messages, no visual flicker or blank-frame transition occurs — content hash comparison prevents unnecessary re-renders
  4. The `message.id` assigned during live streaming matches the `id` stored in the database, so React can reuse the same DOM node when switching from live to history view
**Plans**: TBD
**UI hint**: yes

### Phase 49: Network Resilience
**Goal**: SSE streams resume from the last received position after network drops, with visible reconnecting state
**Depends on**: Phase 43
**Requirements**: NET-01, NET-02
**Success Criteria** (what must be TRUE):
  1. After a network interruption, the chat reconnects and sends `Last-Event-ID` in the request header — the backend resumes from the last delivered event position without re-sending already-shown tokens
  2. While attempting to reconnect, the chat shows a pulsating "Reconnecting…" indicator with the retry attempt number — the user knows the system is working without manual action
  3. After a successful reconnect mid-stream, the conversation continues exactly where it left off — no duplicate content, no missing tokens
**Plans**: TBD
**UI hint**: yes

### Phase 50: SSE Protocol Extensions
**Goal**: The SSE stream carries structured continuation, step-grouping, and agent handoff metadata — the UI handles each transparently
**Depends on**: Phase 49
**Requirements**: SSE-01, SSE-02, AGNT-01
**Success Criteria** (what must be TRUE):
  1. When the LLM hits a token-limit finish, the UI automatically stitches the continuation — a subtle visual separator appears between continuations and the assistant response reads as one uninterrupted block
  2. Tool execution steps are visually grouped using start-step/finish-step event boundaries — the user sees a structured "thinking" expansion rather than a flat stream of tool events
  3. When an agent handoff occurs mid-stream, the avatar and agent label switch smoothly at the handoff point — no flash, no full re-render of the message list
**Plans**: TBD
**UI hint**: yes

### Phase 51: Human-in-the-Loop
**Goal**: Dangerous tool calls pause inline for user review — the stream stays alive during the wait, and users can approve, reject, or edit tool arguments
**Depends on**: Phase 50
**Requirements**: HITL-01, HITL-02
**Success Criteria** (what must be TRUE):
  1. When an agent requests approval for a tool call, an inline approve/reject card appears in the chat feed — the connection does not drop while waiting (SSE heartbeat keeps it alive through nginx's 60-second timeout)
  2. The user can approve or reject directly from the chat feed without navigating away — the agent continues immediately after the decision
  3. The user can open a JSON editor for the tool's input arguments, modify them, and submit the modified args — the agent receives and uses the edited values
  4. If the user takes no action within the timeout period, the tool call is automatically rejected and the agent receives a rejection result
**Plans**: TBD
**UI hint**: yes

### Phase 52: Citations & Generative UI
**Goal**: Markdown citations render as interactive footnotes; tool results render as typed rich components via a static registry
**Depends on**: Phase 45
**Requirements**: CITE-01, GENUI-01, GENUI-02
**Success Criteria** (what must be TRUE):
  1. Hovering over a footnote reference in assistant messages shows a tooltip with the cited source text or URL — no page navigation required
  2. A `CARD_REGISTRY` maps rich-card type strings to React components — unknown types fall back to a raw JSON display without crashing
  3. An `ErrorBoundary` wraps each `GenerativeUISlot` — a broken card component logs the error and shows a fallback, never crashing the chat feed
  4. At least two existing tool types (e.g., image result, search result) render as registered card components instead of raw text output
**Plans**: TBD
**UI hint**: yes

### Phase 53: Message Branching
**Goal**: Users can edit past messages and navigate between conversation branches — the tree model replaces the flat message array
**Depends on**: Phase 52
**Requirements**: BRNC-01, BRNC-02, BRNC-03, BRNC-04
**Success Criteria** (what must be TRUE):
  1. The database schema has `branch_from_message_id` and parent tracking columns — migration runs cleanly on the existing dataset without data loss
  2. Editing a user message in the middle of a conversation creates a new branch — the original messages are preserved and a fork endpoint returns the new branch session ID
  3. A "1/2 → 2/2" navigation control appears on messages that have sibling branches — clicking cycles between versions
  4. The MessageTree store correctly models parent/child relationships — switching branches updates the visible message list to show the selected branch's ancestry path
**Plans**: TBD
**UI hint**: yes

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 40. SseConnection Extraction | v0.12.0 | 1/1 | Complete | 2026-04-09 |
| 41. ConnectionPhase FSM | v0.12.0 | 2/2 | Complete | 2026-04-09 |
| 42. History & MessageSource | v0.12.0 | 1/2 | Complete | 2026-04-09 |
| 43. Reconnect & Optimistic UI | v0.12.0 | 1/2 | Complete | 2026-04-09 |
| 44. UX Polish | v0.12.0 | 1/2 | Complete | 2026-04-09 |
| 45. Cleanup | v0.12.0 | 1/1 | Complete | 2026-04-09 |
| 46. Streaming Performance | v0.13.0 | 0/? | Not started | - |
| 47. Scroll & Virtualization | v0.13.0 | 0/? | Not started | - |
| 48. Optimistic & Responsive UI | v0.13.0 | 0/? | Not started | - |
| 49. Network Resilience | v0.13.0 | 0/? | Not started | - |
| 50. SSE Protocol Extensions | v0.13.0 | 0/? | Not started | - |
| 51. Human-in-the-Loop | v0.13.0 | 0/? | Not started | - |
| 52. Citations & Generative UI | v0.13.0 | 0/? | Not started | - |
| 53. Message Branching | v0.13.0 | 0/? | Not started | - |
