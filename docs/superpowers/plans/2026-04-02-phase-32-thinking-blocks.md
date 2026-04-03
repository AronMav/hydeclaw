# Phase 32: Anthropic Thinking Blocks — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Thinking blocks из Anthropic extended thinking API сохраняются в БД, восстанавливаются при replay и корректно вставляются обратно в контекст при следующих ходах.

**Architecture:** Новый тип `ThinkingBlock` добавляется в `hydeclaw-types`. `LlmResponse` и `Message` получают поле `thinking_blocks`. Миграция добавляет колонку `thinking_blocks JSONB` в `messages`. Anthropic-специфичный код в `providers_anthropic.rs` парсит и сериализует thinking blocks; остальные провайдеры игнорируют поле.

**Tech Stack:** Rust, sqlx, serde_json, PostgreSQL JSONB

---

## File Map

| Файл | Действие |
|------|----------|
| `crates/hydeclaw-types/src/lib.rs` | Добавить `ThinkingBlock`, поле в `LlmResponse` и `Message` |
| `crates/hydeclaw-core/src/agent/providers_anthropic.rs` | Парсить thinking blocks, вставлять в контекст |
| `crates/hydeclaw-core/src/db/sessions.rs` | Поле в `MessageRow`, параметр в `save_message_ex`, SQL |
| `crates/hydeclaw-core/src/agent/engine.rs` | Передавать thinking_blocks при save и build_context |
| `migrations/003_thinking_blocks.sql` | `ALTER TABLE messages ADD COLUMN thinking_blocks JSONB` |

---

## Task 1: Добавить ThinkingBlock в hydeclaw-types

**Files:**
- Modify: `crates/hydeclaw-types/src/lib.rs`

- [ ] **Step 1: Написать тест**

```rust
// В конец файла crates/hydeclaw-types/src/lib.rs добавить:
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_block_roundtrip() {
        let tb = ThinkingBlock {
            thinking: "some reasoning".to_string(),
            signature: "sig_abc123".to_string(),
        };
        let json = serde_json::to_string(&tb).unwrap();
        let back: ThinkingBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(back.thinking, "some reasoning");
        assert_eq!(back.signature, "sig_abc123");
    }

    #[test]
    fn llm_response_thinking_blocks_default_empty() {
        let r: LlmResponse = serde_json::from_str(r#"{"content":"hi","tool_calls":[]}"#).unwrap();
        assert!(r.thinking_blocks.is_empty());
    }
}
```

- [ ] **Step 2: Запустить тест, убедиться что не компилируется**

```bash
cd crates/hydeclaw-types && cargo test 2>&1 | head -20
```
Ожидание: ошибка компиляции `ThinkingBlock` not found.

- [ ] **Step 3: Добавить тип ThinkingBlock и обновить LlmResponse и Message**

Найти struct `ThinkingBlock` — его нет. Найти struct `LlmResponse` (строки 110-131) и struct `Message` в `hydeclaw-types/src/lib.rs`. Добавить:

```rust
/// A thinking block from Anthropic extended thinking API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingBlock {
    pub thinking: String,
    pub signature: String,
}
```

В `LlmResponse` добавить поле после `tool_calls`:
```rust
/// Thinking blocks from Anthropic extended thinking (empty for other providers).
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub thinking_blocks: Vec<ThinkingBlock>,
```

В `Message` (найти struct `Message` в том же файле) добавить поле:
```rust
/// Thinking blocks (Anthropic only). Stored separately from content.
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub thinking_blocks: Vec<ThinkingBlock>,
```

- [ ] **Step 4: Запустить тест**

```bash
cd crates/hydeclaw-types && cargo test
```
Ожидание: PASS.

- [ ] **Step 5: Исправить все конструкторы Message и LlmResponse**

Поле `thinking_blocks` в `Message` и `LlmResponse` помечено `#[serde(default)]`, но **явные Rust-конструкторы** (`Message { role, content, tool_calls, tool_call_id }`) сломаются — нужно добавить `thinking_blocks: vec![]` в каждый.

Найти и исправить **22 места Message** (добавить `thinking_blocks: vec![]`):
```bash
grep -rn "Message {" crates/hydeclaw-core/src/ --include="*.rs" | grep -v "//\|test"
```
Ключевые файлы: `engine.rs` (~20 мест), `cli_backend.rs` (~2 места).

Найти и исправить **8 мест LlmResponse** (добавить `thinking_blocks: vec![]`):
```bash
grep -rn "LlmResponse {" crates/ --include="*.rs"
```
Ключевые файлы: `providers_anthropic.rs:236,388`, `providers_claude_cli.rs:54`, `providers_google.rs:279,393`, `providers_openai.rs:295,496`, `engine_subagent.rs:941`.

- [ ] **Step 6: Проверить компиляцию**

```bash
cd crates/hydeclaw-core && cargo check 2>&1 | grep error | head -20
```
Ожидание: 0 ошибок.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-types/src/lib.rs
git commit -m "feat(types): add ThinkingBlock type and fields to LlmResponse and Message"
```

---

## Task 2: Миграция — добавить колонку thinking_blocks

**Files:**
- Create: `migrations/003_thinking_blocks.sql`

- [ ] **Step 1: Создать файл миграции**

```sql
-- migrations/003_thinking_blocks.sql
ALTER TABLE messages ADD COLUMN IF NOT EXISTS thinking_blocks JSONB DEFAULT NULL;
```

- [ ] **Step 2: Проверить что sqlx применит миграцию при старте**

Миграции применяются автоматически при старте через `sqlx::migrate!`. Проверить что имя файла следует после `002_...` (должно быть `003_...`). Других изменений не требуется.

- [ ] **Step 3: Commit**

```bash
git add migrations/003_thinking_blocks.sql
git commit -m "feat(db): add thinking_blocks JSONB column to messages"
```

---

## Task 3: Парсить thinking blocks в providers_anthropic.rs

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/providers_anthropic.rs`

- [ ] **Step 1: Написать тест**

Добавить в конец `providers_anthropic.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_thinking_block() {
        let json = serde_json::json!({
            "content": [
                {"type": "thinking", "thinking": "let me think", "signature": "sig_xyz"},
                {"type": "text", "text": "The answer is 42."}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let resp: AnthropicResponse = serde_json::from_value(json).unwrap();
        let parsed = parse_anthropic_response(resp, "claude-opus-4-6");
        assert_eq!(parsed.content, "The answer is 42.");
        assert_eq!(parsed.thinking_blocks.len(), 1);
        assert_eq!(parsed.thinking_blocks[0].thinking, "let me think");
        assert_eq!(parsed.thinking_blocks[0].signature, "sig_xyz");
    }

    #[test]
    fn thinking_block_other_not_thinking_still_dropped() {
        let json = serde_json::json!({
            "content": [
                {"type": "unknown_future_type", "data": "x"},
                {"type": "text", "text": "hi"}
            ],
            "usage": null
        });
        let resp: AnthropicResponse = serde_json::from_value(json).unwrap();
        let parsed = parse_anthropic_response(resp, "claude-opus-4-6");
        assert_eq!(parsed.content, "hi");
        assert!(parsed.thinking_blocks.is_empty());
    }
}
```

- [ ] **Step 2: Запустить тест, убедиться что падает**

```bash
cd crates/hydeclaw-core && cargo test providers_anthropic::tests -- --nocapture 2>&1 | tail -20
```
Ожидание: FAIL — `Thinking` variant не существует в enum.

- [ ] **Step 3: Добавить Thinking вариант в AnthropicContentBlock**

Найти enum `AnthropicContentBlock` (строки 178-191). Заменить:

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(super) enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(other)]
    Other,
}
```

- [ ] **Step 4: Обновить parse_anthropic_response чтобы собирать thinking blocks**

Найти функцию `parse_anthropic_response` (строки 203-246). Добавить сбор thinking blocks:

```rust
pub(super) fn parse_anthropic_response(api_resp: AnthropicResponse, model: &str) -> LlmResponse {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut thinking_blocks = Vec::new();

    for block in api_resp.content {
        match block {
            AnthropicContentBlock::Text { text } => {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(&text);
            }
            AnthropicContentBlock::Thinking { thinking, signature } => {
                thinking_blocks.push(hydeclaw_types::ThinkingBlock { thinking, signature });
            }
            AnthropicContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(hydeclaw_types::ToolCall {
                    id,
                    name,
                    arguments: input,
                });
            }
            AnthropicContentBlock::Other => {}
        }
    }

    let usage = api_resp.usage.map(|u| {
        if let Some(cache_read) = u.cache_read_input_tokens {
            tracing::info!(cache_read, cache_create = u.cache_creation_input_tokens, "anthropic cache hit");
        }
        hydeclaw_types::TokenUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
        }
    });

    LlmResponse {
        content,
        tool_calls,
        thinking_blocks,
        usage,
        model: Some(model.to_string()),
        provider: Some("anthropic".to_string()),
        fallback_notice: None,
        tools_used: vec![],
        iterations: 0,
    }
}
```

- [ ] **Step 5: Запустить тест**

```bash
cd crates/hydeclaw-core && cargo test providers_anthropic::tests -- --nocapture
```
Ожидание: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/agent/providers_anthropic.rs
git commit -m "feat(anthropic): parse thinking blocks from API response"
```

---

## Task 4: Сохранять и загружать thinking_blocks в БД

**Files:**
- Modify: `crates/hydeclaw-core/src/db/sessions.rs`

- [ ] **Step 1: Написать тест**

В конец `db/sessions.rs`:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn message_row_thinking_blocks_default_none() {
        // MessageRow must have thinking_blocks field with default None
        // This is a compile-time check — if MessageRow doesn't have the field, this won't compile
        let _ = |row: super::MessageRow| {
            let _: Option<serde_json::Value> = row.thinking_blocks;
        };
    }
}
```

- [ ] **Step 2: Добавить поле в MessageRow**

Найти struct `MessageRow` (строки 247-260). Добавить поле:

```rust
#[derive(Debug, FromRow)]
#[allow(dead_code)]
pub struct MessageRow {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub tool_calls: Option<serde_json::Value>,
    pub tool_call_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub agent_id: Option<String>,
    pub feedback: Option<i16>,
    pub edited_at: Option<DateTime<Utc>>,
    pub status: String,
    pub thinking_blocks: Option<serde_json::Value>,
}
```

- [ ] **Step 3: Обновить save_message_ex**

Найти `save_message_ex` в `db/sessions.rs`. Добавить параметр `thinking_blocks: Option<&serde_json::Value>` и обновить SQL INSERT:

```rust
pub async fn save_message_ex(
    db: &PgPool,
    session_id: Uuid,
    role: &str,
    content: &str,
    tool_calls: Option<&serde_json::Value>,
    tool_call_id: Option<&str>,
    agent_id: Option<&str>,
    thinking_blocks: Option<&serde_json::Value>,
) -> Result<Uuid> {
    let id = sqlx::query_scalar(
        "INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id, agent_id, thinking_blocks) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(session_id)
    .bind(role)
    .bind(content)
    .bind(tool_calls)
    .bind(tool_call_id)
    .bind(agent_id)
    .bind(thinking_blocks)
    .fetch_one(db)
    .await?;
    Ok(id)
}
```

- [ ] **Step 4: Обновить save_message — добавить `None` для thinking_blocks**

```rust
pub async fn save_message(
    db: &PgPool,
    session_id: Uuid,
    role: &str,
    content: &str,
    tool_calls: Option<&serde_json::Value>,
    tool_call_id: Option<&str>,
) -> Result<Uuid> {
    save_message_ex(db, session_id, role, content, tool_calls, tool_call_id, None, None).await
}
```

- [ ] **Step 5: Обновить load_messages — включить колонку в SELECT**

Найти `load_messages` (строки 213-245). В обоих SQL запросах добавить `thinking_blocks` в SELECT:

```rust
"SELECT id, role, content, tool_calls, tool_call_id, created_at, agent_id, feedback, edited_at, status, thinking_blocks \
 FROM messages WHERE session_id = $1 ORDER BY created_at ASC"
```

То же для варианта с LIMIT.

- [ ] **Step 6: Обновить row_to_message в engine.rs**

Найти `fn row_to_message` (строки 363-379 в `engine.rs`). Добавить thinking_blocks:

```rust
fn row_to_message(row: &crate::db::sessions::MessageRow) -> Message {
    let tool_calls = row.tool_calls.as_ref().and_then(|tc| {
        serde_json::from_value::<Vec<hydeclaw_types::ToolCall>>(tc.clone()).ok()
    });
    let thinking_blocks = row.thinking_blocks.as_ref().and_then(|tb| {
        serde_json::from_value::<Vec<hydeclaw_types::ThinkingBlock>>(tb.clone()).ok()
    }).unwrap_or_default();
    Message {
        role: match row.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => MessageRole::User,
        },
        content: row.content.clone(),
        tool_calls,
        tool_call_id: row.tool_call_id.clone(),
        thinking_blocks,
    }
}
```

- [ ] **Step 7: Починить все вызовы save_message_ex в engine.rs**

Найти все вызовы `save_message_ex` в `engine.rs`. Каждый получает лишний параметр — добавить `None` как последний аргумент (для вызовов где thinking_blocks не нужны), или передать реальные thinking blocks там где сохраняется assistant message с thinking.

Для сохранения assistant message с LLM response — найти место где engine сохраняет assistant message (ищи `save_message` или `save_message_ex` вызовы с `role = "assistant"` в engine.rs). Там передать:

```rust
let thinking_json = if response.thinking_blocks.is_empty() {
    None
} else {
    serde_json::to_value(&response.thinking_blocks).ok()
};
// ... при вызове save_message_ex:
save_message_ex(&self.db, session_id, "assistant", &response.content, tc_json.as_ref(), None, Some(&self.agent.name), thinking_json.as_ref()).await?
```

- [ ] **Step 8: Проверить компиляцию**

```bash
cd crates/hydeclaw-core && cargo check 2>&1 | head -40
```
Ожидание: 0 ошибок.

- [ ] **Step 9: Commit**

```bash
git add crates/hydeclaw-core/src/db/sessions.rs crates/hydeclaw-core/src/agent/engine.rs
git commit -m "feat(db): save and load thinking_blocks in messages table"
```

---

## Task 5: Вставлять thinking blocks в Anthropic контекст

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/providers_anthropic.rs`

- [ ] **Step 1: Написать тест**

```rust
#[test]
fn build_assistant_message_with_thinking_blocks() {
    let msg = Message {
        role: MessageRole::Assistant,
        content: "The answer is 42.".to_string(),
        tool_calls: None,
        tool_call_id: None,
        thinking_blocks: vec![
            ThinkingBlock {
                thinking: "I need to reason".to_string(),
                signature: "sig_abc".to_string(),
            }
        ],
    };
    // Вызвать приватную функцию build_request_body или напрямую проверить JSON
    // Проверить что thinking block идёт ПЕРЕД text
    let body = build_anthropic_messages(&[msg]);
    let content = &body[0]["content"];
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[1]["type"], "text");
}
```

Если `build_anthropic_messages` не является отдельной функцией — извлечь логику в неё для тестируемости. Иначе написать интеграционный тест через весь провайдер.

- [ ] **Step 2: Найти место в providers_anthropic.rs где строится assistant message**

Это строки 89-110 (в функции `build_request_body` или похожей). Обновить обработку `MessageRole::Assistant`:

```rust
MessageRole::Assistant => {
    let has_tools = msg.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty());
    let has_thinking = !msg.thinking_blocks.is_empty();

    if has_tools || has_thinking {
        let mut content: Vec<serde_json::Value> = Vec::new();
        // Thinking blocks MUST come before text and tool_use (Anthropic API requirement)
        for tb in &msg.thinking_blocks {
            content.push(serde_json::json!({
                "type": "thinking",
                "thinking": tb.thinking,
                "signature": tb.signature,
            }));
        }
        if !msg.content.is_empty() {
            content.push(serde_json::json!({"type": "text", "text": msg.content}));
        }
        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.name,
                    "input": tc.arguments,
                }));
            }
        }
        serde_json::json!({"role": "assistant", "content": content})
    } else {
        serde_json::json!({"role": "assistant", "content": msg.content})
    }
}
```

- [ ] **Step 3: Запустить все тесты**

```bash
cd crates/hydeclaw-core && cargo test -- --nocapture 2>&1 | tail -20
```
Ожидание: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/providers_anthropic.rs
git commit -m "feat(anthropic): insert thinking blocks into context for multi-turn sessions"
```

---

## Task 6: Финальная проверка

- [ ] **Step 1: Полный cargo test**

```bash
cd d:/GIT/bogdan/hydeclaw && cargo test 2>&1 | tail -30
```
Ожидание: все тесты PASS.

- [ ] **Step 2: Проверить что миграция применится**

```bash
cd d:/GIT/bogdan/hydeclaw && cargo check --all-targets 2>&1 | grep -i error | head -10
```

- [ ] **Step 3: Обновить .planning/roadmap.md**

В roadmap добавить Phase 32 как In Progress / выполненную по итогу. Добавить в раздел v5.2:
```
- [ ] **Phase 32: Anthropic Thinking Blocks** - Parse, store, and replay thinking blocks for multi-turn extended thinking sessions
```

- [ ] **Step 4: Commit**

```bash
git add .planning/ROADMAP.md
git commit -m "docs(roadmap): add v5.2 milestone phases 32-36"
```
