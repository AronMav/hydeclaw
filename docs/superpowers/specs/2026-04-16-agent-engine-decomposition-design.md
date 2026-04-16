# AgentEngine Decomposition Design

**Date:** 2026-04-16
**Status:** Approved
**Scope:** Полная декомпозиция god-object `AgentEngine` (8617 строк, 16 файлов, 24 поля) — третий и последний из серии (AppState done, MemoryStore done → AgentEngine)

---

## Проблема

`AgentEngine` — центральный объект системы: 24 поля в одном struct, 16 extension-файлов (`engine_*.rs`) общим объёмом 8617 строк. Все методы — `&self`, все читают произвольные поля. Следствия:

- Невозможно тестировать один pipeline step без создания полного engine с 24 зависимостями
- `processing_session_id` — shared mutable state, race при concurrent SSE (BUG-007)
- SIGHUP reload создаёт dual engines racing на workspace (BUG-006)
- Shutdown убивает backend-сервисы до завершения agent loop (BUG-008)
- LoopDetector не восстанавливается после crash (BUG-026)

---

## Решение: Pipeline Architecture

Engine разбивается на тройку типов. Execution переходит от method calls на `&self` к free functions с explicit dependencies.

### Тройка типов

**AgentConfig** — immutable snapshot, `Arc<AgentConfig>`. Пересоздаётся только при SIGHUP reload.

```rust
pub struct AgentConfig {
    // Identity
    pub agent: AgentSettings,
    pub workspace_dir: String,
    pub default_timezone: String,
    pub app_config: Arc<AppConfig>,

    // LLM
    pub provider: Arc<dyn LlmProvider>,
    pub compaction_provider: Option<Arc<dyn LlmProvider>>,

    // Data
    pub db: PgPool,
    pub memory_store: Arc<dyn MemoryService>,
    pub embedder: Arc<dyn EmbeddingService>,

    // Tools
    pub tools: ToolRegistry,
    pub tool_executor: Arc<DefaultToolExecutor>,
    pub approval_manager: Arc<ApprovalManager>,

    // Infra
    pub scheduler: Option<Arc<Scheduler>>,
    pub agent_map: Option<AgentMap>,
    pub session_pools: Option<SessionPoolsMap>,
    pub audit_queue: Arc<AuditQueue>,
}
```

**AgentState** — per-agent mutable state, `Arc<AgentState>`. Interior mutability, shared между requests.

```rust
pub struct AgentState {
    pub thinking_level: AtomicU8,
    pub channel_formatting_prompt: RwLock<Option<String>>,
    pub channel_info_cache: RwLock<Option<Vec<ChannelInfo>>>,
    pub processing_tracker: Option<ProcessingTracker>,
    pub channel_router: Option<ChannelActionRouter>,
    pub ui_event_tx: Option<broadcast::Sender<String>>,
    pub active_requests: Mutex<Vec<CancellationToken>>,
}
```

**RequestContext** — per-request owned state. Создаётся при входе в `handle_sse`/`handle`, умирает после завершения.

```rust
pub struct RequestContext {
    pub session_id: Uuid,
    pub message_id: String,
    pub cancel: CancellationToken,
    pub loop_detector: LoopDetector,
    pub sse_tx: Option<mpsc::UnboundedSender<StreamEvent>>,
    pub leaf_message_id: Option<String>,
}
```

---

## Pipeline Steps

Extension methods на `&self` становятся free functions с explicit dependencies.

### Структура файлов

```
src/agent/
├── mod.rs               — pub mod declarations
├── config.rs            — AgentConfig struct
├── agent_state.rs       — AgentState struct
├── request_context.rs   — RequestContext struct
├── engine.rs            — AgentEngine thin wrapper (factory)
├── handle.rs            — AgentHandle (lifecycle, unchanged)
├── pipeline/
│   ├── mod.rs           — pub mod declarations
│   ├── entry.rs         — handle_sse, handle_channel (entry points)
│   ├── execution.rs     — execute_with_tools (main LLM loop)
│   ├── context.rs       — build_context (system prompt + workspace + memory)
│   ├── llm_call.rs      — call_llm (provider call + streaming)
│   ├── parallel.rs      — execute_parallel (parallel tool batch)
│   ├── dispatch.rs      — dispatch_tool (single tool dispatch)
│   ├── tool_defs.rs     — tool definitions assembly
│   ├── memory.rs        — augment_with_memory, knowledge extraction
│   ├── commands.rs      — slash commands (/status, /clear, etc.)
│   ├── handlers.rs      — tool result handlers (channel actions, file saves)
│   ├── sandbox.rs       — code_exec tool execution
│   ├── subagent.rs      — agent tool (run/message/status/kill)
│   ├── agent_tool.rs    — session agent pool operations
│   └── sessions.rs      — session CRUD, WAL warm-up
├── tool_executor.rs     — DefaultToolExecutor (unchanged)
├── approval_manager.rs  — ApprovalManager (unchanged)
├── session_agent_pool.rs — SessionAgentPool (unchanged)
└── ... (other unchanged files)
```

### Function signatures

```rust
// pipeline/entry.rs
pub async fn handle_sse(
    cfg: &AgentConfig,
    state: &AgentState,
    ctx: RequestContext,
    messages: Vec<Message>,
) -> Result<()>;

// pipeline/execution.rs
pub async fn execute_with_tools(
    cfg: &AgentConfig,
    state: &AgentState,
    ctx: &mut RequestContext,
    messages: &mut Vec<Message>,
) -> Result<()>;

// pipeline/context.rs
pub async fn build_context(
    cfg: &AgentConfig,
    ctx: &RequestContext,
) -> Result<Vec<Message>>;

// pipeline/llm_call.rs
pub async fn call_llm(
    cfg: &AgentConfig,
    ctx: &RequestContext,
    messages: &[Message],
) -> Result<LlmResponse>;

// pipeline/dispatch.rs
pub async fn dispatch_tool(
    cfg: &AgentConfig,
    state: &AgentState,
    ctx: &mut RequestContext,
    call: ToolCall,
) -> ToolResult;

// pipeline/parallel.rs
pub async fn execute_parallel(
    cfg: &AgentConfig,
    state: &AgentState,
    ctx: &mut RequestContext,
    calls: Vec<ToolCall>,
) -> Vec<ToolResult>;
```

### AgentEngine — thin factory

```rust
pub struct AgentEngine {
    pub cfg: Arc<AgentConfig>,
    pub state: Arc<AgentState>,
}

impl AgentEngine {
    pub fn new(cfg: Arc<AgentConfig>, state: Arc<AgentState>) -> Self {
        Self { cfg, state }
    }

    pub async fn handle_sse(&self, messages: Vec<Message>, ...) -> Result<()> {
        let ctx = RequestContext::new(
            session_id,
            self.state.register_request(), // returns CancellationToken, adds to active_requests
        );
        let result = pipeline::entry::handle_sse(&self.cfg, &self.state, ctx).await;
        self.state.unregister_request(&ctx.cancel);
        result
    }

    // Accessors for backward compat during migration
    pub fn name(&self) -> &str { &self.cfg.agent.name }
    pub fn agent(&self) -> &AgentSettings { &self.cfg.agent }
    pub fn db_pool(&self) -> &PgPool { &self.cfg.db }
}
```

---

## 4 архитектурных бага — решаются архитектурой

### BUG-006: SIGHUP dual engines

SIGHUP handler:
1. Создаёт новый `AgentConfig` + `AgentState`
2. Вызывает `old_state.cancel_all_requests()` — итерирует `active_requests`, вызывает `cancel()` на каждом
3. Ожидает drain: `old_state.wait_drain(timeout: 10s).await`
4. Заменяет engine в map
5. Старые in-flight requests завершаются gracefully (pipeline steps проверяют `ctx.cancel.is_cancelled()`)

### BUG-007: processing_session_id удаляется

`processing_session_id` → `ctx.session_id`. Per-request, не shared. Все 15 reference sites переписываются на `ctx.session_id`. Zero races.

### BUG-008: Ordered shutdown

```
SIGTERM
  → for each agent: state.cancel_all_requests()
  → await all agents drained (10s timeout)
  → process_manager.stop_all()  // toolgate/channels still alive during drain
  → exit
```

### BUG-026: WAL warm-up

```rust
impl RequestContext {
    pub async fn new_for_session(db: &PgPool, session_id: Uuid, loop_config: &LoopConfig) -> Self {
        let mut detector = LoopDetector::new(loop_config);
        // Read WAL events and replay into detector
        if let Ok(events) = db::session_wal::load_tool_events(db, session_id).await {
            for event in events {
                detector.record_execution(&event.tool_name, event.success);
            }
        }
        Self {
            session_id,
            loop_detector: detector,
            cancel: CancellationToken::new(),
            ..
        }
    }
}
```

Требует добавления `load_tool_events()` в `session_wal.rs`.

---

## Тестирование (TDD)

### Тестируемость pipeline steps

Каждый step — free function. Тесты подставляют fake dependencies:

```rust
#[tokio::test]
async fn test_build_context_includes_system_prompt() {
    let cfg = AgentConfig::test_minimal();
    let ctx = RequestContext::test_new();
    let messages = pipeline::context::build_context(&cfg, &ctx).await.unwrap();
    assert!(messages[0].role == "system");
}

#[tokio::test]
async fn test_cancellation_stops_tool_loop() {
    let cfg = AgentConfig::test_minimal();
    let state = AgentState::test_new();
    let ctx = RequestContext::test_new();
    ctx.cancel.cancel(); // pre-cancel
    let result = pipeline::execution::execute_with_tools(&cfg, &state, &mut ctx, &mut vec![]).await;
    assert!(result.is_err()); // cancelled
}
```

### Test constructors

```rust
impl AgentConfig {
    #[cfg(test)]
    pub fn test_minimal() -> Arc<Self> { /* lazy pools, mock provider, empty tools */ }
}

impl AgentState {
    #[cfg(test)]
    pub fn test_new() -> Arc<Self> { /* defaults */ }
}

impl RequestContext {
    #[cfg(test)]
    pub fn test_new() -> Self { /* random session_id, fresh detector, no sse_tx */ }
}
```

---

## План миграции

6 фаз, каждая компилируется и тестируется отдельно:

### Фаза 1: Создать типы (не ломает ничего)

Создать `config.rs`, `agent_state.rs`, `request_context.rs` с тестами. `pipeline/mod.rs` scaffold. Компиляция не ломается — новые типы пока не используются.

### Фаза 2: AgentEngine → thin wrapper

Заменить 24 поля AgentEngine на `cfg: Arc<AgentConfig>` + `state: Arc<AgentState>`. Все extension methods (`engine_*.rs`) обновляются: `self.field` → `self.cfg.field` или `self.state.field`. Accessor methods на engine делегируют в cfg/state.

### Фаза 3: RequestContext + cancel

`handle_sse` создаёт RequestContext. `processing_session_id` удаляется — все 15 reference sites переписываются на `ctx.session_id`. Active request tracking в AgentState. Cancel token проверяется в tool loop.

### Фаза 4: Pipeline extraction (файл за файлом)

Каждый `engine_*.rs` → `pipeline/*.rs`. Порядок от наименее зависимого:
1. `context.rs` — build_context
2. `memory.rs` — memory augmentation
3. `commands.rs` — slash commands
4. `sessions.rs` — session helpers + WAL warm-up
5. `sandbox.rs` — code_exec
6. `handlers.rs` — tool result handlers
7. `dispatch.rs` — tool dispatch
8. `subagent.rs` + `agent_tool.rs` — agent tool
9. `parallel.rs` — parallel execution
10. `tool_defs.rs` — tool definitions
11. `llm_call.rs` — LLM provider call
12. `execution.rs` — main loop
13. `entry.rs` — SSE entry point

### Фаза 5: Shutdown + SIGHUP

Ordered shutdown в main.rs. SIGHUP graceful drain.

### Фаза 6: Cleanup + finalization

Удалить пустые `engine_*.rs`. Tests, clippy, deploy.

---

## Что НЕ входит

- Изменение LLM provider protocol — отдельная задача
- Split на отдельные crates — YAGNI
- Трейт для AgentEngine — не нужен (factory pattern достаточен)
- Изменение SSE event format — не трогаем
- Изменение tool execution model (sequential/parallel) — сохраняем as-is

---

## Критерии готовности

- [ ] `AgentConfig`, `AgentState`, `RequestContext` созданы с test constructors
- [ ] `AgentEngine` — thin wrapper с `cfg` + `state`
- [ ] `processing_session_id` удалён, все 15 sites используют `ctx.session_id`
- [ ] `CancellationToken` в RequestContext, проверяется в tool loop
- [ ] Active request tracking: `AgentState.active_requests`
- [ ] WAL warm-up при создании RequestContext для existing session
- [ ] Pipeline steps — free functions в `pipeline/` директории
- [ ] SIGHUP: cancel → drain → replace
- [ ] Shutdown: cancel → drain → stop processes
- [ ] Старые `engine_*.rs` файлы удалены
- [ ] `cargo check --all-targets` чистый
- [ ] `cargo test` зелёный
- [ ] `cargo clippy --all-targets -- -D warnings` чистый
