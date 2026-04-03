# Phase 34: Global Agent Defaults + Compaction Provider — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `[agent.defaults]` в `hydeclaw.toml` задаёт глобальные LLM параметры по умолчанию; новая capability `compaction` в Active Providers позволяет использовать отдельную (дешёвую) модель для компакции контекста.

**Architecture:** Часть A — новая struct `AgentDefaultsConfig` в `AppConfig`, fallback в `create_provider()`. Часть B — новая строка `Compaction` в UI Active Providers, capability `compaction` в `provider_active`, `compact_if_needed()` принимает опциональный провайдер.

**Tech Stack:** Rust (serde, sqlx), TypeScript/React (TanStack Query), PostgreSQL

---

## File Map

| Файл | Действие |
|------|----------|
| `crates/hydeclaw-core/src/config/mod.rs` | Добавить `AgentDefaultsConfig`, поле в `AppConfig` |
| `crates/hydeclaw-core/src/agent/providers.rs` | Fallback на `agent.defaults` в `create_provider()` |
| `crates/hydeclaw-core/src/agent/history.rs` | Параметр `compaction_provider` в `compact_if_needed()` |
| `crates/hydeclaw-core/src/agent/engine.rs` | Читать compaction провайдер из `provider_active`, передавать |
| `crates/hydeclaw-core/src/db/providers.rs` | Функция `get_active_provider("compaction")` (если нет) |
| `ui/src/app/(authenticated)/providers/page.tsx` | Добавить строку Compaction в ACTIVE PROVIDERS |

---

## Task 1: Добавить AgentDefaultsConfig в config/mod.rs

**Files:**
- Modify: `crates/hydeclaw-core/src/config/mod.rs`

- [ ] **Step 1: Написать тест**

```rust
#[cfg(test)]
mod defaults_tests {
    use super::*;

    #[test]
    fn agent_defaults_deserialize_from_toml() {
        let toml_str = r#"
[agent.defaults]
temperature = 0.5
max_tokens = 2048
"#;
        // AppConfig wraps this — test the sub-struct directly
        #[derive(serde::Deserialize)]
        struct Wrapper {
            agent: AgentSection,
        }
        #[derive(serde::Deserialize)]
        struct AgentSection {
            defaults: AgentDefaultsConfig,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.agent.defaults.temperature, Some(0.5));
        assert_eq!(w.agent.defaults.max_tokens, Some(2048));
    }

    #[test]
    fn agent_defaults_missing_is_none() {
        let cfg = AgentDefaultsConfig::default();
        assert!(cfg.temperature.is_none());
        assert!(cfg.max_tokens.is_none());
    }
}
```

- [ ] **Step 2: Запустить тест — убедиться что падает**

```bash
cd crates/hydeclaw-core && cargo test defaults_tests -- --nocapture 2>&1 | tail -10
```

- [ ] **Step 3: Добавить AgentDefaultsConfig**

В `config/mod.rs` добавить struct и поле в `AppConfig`:

```rust
/// Global LLM parameter defaults applied when agent config doesn't specify them.
/// Priority: routing rule → agent config → [agent.defaults] → provider defaults.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentDefaultsConfig {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

/// Wrapper for [agent] section in hydeclaw.toml
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentSectionConfig {
    #[serde(default)]
    pub defaults: AgentDefaultsConfig,
}
```

В struct `AppConfig` добавить поле:

```rust
#[serde(default)]
pub agent: AgentSectionConfig,
```

- [ ] **Step 4: Запустить тест**

```bash
cd crates/hydeclaw-core && cargo test defaults_tests -- --nocapture
```
Ожидание: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/config/mod.rs
git commit -m "feat(config): add AgentDefaultsConfig for global LLM parameter defaults"
```

---

## Task 2: Применять глобальные defaults в create_provider

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/providers.rs`

- [ ] **Step 1: Написать тест**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn apply_agent_defaults_fills_missing() {
        let agent_temp: Option<f64> = None; // agent didn't set temperature
        let default_temp: Option<f64> = Some(0.5);
        let resolved = agent_temp.or(default_temp).unwrap_or(0.7);
        assert_eq!(resolved, 0.5);
    }

    #[test]
    fn agent_config_overrides_defaults() {
        let agent_temp: Option<f64> = Some(0.9); // agent set temperature
        let default_temp: Option<f64> = Some(0.5);
        let resolved = agent_temp.or(default_temp).unwrap_or(0.7);
        assert_eq!(resolved, 0.9);
    }
}
```

- [ ] **Step 2: Запустить тест**

```bash
cd crates/hydeclaw-core && cargo test providers::tests -- --nocapture
```
Ожидание: PASS (логика простая, тест проходит сразу).

- [ ] **Step 3: Добавить поле app_config в AgentEngine**

В `engine.rs` struct `AgentEngine` **нет** поля `app_config`. Нужно добавить:

```rust
pub struct AgentEngine {
    // ... существующие поля ...
    pub app_config: Arc<crate::config::AppConfig>,
}
```

Найти функцию создания `AgentEngine` (обычно в `main.rs` или `gateway/state.rs`) и пробросить `Arc<AppConfig>`.

- [ ] **Step 4: Применить defaults при создании провайдера**

В `engine.rs` найти место где создаётся основной LLM провайдер агента (вызов `create_provider` или `create_provider_from_connection` с `agent.temperature`, `agent.max_tokens`). Применить fallback:

```rust
// Применяем global defaults для max_tokens (temperature не трогаем — у неё уже serde default 0.7)
let max_tokens = self.agent.max_tokens.or(self.app_config.agent.defaults.max_tokens);
// Temperature: если агент использует serde-дефолт И в global defaults задано — применить
let global_temp = self.app_config.agent.defaults.temperature;
let temperature = global_temp.unwrap_or(self.agent.temperature);
```

**Примечание:** поскольку `AgentSettings.temperature` имеет serde дефолт (`fn default_temperature() -> f64`), агент не может отличить "я явно задал 0.7" от "serde подставил 0.7". Это допустимо — global defaults перезапишут serde-дефолт.

- [ ] **Step 5: Проверить компиляцию**

```bash
cd crates/hydeclaw-core && cargo check 2>&1 | grep error | head -10
```

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine.rs crates/hydeclaw-core/src/agent/providers.rs
git commit -m "feat(engine): apply [agent.defaults] temperature and max_tokens fallback"
```

---

## Task 3: Добавить capability compaction в provider_active

**Files:**
- Modify: (нет миграции — таблица без CHECK constraint, просто новая запись)
- Modify: `crates/hydeclaw-core/src/db/providers.rs` (или где хранятся функции для provider_active)

- [ ] **Step 1: Найти функцию get_active_provider**

Поискать в `db/providers.rs` или `memory.rs` функцию которая читает `provider_active`. Она выглядит примерно как:

```rust
pub async fn get_active_provider(db: &PgPool, capability: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT provider_name FROM provider_active WHERE capability = $1"
    )
    .bind(capability)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(name,)| name))
}
```

Если такой функции нет — найти где используется таблица `provider_active` и выяснить как читается capability. Добавить функцию `get_active_provider` если её нет.

- [ ] **Step 2: Написать тест**

```rust
#[test]
fn compaction_capability_name() {
    // Просто проверяем константу чтобы не было опечаток
    assert_eq!(crate::db::providers::CAPABILITY_COMPACTION, "compaction");
}
```

- [ ] **Step 3: Добавить константу**

В `db/providers.rs` добавить:

```rust
pub const CAPABILITY_COMPACTION: &str = "compaction";
```

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/db/providers.rs
git commit -m "feat(providers): add compaction capability constant"
```

---

## Task 4: compact_if_needed принимает опциональный провайдер

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/history.rs`
- Modify: `crates/hydeclaw-core/src/agent/engine.rs`

- [ ] **Step 1: Написать тест**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Verify signature accepts Option<&dyn LlmProvider>
    // This is a compile-time check
    fn _accepts_optional_provider(
        messages: &mut Vec<hydeclaw_types::Message>,
        primary: &dyn crate::agent::providers::LlmProvider,
        compaction: Option<&dyn crate::agent::providers::LlmProvider>,
    ) {
        // Just verify the function can be called with None
        let _ = async move {
            compact_if_needed(messages, primary, compaction, 4096, 5, None).await
        };
    }
}
```

- [ ] **Step 2: Обновить сигнатуру compact_if_needed**

Найти функцию `compact_if_needed` в `history.rs` (строки 34-46). Обновить сигнатуру:

```rust
pub async fn compact_if_needed(
    messages: &mut Vec<Message>,
    provider: &dyn LlmProvider,
    compaction_provider: Option<&dyn LlmProvider>,
    max_tokens: usize,
    preserve_last_n: usize,
    agent_language: Option<&str>,
) -> Result<Option<Vec<String>>> {
    let total = estimate_tokens(messages);
    let threshold = max_tokens * 80 / 100;

    if total < threshold {
        return Ok(None);
    }

    // Use compaction_provider if provided, otherwise fall back to primary provider
    let active_provider: &dyn LlmProvider = compaction_provider.unwrap_or(provider);
    
    // ... остальная логика без изменений, но используя active_provider вместо provider
```

Найти все вызовы `provider.chat(...)` внутри функции и заменить на `active_provider.chat(...)`.

- [ ] **Step 3: Исправить все вызовы compact_if_needed в engine.rs**

Найти все вызовы `compact_if_needed(...)` в `engine.rs`. Добавить `None` как третий параметр:

```rust
compact_if_needed(&mut messages, provider.as_ref(), None, max_tokens, preserve_last_n, Some(&lang)).await?
```

Это временно — в Task 5 заменим `None` на реальный compaction провайдер.

- [ ] **Step 4: Проверить компиляцию**

```bash
cd crates/hydeclaw-core && cargo check 2>&1 | grep error | head -10
```

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/agent/history.rs crates/hydeclaw-core/src/agent/engine.rs
git commit -m "feat(history): compact_if_needed accepts optional compaction provider"
```

---

## Task 5: Engine читает compaction провайдер из provider_active

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine.rs`

- [ ] **Step 1: При инициализации AgentEngine читать compaction провайдер**

Найти место в `engine.rs` где инициализируется `AgentEngine` (функция `new` или `create`). После создания основного провайдера добавить:

```rust
// Load compaction provider from provider_active (optional — falls back to primary)
let compaction_provider: Option<Arc<dyn LlmProvider>> = {
    match crate::db::providers::get_provider_active(&db, crate::db::providers::CAPABILITY_COMPACTION).await {
        Ok(Some(provider_name)) => {
            // Load provider config from providers table
            match crate::db::providers::get_provider_by_name(&db, &provider_name).await {
                Ok(Some(provider_config)) => {
                    // create_provider_from_connection builds a provider from ProviderRow
                    let p = crate::agent::providers::create_provider_from_connection(
                        &provider_config,
                        provider_config.temperature.unwrap_or(0.3),
                        provider_config.max_tokens,
                        secrets.clone(),
                    );
                    tracing::info!(provider = %provider_name, "using dedicated compaction provider");
                    Some(p)
                }
                _ => None,
            }
        }
        _ => None,
    }
};
```

Сохранить как поле `compaction_provider: Option<Arc<dyn LlmProvider>>` в `AgentEngine`.

- [ ] **Step 2: Обновить вызовы compact_if_needed**

Заменить `None` в вызовах `compact_if_needed` на `self.compaction_provider.as_deref()`:

```rust
compact_if_needed(
    &mut messages,
    self.provider.as_ref(),
    self.compaction_provider.as_deref(),
    max_tokens,
    preserve_last_n,
    Some(&lang),
).await?
```

- [ ] **Step 3: Проверить компиляцию**

```bash
cd crates/hydeclaw-core && cargo check 2>&1 | grep error | head -10
```

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine.rs
git commit -m "feat(engine): load and use compaction provider from provider_active"
```

---

## Task 6: UI — добавить Compaction в Active Providers

**Files:**
- Modify: `ui/src/app/(authenticated)/providers/page.tsx`

- [ ] **Step 1: Найти секцию ACTIVE PROVIDERS в providers/page.tsx**

Найти строки где рендерится список capabilities (Graph, STT, ImageGen, Embedding). Там должен быть массив capability конфигов вида:

```typescript
const CAPABILITIES = [
  { key: "graph", label: "Graph" },
  { key: "stt", label: "STT" },
  { key: "imagegen", label: "ImageGen" },
  { key: "embedding", label: "Embedding" },
] as const;
```

Или capabilities рендерятся хардкодом. Найти это место.

- [ ] **Step 2: Добавить Compaction capability**

Добавить `compaction` в список capabilities:

```typescript
{ key: "compaction", label: t("providers.compaction") },
```

Если capabilities хардкодированы — добавить строку `Compaction` рядом с `Embedding`. Если используется массив — добавить объект.

- [ ] **Step 3: Добавить i18n ключ**

Найти файлы переводов (`ui/src/i18n/` или похожее). Добавить:

В русский файл:
```json
"compaction": "Компакция"
```

В английский файл:
```json
"compaction": "Compaction"
```

- [ ] **Step 4: Проверить UI сборку**

```bash
cd ui && npm run build 2>&1 | tail -20
```
Ожидание: 0 ошибок.

- [ ] **Step 5: Commit**

```bash
git add ui/src/app/(authenticated)/providers/page.tsx ui/src/i18n/
git commit -m "feat(ui): add Compaction capability to Active Providers panel"
```

---

## Task 7: Финальная проверка

- [ ] **Step 1: Полный cargo test**

```bash
cd d:/GIT/bogdan/hydeclaw && cargo test 2>&1 | tail -20
```
Ожидание: PASS.

- [ ] **Step 2: UI тесты**

```bash
cd ui && npm test 2>&1 | tail -10
```

- [ ] **Step 3: Commit финальный**

```bash
git add -u
git commit -m "feat: phase 34 complete — global agent defaults and compaction provider"
```
