# Post-Session Knowledge Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically extract and save knowledge from conversations after each session, so agents remember user facts, decisions, and tool insights across sessions.

**Architecture:** New `knowledge_extractor.rs` module with one public function `extract_and_save()`. Called via `tokio::spawn` from all three engine paths after `lifecycle_guard.done()`. Uses the agent's LLM provider for extraction and `MemoryStore` for dedup + save.

**Tech Stack:** Rust (serde_json, sqlx), LLM provider (existing trait)

**Spec:** `docs/superpowers/specs/2026-04-13-post-session-knowledge-extraction-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `crates/hydeclaw-core/src/agent/knowledge_extractor.rs` | Extraction prompt, JSON parsing, dedup, save to memory |
| Modify | `crates/hydeclaw-core/src/agent/mod.rs` | Register module |
| Modify | `crates/hydeclaw-core/src/agent/engine_execution.rs` | Spawn extraction after handle_with_status |
| Modify | `crates/hydeclaw-core/src/agent/engine_sse.rs` | Spawn extraction after handle_sse |
| Modify | `crates/hydeclaw-core/src/agent/engine.rs` | Spawn extraction after handle_isolated |

---

### Task 1: Create Knowledge Extractor Module

**Files:**
- Create: `crates/hydeclaw-core/src/agent/knowledge_extractor.rs`
- Modify: `crates/hydeclaw-core/src/agent/mod.rs`

- [ ] **Step 1: Create the module file**

```rust
//! Post-session knowledge extraction.
//!
//! After a session completes with ≥ 5 messages, extracts user facts, outcomes,
//! and tool insights via LLM and saves them to long-term memory.

use std::sync::Arc;
use anyhow::Result;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::agent::memory_service::MemoryService;
use crate::agent::providers::LlmProvider;
use hydeclaw_types::{Message, MessageRole};

/// Minimum messages in a session to trigger extraction.
const MIN_MESSAGES: usize = 5;
/// Maximum messages to include in the extraction prompt.
const MAX_CONTEXT_MESSAGES: usize = 20;
/// LLM call timeout.
const EXTRACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Similarity threshold for dedup — skip saving if existing chunk is this similar.
const DEDUP_THRESHOLD: f64 = 0.9;

#[derive(Debug, Deserialize)]
struct ExtractedKnowledge {
    #[serde(default)]
    user_facts: Vec<String>,
    #[serde(default)]
    outcomes: Vec<String>,
    #[serde(default)]
    tool_insights: Vec<String>,
}

/// Extract knowledge from a completed session and save to memory.
/// Runs in background — errors are logged, never propagated.
pub async fn extract_and_save(
    db: PgPool,
    session_id: Uuid,
    agent_name: String,
    provider: Arc<dyn LlmProvider>,
    memory_store: Arc<dyn MemoryService>,
) {
    if !memory_store.is_available() {
        return;
    }

    if let Err(e) = extract_and_save_inner(&db, session_id, &agent_name, &provider, &memory_store).await {
        tracing::warn!(
            session_id = %session_id,
            agent = %agent_name,
            error = %e,
            "knowledge extraction failed"
        );
    }
}

async fn extract_and_save_inner(
    db: &PgPool,
    session_id: Uuid,
    agent_name: &str,
    provider: &Arc<dyn LlmProvider>,
    memory_store: &Arc<dyn MemoryService>,
) -> Result<()> {
    // 1. Load messages
    let rows = crate::db::sessions::load_messages(db, session_id, None).await?;
    if rows.len() < MIN_MESSAGES {
        return Ok(());
    }

    // 2. Build context: last N user+assistant messages (skip tool results to save tokens)
    let relevant: Vec<&crate::db::sessions::MessageRow> = rows.iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .collect();
    let context_msgs: Vec<&crate::db::sessions::MessageRow> = if relevant.len() > MAX_CONTEXT_MESSAGES {
        relevant[relevant.len() - MAX_CONTEXT_MESSAGES..].to_vec()
    } else {
        relevant
    };

    if context_msgs.is_empty() {
        return Ok(());
    }

    // 3. Format conversation for LLM
    let mut conversation = String::new();
    for m in &context_msgs {
        let role_label = match m.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            _ => continue,
        };
        let content = m.content.trim();
        if !content.is_empty() {
            conversation.push_str(&format!("{}: {}\n\n", role_label, content));
        }
    }

    if conversation.len() < 50 {
        return Ok(()); // Too short to extract anything meaningful
    }

    // 4. Call LLM for extraction
    let prompt = format!(
        "You are a knowledge extraction assistant. Analyze the conversation below and extract information worth remembering long-term.\n\n\
         Return a JSON object with three arrays:\n\
         {{\n\
           \"user_facts\": [\"...\"],\n\
           \"outcomes\": [\"...\"],\n\
           \"tool_insights\": [\"...\"]\n\
         }}\n\n\
         Rules:\n\
         - Only extract non-trivial information. Skip greetings, small talk, obvious context.\n\
         - Each item should be a self-contained sentence that makes sense without the conversation.\n\
         - Write in the same language as the conversation.\n\
         - Return empty arrays if nothing worth saving.\n\
         - Maximum 5 items per category.\n\n\
         Conversation:\n{}", conversation
    );

    let messages = vec![
        Message {
            role: MessageRole::User,
            content: prompt,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        },
    ];

    let response = tokio::time::timeout(
        EXTRACTION_TIMEOUT,
        provider.chat(&messages, &[]),
    )
    .await
    .map_err(|_| anyhow::anyhow!("extraction LLM call timed out"))??;

    // 5. Parse JSON from response
    let extracted = parse_extraction(&response.content)?;

    // 6. Dedup and save each fact
    let mut saved = 0u32;
    let source_prefix = format!("auto:session:{}", session_id);

    for fact in &extracted.user_facts {
        if save_if_new(memory_store, fact, &format!("{}:user", source_prefix), agent_name).await {
            saved += 1;
        }
    }
    for outcome in &extracted.outcomes {
        if save_if_new(memory_store, outcome, &format!("{}:outcome", source_prefix), agent_name).await {
            saved += 1;
        }
    }
    for insight in &extracted.tool_insights {
        if save_if_new(memory_store, insight, &format!("{}:tool", source_prefix), agent_name).await {
            saved += 1;
        }
    }

    if saved > 0 {
        tracing::info!(
            session_id = %session_id,
            agent = %agent_name,
            saved,
            user_facts = extracted.user_facts.len(),
            outcomes = extracted.outcomes.len(),
            tool_insights = extracted.tool_insights.len(),
            "knowledge extracted from session"
        );
    }

    Ok(())
}

/// Parse the LLM response into ExtractedKnowledge.
/// Handles markdown fences, <think> blocks, and partial JSON.
fn parse_extraction(content: &str) -> Result<ExtractedKnowledge> {
    // Strip <think>...</think> blocks
    let mut cleaned = content.to_string();
    while let Some(start) = cleaned.find("<think>") {
        if let Some(end) = cleaned.find("</think>") {
            cleaned = format!("{}{}", &cleaned[..start], &cleaned[end + 8..]);
        } else {
            cleaned = cleaned[..start].to_string();
            break;
        }
    }

    // Strip markdown fences
    let cleaned = cleaned
        .replace("```json", "")
        .replace("```", "")
        .trim()
        .to_string();

    // Find JSON object in the text
    if let Some(start) = cleaned.find('{') {
        if let Some(end) = cleaned.rfind('}') {
            let json_str = &cleaned[start..=end];
            return Ok(serde_json::from_str(json_str)?);
        }
    }

    anyhow::bail!("no JSON object found in extraction response")
}

/// Save a fact to memory if it's not already known (similarity < threshold).
async fn save_if_new(
    memory_store: &Arc<dyn MemoryService>,
    text: &str,
    source: &str,
    _agent_name: &str,
) -> bool {
    let text = text.trim();
    if text.is_empty() || text.len() < 10 {
        return false;
    }

    // Check for duplicates via search
    match memory_store.search(text, 1, &[], None, None).await {
        Ok((results, _)) => {
            if let Some(top) = results.first() {
                if top.similarity >= DEDUP_THRESHOLD {
                    return false; // Already known
                }
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, "dedup search failed, saving anyway");
        }
    }

    // Save as new memory chunk
    match memory_store.index(text, Some(source), false, None, None).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, "failed to save extracted knowledge");
            false
        }
    }
}
```

- [ ] **Step 2: Register module in mod.rs**

Add to `crates/hydeclaw-core/src/agent/mod.rs`:

```rust
pub mod knowledge_extractor;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check`

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/knowledge_extractor.rs crates/hydeclaw-core/src/agent/mod.rs
git commit -m "feat: add knowledge_extractor module for post-session knowledge extraction"
```

---

### Task 2: Integrate into engine_execution.rs (handle_with_status)

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine_execution.rs`

- [ ] **Step 1: Add extraction spawn after lifecycle_guard.done()**

Find `lifecycle_guard.done().await;` in `handle_with_status` (around line 468). After it, add:

```rust
        // Post-session knowledge extraction (background, non-blocking)
        if messages.len() >= 5 {
            let db = self.db.clone();
            let provider = self.provider.clone();
            let memory = self.memory_store.clone();
            let agent = self.agent.name.clone();
            let sid = session_id;
            tokio::spawn(async move {
                crate::agent::knowledge_extractor::extract_and_save(
                    db, sid, agent, provider, memory,
                ).await;
            });
        }
```

Note: there are TWO `lifecycle_guard.done()` calls in this file — one in `handle_with_status` (~line 468) and one in `handle_streaming` (~line 544). Add the spawn after BOTH.

But for `handle_streaming` the `messages` variable may not be available after the streaming call. Use a message count tracked earlier. Check the local variables — if `messages` is in scope, use `messages.len()`. If not, skip this path (streaming is for channel adapters, less critical).

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine_execution.rs
git commit -m "feat: spawn knowledge extraction after handle_with_status"
```

---

### Task 3: Integrate into engine_sse.rs (handle_sse)

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine_sse.rs`

- [ ] **Step 1: Add extraction spawn after lifecycle_guard.done()**

Find `lifecycle_guard.done().await;` in `handle_sse` (around line 610). After it, before the "Clear processing session context" comment, add:

```rust
        // Post-session knowledge extraction (background, non-blocking)
        if messages.len() >= 5 {
            let db = self.db.clone();
            let provider = self.provider.clone();
            let memory = self.memory_store.clone();
            let agent = self.agent.name.clone();
            let sid = session_id;
            tokio::spawn(async move {
                crate::agent::knowledge_extractor::extract_and_save(
                    db, sid, agent, provider, memory,
                ).await;
            });
        }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine_sse.rs
git commit -m "feat: spawn knowledge extraction after handle_sse"
```

---

### Task 4: Integrate into engine.rs (handle_isolated)

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine.rs`

- [ ] **Step 1: Add extraction spawn after final save in handle_isolated**

Find the end of `handle_isolated` method. After `sm.save_message_ex(...)` and before `self.hooks().fire(&super::hooks::HookEvent::AfterResponse)`, add:

```rust
        // Post-session knowledge extraction (background, non-blocking)
        if messages.len() >= 5 {
            let db = self.db.clone();
            let provider = self.provider.clone();
            let memory = self.memory_store.clone();
            let agent_name = self.agent.name.clone();
            tokio::spawn(async move {
                crate::agent::knowledge_extractor::extract_and_save(
                    db, session_id, agent_name, provider, memory,
                ).await;
            });
        }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --all-targets`

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine.rs
git commit -m "feat: spawn knowledge extraction after handle_isolated"
```

---

### Task 5: Final Verification + Deploy

- [ ] **Step 1: Full build**

Run: `cargo check --all-targets`
Expected: 0 errors

- [ ] **Step 2: Run tests**

Run: `cargo test`
Expected: All pass

- [ ] **Step 3: Deploy to Pi**

Build ARM64, deploy binary, restart service, verify health.

- [ ] **Step 4: Test manually**

1. Send a message with substance (e.g. "Обсуди мой портфель на BCS")
2. Wait for response to complete
3. Check logs for `knowledge extracted from session`
4. Check `/api/memory` for new chunks with `source` starting with `auto:session:`

- [ ] **Step 5: Commit any fixes**
