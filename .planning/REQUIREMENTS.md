# Requirements: HydeClaw v0.13.0

**Defined:** 2026-04-09
**Core Value:** Стабильная и безопасная AI-платформа с self-hosted фокусом

## v0.13.0 Requirements

Requirements for Chat UX Evolution milestone. Stability, responsiveness, advanced UX patterns.

### Streaming Performance

- [x] **PERF-01**: rAF-throttled rendering — токены буферизуются и сбрасываются в UI синхронно с requestAnimationFrame
- [x] **PERF-02**: Incremental markdown parsing — stream-friendly парсер достраивает DOM, а не перестраивает заново
- [x] **PERF-03**: Deferred syntax highlighting — подсветка кода применяется после завершения блока или через Web Worker

### Scroll & Navigation

- [x] **SCRL-01**: CSS `overflow-anchor: auto` на контейнере чата
- [x] **SCRL-02**: Smart sticky logic — авто-скролл отключается при ручном скролле вверх >100px
- [ ] **SCRL-03**: Floating "scroll to bottom" кнопка с индикатором новых токенов

### Optimistic & Responsive UI

- [x] **OPTI-01**: Instant thinking indicator — анимация "думания" запускается локально сразу при send, до первого SSE байта
- [x] **OPTI-02**: Skeleton sync при переключении агентов — shape-matched скелетные превью
- [ ] **OPTI-03**: Content hash для live→history перехода — без визуального мерцания при замене
- [ ] **OPTI-04**: Reference stability — message.id совпадает между live и DB для переиспользования DOM узлов

### Virtualization

- [x] **VIRT-01**: `increaseViewportBy` для плавной подгрузки медиа при скролле вверх
- [ ] **VIRT-02**: DOM node capping — тяжёлые элементы (Rich Cards, iframes) заменяются заглушками при уходе из viewport

### Network Resilience

- [ ] **NET-01**: Last-Event-ID resume при reconnect — продолжение с последней полученной позиции
- [ ] **NET-02**: Reconnecting phase с pulsating анимацией и статусом

### SSE Protocol

- [ ] **SSE-01**: Automatic Continuations — ContinuationStart SSE event + frontend separator при finish_reason=length
- [ ] **SSE-02**: Data Stream Protocol — start-step/finish-step events для структурированной группировки шагов

### Agent UX

- [ ] **AGNT-01**: Agent Handoff UI — HandoffMetadata SSE event + avatar/label switch mid-stream

### Human-in-the-Loop

- [ ] **HITL-01**: Inline approve/reject в ленте чата — ApprovalRequest SSE event + SSE heartbeat при ожидании
- [ ] **HITL-02**: Edit tool args — CodeMirror JSON editor аргументов + modified_args в approval API

### Message Branching

- [ ] **BRNC-01**: DB migration для веток (branch_from_message_id + parent tracking)
- [ ] **BRNC-02**: Backend fork endpoint — создание ветки от выбранного сообщения
- [ ] **BRNC-03**: MessageTree store model — замена flat массива на дерево
- [ ] **BRNC-04**: Branch navigation UI — переключение между версиями (1/2, 2/2)

### Generative UI

- [ ] **GENUI-01**: Static CARD_REGISTRY для rich-card типов + GenerativeUISlot с ErrorBoundary
- [ ] **GENUI-02**: Первые 2-3 зарегистрированных компонента для существующих инструментов

### Citations

- [ ] **CITE-01**: Source citation footnote tooltips — всплывающие подсказки при наведении на сноски

## Future Requirements

- **FUT-01**: Mobile-optimized responsive layout
- **FUT-02**: Custom themes / dark mode variants
- **FUT-03**: Pre-fetching next action (connection pre-warming on hover)

## Out of Scope

| Feature | Reason |
|---------|--------|
| Vercel AI SDK adoption | Requires protocol rewrite, incompatible with custom SSE |
| assistant-ui library | Protocol coupling, static export incompatible with RSC |
| Real-time collaboration | Not needed for self-hosted single-user |
| Voice input in chat | Separate feature, channels handle this |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| PERF-01 | Phase 46 | Complete |
| PERF-02 | Phase 46 | Complete |
| PERF-03 | Phase 46 | Complete |
| SCRL-01 | Phase 47 | Complete |
| SCRL-02 | Phase 47 | Complete |
| SCRL-03 | Phase 47 | Pending |
| VIRT-01 | Phase 47 | Complete |
| VIRT-02 | Phase 47 | Pending |
| OPTI-01 | Phase 48 | Complete |
| OPTI-02 | Phase 48 | Complete |
| OPTI-03 | Phase 48 | Pending |
| OPTI-04 | Phase 48 | Pending |
| NET-01 | Phase 49 | Pending |
| NET-02 | Phase 49 | Pending |
| SSE-01 | Phase 50 | Pending |
| SSE-02 | Phase 50 | Pending |
| AGNT-01 | Phase 50 | Pending |
| HITL-01 | Phase 51 | Pending |
| HITL-02 | Phase 51 | Pending |
| CITE-01 | Phase 52 | Pending |
| GENUI-01 | Phase 52 | Pending |
| GENUI-02 | Phase 52 | Pending |
| BRNC-01 | Phase 53 | Pending |
| BRNC-02 | Phase 53 | Pending |
| BRNC-03 | Phase 53 | Pending |
| BRNC-04 | Phase 53 | Pending |

**Coverage:**
- v0.13.0 requirements: 26 total
- Mapped to phases: 26
- Unmapped: 0

---
*Requirements defined: 2026-04-09*
*Last updated: 2026-04-09 after roadmap creation (Phases 46–53)*
