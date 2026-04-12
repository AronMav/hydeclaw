# Post-Session Knowledge Extraction — Design Spec

## Problem

Agents respond to users but don't retain insights from conversations. After a session ends, all context is lost unless the agent explicitly called `memory_write` (rare). Users expect agents to remember facts, decisions, and preferences from prior interactions.

## Solution

After each session with ≥ 5 messages, a background task extracts knowledge via LLM and saves it to long-term memory. No user-facing latency — extraction runs in `tokio::spawn` after the session lifecycle completes.

## Knowledge Categories

| Category | Example | Memory `source` |
|----------|---------|-----------------|
| `user_facts` | "User works in IT", "Portfolio on BCS", "Prefers Russian" | `auto:session:{id}:user` |
| `outcomes` | "Recommended reducing oil & gas to 25%", "Chose GraphQL over REST" | `auto:session:{id}:outcome` |
| `tool_insights` | "bcs_portfolio returned data in 2s", "User approved Alma's analysis" | `auto:session:{id}:tool` |

## Extraction Flow

```
Session ends (≥ 5 messages)
  ↓
tokio::spawn (background, non-blocking)
  ↓
Load last 20 user+assistant messages from DB
  ↓
LLM call with extraction prompt (60s timeout)
  ↓
Parse JSON response
  ↓
For each fact:
  ├─ Semantic search (similarity > 0.9) → update existing chunk
  └─ No match → index as new chunk (pinned=false)
```

## LLM Prompt

```
You are a knowledge extraction assistant. Analyze the conversation below and extract information worth remembering long-term.

Return a JSON object with three arrays:
{
  "user_facts": ["..."],   // Facts about the user: preferences, context, identity, goals
  "outcomes": ["..."],     // Decisions made, conclusions reached, recommendations given
  "tool_insights": ["..."] // What tools were used, what worked/failed, user reactions
}

Rules:
- Only extract non-trivial information. Skip greetings, small talk, obvious context.
- Each item should be a self-contained sentence that makes sense without the conversation.
- Write in the same language as the conversation.
- Return empty arrays if nothing worth saving.
- Maximum 5 items per category.

Conversation:
{messages}
```

## Deduplication

Before saving each extracted fact:
1. Call `memory_store.embed(fact_text)` to get embedding vector
2. Call `memory_store.search(fact_text, limit=1)` with the agent's scope
3. If top result has similarity > 0.9 — skip (already known)
4. Otherwise — `memory_store.index(fact_text, source, pinned=false)`

This prevents duplicate facts from accumulating across sessions.

## Integration Points

| File | Change |
|------|--------|
| Create: `crates/hydeclaw-core/src/agent/knowledge_extractor.rs` | Extraction logic: prompt, parse, dedup, save |
| Modify: `crates/hydeclaw-core/src/agent/mod.rs` | Register module |
| Modify: `crates/hydeclaw-core/src/agent/engine_execution.rs` | Add `tokio::spawn` after `lifecycle_guard.done()` |
| Modify: `crates/hydeclaw-core/src/agent/engine_sse.rs` | Same spawn point |
| Modify: `crates/hydeclaw-core/src/agent/engine.rs` | Same spawn point (handle_isolated) |

## Constraints

- **Timeout:** 60 seconds for LLM extraction call
- **Message limit:** Last 20 messages (user + assistant only, skip tool results to save tokens)
- **Session threshold:** ≥ 5 messages total
- **Skip conditions:** memory store unavailable, agent has `memory.enabled = false`
- **No blocking:** runs in background, errors are logged but don't affect user
- **Provider:** uses the agent's primary LLM provider (same model that handled the conversation)

## What This Does NOT Do

- No wiki pages or structured assertions (that's B3)
- No shared memory between agents (that's B2)
- No multi-level summarization (that's B4)
- No UI for reviewing extracted knowledge (facts appear in existing memory page)
