# Requirements: HydeClaw v0.12.0

**Defined:** 2026-04-09
**Core Value:** Стабильная и безопасная AI-платформа с self-hosted фокусом

## v0.12.0 Requirements

Requirements for Chat Redesign milestone. ChatGPT-level UX quality.

### Chat State Machine

- [x] **FSM-01**: ConnectionPhase enum (idle → submitted → streaming → complete → error) заменяет 4 boolean flags
- [x] **FSM-02**: ThinkingMessage рендерится ТОЛЬКО когда phase=submitted AND нет assistant parts
- [x] **FSM-03**: finish SSE event атомарно переводит state в idle и очищает все streaming флаги
- [x] **FSM-04**: IncrementalParser сбрасывается между agent turns в multi-agent сессии

### History & Restore

- [ ] **HIST-01**: F5 → history загружается мгновенно, никаких ghost thinking avatars
- [ ] **HIST-02**: MessageSource discriminated union заменяет viewMode + liveMessages двойственность
- [ ] **HIST-03**: streamGeneration counter per-agent (не module-scope) — смена агента не убивает чужой stream

### SSE Connection

- [x] **SSE-01**: SseConnection class извлечён из Zustand store — testable в изоляции
- [ ] **SSE-02**: Exponential backoff reconnect при обрыве соединения
- [ ] **SSE-03**: Optimistic user message с rollback при ошибке (useOptimistic React 19)

### UX Polish

- [ ] **UX-01**: Input draft persistence — незавершённый текст сохраняется в localStorage
- [ ] **UX-02**: Scroll behavior — единый авторитетный источник (followOutput)
- [ ] **UX-03**: Error state UI — connection lost, API error, timeout с retry кнопками

### Cleanup

- [ ] **CLN-01**: Удалить deprecated viewMode, sessionStorage streaming flag, thinkingSessionId
- [ ] **CLN-02**: Удалить module-scope globals — переместить в AgentState

## Future Requirements

- **FUT-01**: SSE auto-reconnect with exponential backoff
- **FUT-02**: Message edit + re-generate from any point
- **FUT-03**: Mobile-optimized responsive layout

## Out of Scope

| Feature                    | Reason                                    |
| -------------------------- | ----------------------------------------- |
| Real-time collaboration    | Not needed for self-hosted single-user    |
| Voice input in chat        | Separate feature, channels handle this    |
| Custom themes              | Incremental, not architectural            |

## Traceability

| Requirement | Phase    | Status  |
| ----------- | -------- | ------- |
| FSM-01      | Phase 41 | Complete |
| FSM-02      | Phase 41 | Complete |
| FSM-03      | Phase 41 | Complete |
| FSM-04      | Phase 41 | Complete |
| HIST-01     | Phase 42 | Pending |
| HIST-02     | Phase 42 | Pending |
| HIST-03     | Phase 42 | Pending |
| SSE-01      | Phase 40 | Complete |
| SSE-02      | Phase 43 | Pending |
| SSE-03      | Phase 43 | Pending |
| UX-01       | Phase 44 | Pending |
| UX-02       | Phase 44 | Pending |
| UX-03       | Phase 44 | Pending |
| CLN-01      | Phase 45 | Pending |
| CLN-02      | Phase 45 | Pending |
