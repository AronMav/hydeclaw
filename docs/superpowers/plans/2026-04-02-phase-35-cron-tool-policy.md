# Phase 35: Per-Cron Tool Allowlist — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Каждый scheduled job может задать `tool_policy` (allow/deny) которая применяется поверх агентской при запуске job.

**Architecture:** Миграция добавляет `tool_policy JSONB` в `scheduled_jobs`. Rust struct `ScheduledJob` получает поле. API принимает и сохраняет политику. Scheduler передаёт её в `engine.handle_isolated`. UI показывает секцию Tool Policy в диалоге cron job.

**Tech Stack:** Rust (sqlx, serde), TypeScript/React, PostgreSQL

---

## File Map

| Файл | Действие |
|------|----------|
| `migrations/004_cron_tool_policy.sql` | ADD COLUMN tool_policy JSONB |
| `crates/hydeclaw-core/src/scheduler/mod.rs` | Поле `tool_policy` в `ScheduledJob`, передача в engine |
| `crates/hydeclaw-core/src/gateway/handlers/cron.rs` | Принимать `tool_policy` в CreateCronRequest/UpdateCronRequest |
| `crates/hydeclaw-core/src/agent/engine.rs` | `handle_isolated_with_policy` или override механизм |
| `ui/src/app/(authenticated)/tasks/page.tsx` | Секция Tool Policy в диалоге |

---

## Task 1: Миграция — добавить tool_policy в scheduled_jobs

**Files:**
- Create: `migrations/004_cron_tool_policy.sql`

- [ ] **Step 1: Создать файл миграции**

```sql
-- migrations/004_cron_tool_policy.sql
ALTER TABLE scheduled_jobs ADD COLUMN IF NOT EXISTS tool_policy JSONB DEFAULT NULL;

COMMENT ON COLUMN scheduled_jobs.tool_policy IS
  'Optional tool policy override for this job. Format: {"allow": ["tool1"], "deny": ["tool2"]}. Applied on top of agent tool policy.';
```

- [ ] **Step 2: Commit**

```bash
git add migrations/004_cron_tool_policy.sql
git commit -m "feat(db): add tool_policy JSONB column to scheduled_jobs"
```

---

## Task 2: Обновить ScheduledJob struct и SQL queries

**Files:**
- Modify: `crates/hydeclaw-core/src/scheduler/mod.rs`

- [ ] **Step 1: Написать тест**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduled_job_tool_policy_default_none() {
        // Compile-time check that field exists and defaults to None
        let _ = |job: ScheduledJob| {
            let _: Option<crate::config::AgentToolPolicy> = job.tool_policy;
        };
    }

    #[test]
    fn tool_policy_deserialize_from_json() {
        let json = serde_json::json!({"allow": ["memory_search", "searxng_search"], "deny": []});
        let policy: crate::config::AgentToolPolicy = serde_json::from_value(json).unwrap();
        assert_eq!(policy.allow, vec!["memory_search", "searxng_search"]);
        assert!(policy.deny.is_empty());
    }
}
```

- [ ] **Step 2: Запустить тест — убедиться что падает**

```bash
cd crates/hydeclaw-core && cargo test scheduler::tests -- --nocapture 2>&1 | tail -10
```

- [ ] **Step 3: Добавить поле в ScheduledJob**

Найти struct `ScheduledJob` (строки 19-39). Добавить поле:

```rust
pub struct ScheduledJob {
    pub id: Uuid,
    pub agent_id: String,
    pub name: String,
    pub cron_expr: String,
    pub timezone: String,
    pub task_message: String,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    #[sqlx(default)]
    pub silent: bool,
    #[sqlx(default)]
    pub announce_to: Option<serde_json::Value>,
    #[sqlx(default)]
    pub jitter_secs: i32,
    #[sqlx(default)]
    pub run_once: bool,
    #[sqlx(default)]
    pub run_at: Option<chrono::DateTime<chrono::Utc>>,
    #[sqlx(default)]
    pub tool_policy: Option<serde_json::Value>,
}
```

Примечание: хранить как `Option<serde_json::Value>` в struct (для sqlx), конвертировать в `AgentToolPolicy` при использовании.

- [ ] **Step 4: Обновить SELECT запросы**

Найти все SQL `SELECT` запросы к `scheduled_jobs` в `scheduler/mod.rs` и `gateway/handlers/cron.rs`. В каждый добавить `tool_policy` в список колонок:

```sql
SELECT id, agent_id, name, cron_expr, timezone, task_message, enabled, created_at, last_run_at, silent, announce_to, jitter_secs, run_once, run_at, tool_policy \
FROM scheduled_jobs ...
```

- [ ] **Step 5: Запустить тест**

```bash
cd crates/hydeclaw-core && cargo test scheduler::tests -- --nocapture
```
Ожидание: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/scheduler/mod.rs
git commit -m "feat(scheduler): add tool_policy field to ScheduledJob"
```

---

## Task 3: Обновить API handlers — принимать tool_policy

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/cron.rs`

- [ ] **Step 1: Написать тест**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn create_cron_request_accepts_tool_policy() {
        let json = serde_json::json!({
            "name": "daily-summary",
            "agent": "main",
            "task": "summarize today",
            "cron": "0 9 * * *",
            "tool_policy": {"allow": ["memory_search"], "deny": []}
        });
        let req: super::CreateCronRequest = serde_json::from_value(json).unwrap();
        assert!(req.tool_policy.is_some());
    }

    #[test]
    fn create_cron_request_without_tool_policy() {
        let json = serde_json::json!({
            "name": "test",
            "agent": "main",
            "task": "do something",
            "cron": "0 * * * *"
        });
        let req: super::CreateCronRequest = serde_json::from_value(json).unwrap();
        assert!(req.tool_policy.is_none());
    }
}
```

- [ ] **Step 2: Найти CreateCronRequest и UpdateCronRequest в cron.rs**

Добавить поле:

```rust
#[derive(Debug, Deserialize)]
pub(crate) struct CreateCronRequest {
    pub name: String,
    pub agent: String,
    pub task: String,
    pub cron: String,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub silent: bool,
    #[serde(default)]
    pub announce_to: Option<serde_json::Value>,
    #[serde(default)]
    pub jitter_secs: i32,
    #[serde(default)]
    pub run_once: bool,
    #[serde(default)]
    pub run_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub tool_policy: Option<serde_json::Value>,
}
```

То же для `UpdateCronRequest` если он существует.

- [ ] **Step 3: Добавить tool_policy в INSERT и UPDATE запросы**

В `api_create_cron` handler найти INSERT запрос. Добавить колонку:

```rust
sqlx::query(
    "INSERT INTO scheduled_jobs (agent_id, name, cron_expr, timezone, task_message, enabled, silent, announce_to, jitter_secs, run_once, run_at, tool_policy) \
     VALUES ($1, $2, $3, $4, $5, true, $6, $7, $8, $9, $10, $11) RETURNING id",
)
.bind(&req.agent)
.bind(&req.name)
.bind(&req.cron)
.bind(req.timezone.as_deref().unwrap_or("UTC"))
.bind(&req.task)
.bind(req.silent)
.bind(&req.announce_to)
.bind(req.jitter_secs)
.bind(req.run_once)
.bind(&req.run_at)
.bind(&req.tool_policy)   // ← новое
```

Аналогично для UPDATE если есть.

- [ ] **Step 4: Валидация tool names**

Добавить валидацию имён инструментов (паттерн `[a-zA-Z0-9_-]`):

```rust
if let Some(ref policy) = req.tool_policy {
    if let Ok(p) = serde_json::from_value::<crate::config::AgentToolPolicy>(policy.clone()) {
        let valid = regex::Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap();
        for name in p.allow.iter().chain(p.deny.iter()) {
            if !valid.is_match(name) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": format!("invalid tool name: {}", name)})),
                ).into_response();
            }
        }
    }
}
```

- [ ] **Step 5: Запустить тест**

```bash
cd crates/hydeclaw-core && cargo test cron -- --nocapture
```
Ожидание: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/cron.rs
git commit -m "feat(api): accept tool_policy in cron job create/update with name validation"
```

---

## Task 4: Передать tool_policy из scheduler в engine

**Files:**
- Modify: `crates/hydeclaw-core/src/scheduler/mod.rs`
- Modify: `crates/hydeclaw-core/src/agent/engine.rs`

- [ ] **Step 1: Найти место в scheduler где создаётся IncomingMessage и вызывается engine**

Строки 535-565 в `scheduler/mod.rs`. Там:
```rust
let msg = hydeclaw_types::IncomingMessage { ... };
match engine.handle_isolated(&msg).await { ... }
```

- [ ] **Step 2: Проверить IncomingMessage — есть ли поле для tool policy override**

Найти struct `IncomingMessage` в `hydeclaw-types/src/lib.rs`. Если поля нет — добавить:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub user_id: String,
    pub text: Option<String>,
    pub attachments: Vec<Attachment>,
    pub agent_id: String,
    pub channel: String,
    pub context: serde_json::Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub formatting_prompt: Option<String>,
    /// Optional tool policy override (used by cron jobs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy_override: Option<serde_json::Value>,
}
```

- [ ] **Step 3: Передать tool_policy при создании сообщения**

В scheduler, при формировании `IncomingMessage`:

```rust
let msg = hydeclaw_types::IncomingMessage {
    user_id: "system".to_string(),
    text: Some(task_message),
    attachments: vec![],
    agent_id: agent_name.to_string(),
    channel: crate::agent::channel_kind::channel::CRON.to_string(),
    context: announce_to.clone().unwrap_or(serde_json::Value::Null),
    timestamp: chrono::Utc::now(),
    formatting_prompt: fmt_prompt,
    tool_policy_override: job.tool_policy.clone(), // ← новое
};
```

- [ ] **Step 4: В engine.handle_isolated применять tool_policy_override**

Найти `handle_isolated` в `engine.rs`. Добавить логику применения override после загрузки agent tool policy:

```rust
// Apply cron job tool policy override if present
if let Some(ref policy_json) = msg.tool_policy_override {
    if let Ok(override_policy) = serde_json::from_value::<crate::config::AgentToolPolicy>(policy_json.clone()) {
        // Merge: override deny is added to agent deny; override allow restricts agent allow
        // If override has allow list — intersect with available tools
        // If override has deny list — union with agent deny
        effective_policy = merge_tool_policies(&agent_policy, &override_policy);
        tracing::debug!(
            agent = %self.agent.name,
            allow = ?override_policy.allow,
            deny = ?override_policy.deny,
            "cron job tool policy override applied"
        );
    }
}
```

Добавить вспомогательную функцию `merge_tool_policies`:

```rust
/// Merge job tool policy override on top of agent tool policy.
/// Override deny: union (both denies apply).
/// Override allow: if non-empty, restricts to intersection with agent allow (or all if agent has no allow).
fn merge_tool_policies(
    agent: &crate::config::AgentToolPolicy,
    override_policy: &crate::config::AgentToolPolicy,
) -> crate::config::AgentToolPolicy {
    let mut merged = agent.clone();
    // Add override denies
    for d in &override_policy.deny {
        if !merged.deny.contains(d) {
            merged.deny.push(d.clone());
        }
    }
    // If override has allow list — restrict: only tools in override allow are permitted
    if !override_policy.allow.is_empty() {
        merged.allow = override_policy.allow.clone();
        merged.deny_all_others = true;
    }
    merged
}
```

- [ ] **Step 5: Проверить компиляцию**

```bash
cd crates/hydeclaw-core && cargo check 2>&1 | grep error | head -10
```

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/scheduler/mod.rs crates/hydeclaw-core/src/agent/engine.rs crates/hydeclaw-types/src/lib.rs
git commit -m "feat(scheduler): pass tool_policy_override from cron job to engine"
```

---

## Task 5: UI — секция Tool Policy в диалоге cron job

**Files:**
- Modify: `ui/src/app/(authenticated)/tasks/page.tsx`

- [ ] **Step 1: Найти форму создания/редактирования cron job в tasks/page.tsx**

Найти `formOpen`, `setForm`, поля формы. Там есть `<Input>` для name, cron, task. Добавить состояние для tool policy:

```typescript
const [toolPolicyAllow, setToolPolicyAllow] = useState("");
const [toolPolicyDeny, setToolPolicyDeny] = useState("");
```

- [ ] **Step 2: Добавить секцию Tool Policy в форму**

После существующих полей формы добавить collapsible секцию:

```tsx
{/* Tool Policy */}
<details className="group">
  <summary className="cursor-pointer text-sm font-medium text-muted-foreground py-1 select-none">
    {t("cron.tool_policy")} <span className="text-xs">(optional)</span>
  </summary>
  <div className="mt-2 space-y-2">
    <div>
      <label className="text-xs text-muted-foreground">{t("cron.tool_allow")}</label>
      <Textarea
        placeholder="memory_search&#10;searxng_search"
        value={toolPolicyAllow}
        onChange={(e) => setToolPolicyAllow(e.target.value)}
        rows={3}
        className="mt-1 font-mono text-xs"
      />
      <p className="text-xs text-muted-foreground mt-1">{t("cron.tool_policy_hint")}</p>
    </div>
    <div>
      <label className="text-xs text-muted-foreground">{t("cron.tool_deny")}</label>
      <Textarea
        placeholder="workspace_write&#10;code_exec"
        value={toolPolicyDeny}
        onChange={(e) => setToolPolicyDeny(e.target.value)}
        rows={3}
        className="mt-1 font-mono text-xs"
      />
    </div>
  </div>
</details>
```

- [ ] **Step 3: Включить tool_policy в saveJob функцию**

Найти `saveJob` (строки 106-120). Добавить формирование `tool_policy`:

```typescript
const allow = toolPolicyAllow.split("\n").map(s => s.trim()).filter(Boolean);
const deny = toolPolicyDeny.split("\n").map(s => s.trim()).filter(Boolean);
const tool_policy = (allow.length > 0 || deny.length > 0)
  ? { allow, deny }
  : undefined;

const payload = {
  name: form.name,
  agent: form.agent,
  task: form.task,
  cron: form.cron,
  // ... остальные поля
  tool_policy,
};
```

- [ ] **Step 4: При открытии редактирования заполнять поля**

В `openEdit` функции добавить заполнение tool policy полей:

```typescript
setToolPolicyAllow((job.tool_policy?.allow ?? []).join("\n"));
setToolPolicyDeny((job.tool_policy?.deny ?? []).join("\n"));
```

- [ ] **Step 5: Добавить i18n ключи**

В русский файл:
```json
"cron.tool_policy": "Политика инструментов",
"cron.tool_allow": "Разрешить (по одному на строку)",
"cron.tool_deny": "Запретить (по одному на строку)",
"cron.tool_policy_hint": "Пусто = используется политика агента"
```

В английский файл:
```json
"cron.tool_policy": "Tool Policy",
"cron.tool_allow": "Allow (one per line)",
"cron.tool_deny": "Deny (one per line)",
"cron.tool_policy_hint": "Empty = use agent policy"
```

- [ ] **Step 6: Проверить сборку UI**

```bash
cd ui && npm run build 2>&1 | tail -10
```
Ожидание: 0 ошибок.

- [ ] **Step 7: Commit**

```bash
git add ui/src/app/(authenticated)/tasks/page.tsx ui/src/i18n/
git commit -m "feat(ui): add Tool Policy section to cron job dialog"
```

---

## Task 6: Финальная проверка

- [ ] **Step 1: Полный cargo test**

```bash
cd d:/GIT/bogdan/hydeclaw && cargo test 2>&1 | tail -20
```

- [ ] **Step 2: UI тесты**

```bash
cd ui && npm test 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "feat: phase 35 complete — per-cron tool allowlist"
```
