# Shared Memory (Scope-Based) — Design Spec

## Problem

Each agent sees only its own memory chunks (`agent_id` scope). In multi-agent sessions, Alma and Hyde start from zero even when Arty already learned user facts and outcomes. Knowledge doesn't flow between agents.

## Solution

Add `scope` column to `memory_chunks`: `"private"` (default, agent-only) or `"shared"` (visible to all agents). Search automatically includes shared chunks alongside private ones.

## Data Model

```sql
ALTER TABLE memory_chunks ADD COLUMN scope TEXT NOT NULL DEFAULT 'private';
```

Existing chunks remain `private`. No data migration needed.

## Search Query Change

**Before:**
```sql
WHERE agent_id = $agent AND archived = false
```

**After:**
```sql
WHERE (agent_id = $agent OR scope = 'shared') AND archived = false
```

Applied to both semantic and FTS search paths in `memory.rs`.

## Scope Assignment Rules

| Source | Scope | Rationale |
|--------|-------|-----------|
| `memory_write` tool (agent manual) | `private` | Agent-specific notes |
| `knowledge_extractor` → `user_facts` | `shared` | User info relevant to all agents |
| `knowledge_extractor` → `outcomes` | `shared` | Decisions/conclusions relevant to all |
| `knowledge_extractor` → `tool_insights` | `private` | Tool-specific, agent-specific |
| Workspace file indexing | `private` | Per-agent workspace |

## Integration Points

| File | Change |
|------|--------|
| Create | `migrations/019_memory_scope.sql` |
| Modify | `crates/hydeclaw-core/src/memory.rs` | Add `scope` param to `index()`, update search WHERE |
| Modify | `crates/hydeclaw-core/src/agent/memory_service.rs` | Add `scope` to `index()` + `index_batch()` trait methods |
| Modify | `crates/hydeclaw-core/src/agent/engine_memory.rs` | Pass `"private"` scope for manual memory_write |
| Modify | `crates/hydeclaw-core/src/agent/knowledge_extractor.rs` | Pass `"shared"` for user_facts/outcomes, `"private"` for tool_insights |
| Modify | `crates/hydeclaw-core/src/db/memory_queries.rs` | Update queries if any use agent_id filter |

## Pinned Memory (L0)

`load_pinned()` currently loads `WHERE agent_id = $agent AND pinned = true`. Update to:
```sql
WHERE (agent_id = $agent OR scope = 'shared') AND pinned = true AND archived = false
```

Shared pinned facts appear in all agents' "Known Facts" section.

## What This Does NOT Do

- No UI for managing scope (chunks show as regular memory)
- No per-agent access control lists
- No cross-agent write permissions (agents can't write to another agent's private memory)
- No scope parameter in `memory_write` tool (always private)
