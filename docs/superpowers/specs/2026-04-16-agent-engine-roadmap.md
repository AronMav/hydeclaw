# AgentEngine Decomposition Roadmap

**Date:** 2026-04-16
**Status:** Draft — requires brainstorming session before implementation
**Scope:** Рефакторинг god-object `AgentEngine` (engine.rs ~127 КБ) — третий и последний из серии (AppState done, MemoryStore done → AgentEngine)
**Prerequisites:** Bug audit выявил 4 архитектурных проблемы, которые должны быть решены в рамках этого рефакторинга.

---

## Текущее состояние

`engine.rs` — ~127 КБ, центральный LLM-loop. Каждый агент работает как независимый tokio task:
1. Входящее сообщение → build context (system prompt + workspace + memory)
2. Вызов LLM → стрим токенов
3. Парсинг tool calls → execution (sequential/parallel)
4. Цикл до завершения или лимита итераций

Дополнительные файлы: `engine_sse.rs`, `engine_execution.rs`, `engine_parallel.rs`, `engine_subagent.rs`, `engine_memory.rs`, `engine_context.rs`, `engine_commands.rs`, `engine_sandbox.rs`, `engine_dispatch.rs`, `engine_handlers.rs`, `engine_agent_tool.rs`.

---

## Архитектурные баги для решения

### 1. SIGHUP reload: dual engines race (BUG-006, HIGH)

**Проблема:** При hot-reload конфига (SIGHUP) старый engine продолжает in-flight SSE request, новый engine уже в `agents.map`. Оба пишут в workspace файлы с отдельными `LoopDetector`, `memory_md_lock`, tool caches.

**Требуется:** Graceful drain — `CancellationToken` + await завершения текущей задачи перед заменой engine.

**Затрагивает:** `AgentHandle`, `engine_sse.rs`, `main.rs` SIGHUP handler.

### 2. `processing_session_id` shared state (BUG-007, HIGH)

**Проблема:** Одно `Arc<Mutex<Option<Uuid>>>` на engine. Concurrent SSE для одного агента (два таба, retry) → второй перезаписывает session_id → tools получают wrong session pool.

**Требуется:** Per-request context вместо shared engine state. Session ID пробрасывается через весь call chain, а не через shared `processing_session_id`.

**Затрагивает:** `engine_sse.rs`, `engine_execution.rs`, `engine_agent_tool.rs` (~50 мест где `processing_session_id` используется как fallback).

### 3. Graceful shutdown (BUG-008, MEDIUM)

**Проблема:** SIGTERM убивает toolgate/channels через `process_manager.stop_all()` ДО завершения agent loop. Агент ещё 15 секунд шлёт запросы в мёртвые endpoints.

**Требуется:** Ordered shutdown: 1) signal agents to stop → 2) await drain → 3) stop processes.

**Затрагивает:** `handle.rs`, `main.rs` shutdown path.

### 4. WAL crash recovery для LoopDetector (BUG-026, MEDIUM)

**Проблема:** LoopDetector создаётся fresh при каждом входе в сессию. После crash зациклившийся агент получает ещё `break_threshold` итераций до детекции.

**Требуется:** При входе в сессию — read session_events WAL, восстановить состояние LoopDetector из tool_start/tool_end событий.

**Затрагивает:** `engine_sse.rs`, `session_wal.rs`, `tool_loop.rs`.

---

## Предлагаемая архитектура (предварительно)

### Per-Request Context (решает #1 и #2)

Вместо shared `processing_session_id` ввести `RequestContext`:

```rust
pub struct RequestContext {
    pub session_id: Uuid,
    pub message_id: String,
    pub cancel: CancellationToken,
    pub loop_detector: LoopDetector,
}
```

Пробрасывается через все методы engine. SIGHUP handler отменяет `cancel` токен старого request, ожидает завершения, затем заменяет engine.

### Ordered Shutdown (решает #3)

```
SIGTERM received
  → set shutdown flag on all engines
  → engines check flag before next tool iteration → break loop
  → await all engine tasks (with 10s timeout)
  → process_manager.stop_all()
  → exit
```

### WAL Replay (решает #4)

```
enter session
  → read session_events WHERE session_id = X AND event_type IN ('tool_start', 'tool_end')
  → replay into LoopDetector
  → continue with warm detector
```

---

## Scope разбиения engine.rs

Файл 127 КБ содержит ~6 ответственностей, которые можно выделить:

1. **LLM Call Pipeline** — вызов провайдера, стриминг, парсинг ответа
2. **Tool Execution** — dispatch, approval workflow, result handling
3. **Context Building** — system prompt, workspace, memory, skills assembly
4. **Session Management** — WAL, status tracking, retry logic
5. **Subagent Orchestration** — SessionAgentPool, peer messaging
6. **Streaming** — SSE event emission, channel management

Каждый блок уже частично выделен в `engine_*.rs` файлы, но все зависят от `&self` (AgentEngine struct) с ~20 полями.

---

## Критерии готовности рефакторинга

- [ ] `RequestContext` заменяет `processing_session_id`
- [ ] SIGHUP graceful drain работает (old engine завершает request)
- [ ] Ordered shutdown: agents drain → processes stop
- [ ] WAL warm-up для LoopDetector реализован
- [ ] `cargo check --all-targets` чистый
- [ ] `cargo test` зелёный
- [ ] `cargo clippy --all-targets -- -D warnings` чистый
- [ ] Pi deploy + doctor check green

---

## Что НЕ входит

- Полный split engine.rs на отдельные crate — YAGNI на данном этапе
- Изменение LLM provider protocol (Anthropic/OpenAI/Google) — отдельная задача
- Переход на trait-based engine — не нужен для текущих целей
