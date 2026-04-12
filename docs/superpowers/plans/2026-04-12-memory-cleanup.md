# Memory System Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove ~3000 lines of dead memory infrastructure (knowledge graph, compression worker, category/topic taxonomy, session documents, MemoryPalace UI) that doesn't contribute to agent response quality.

**Architecture:** Five subsystems removed in order: graph → compression → session docs → taxonomy endpoints → UI. One migration drops 5 tables. MemoryService trait simplified. Core search pipeline (hybrid search + pinned memory) untouched.

**Tech Stack:** Rust (sqlx), TypeScript (Next.js), PostgreSQL

---

## File Structure

| Action | File | What changes |
|--------|------|-------------|
| Delete | `crates/hydeclaw-core/src/memory_graph.rs` | Entire graph implementation |
| Delete | `crates/hydeclaw-core/src/graph_worker.rs` | Graph extraction background worker |
| Delete | `crates/hydeclaw-core/src/compression_worker.rs` | Compression background worker |
| Delete | `ui/src/components/chat/MemoryPalace.tsx` | Graph visualization component |
| Delete | `ui/src/app/(authenticated)/chat/memory/page.tsx` | Graph page route |
| Create | `migrations/018_drop_graph_and_session_docs.sql` | Drop 5 tables |
| Modify | `crates/hydeclaw-core/src/main.rs` | Remove mod declarations + worker spawns |
| Modify | `crates/hydeclaw-core/src/agent/memory_service.rs` | Remove 7 trait methods + impls |
| Modify | `crates/hydeclaw-core/src/agent/engine_memory.rs` | Remove graph_search, session_docs search, compress tools |
| Modify | `crates/hydeclaw-core/src/memory.rs` | Remove graph/compression/session_doc methods from MemoryStore |
| Modify | `crates/hydeclaw-core/src/gateway/handlers/memory.rs` | Remove graph, extraction-queue, categories, topics endpoints |
| Modify | `crates/hydeclaw-core/src/agent/engine_tool_defs.rs` | Remove graph_search, memory_compress tool definitions |
| Modify | `ui/package.json` | Remove `react-force-graph-2d` dependency |

---

### Task 1: Migration — Drop Tables

**Files:**
- Create: `migrations/018_drop_graph_and_session_docs.sql`

- [ ] **Step 1: Write migration**

```sql
-- 018_drop_graph_and_session_docs.sql
-- Remove knowledge graph tables, extraction queue, and session documents.
-- These subsystems don't contribute to agent context building.
DROP TABLE IF EXISTS graph_episodes CASCADE;
DROP TABLE IF EXISTS graph_edges CASCADE;
DROP TABLE IF EXISTS graph_entities CASCADE;
DROP TABLE IF EXISTS graph_extraction_queue CASCADE;
DROP TABLE IF EXISTS session_documents CASCADE;
```

- [ ] **Step 2: Commit**

```bash
git add migrations/018_drop_graph_and_session_docs.sql
git commit -m "feat(db): drop graph tables, extraction queue, session_documents (migration 018)"
```

---

### Task 2: Remove Graph + Compression from main.rs

**Files:**
- Delete: `crates/hydeclaw-core/src/memory_graph.rs`
- Delete: `crates/hydeclaw-core/src/graph_worker.rs`
- Delete: `crates/hydeclaw-core/src/compression_worker.rs`
- Modify: `crates/hydeclaw-core/src/main.rs`

- [ ] **Step 1: Delete the three source files**

```bash
rm crates/hydeclaw-core/src/memory_graph.rs
rm crates/hydeclaw-core/src/graph_worker.rs
rm crates/hydeclaw-core/src/compression_worker.rs
```

- [ ] **Step 2: Remove mod declarations from main.rs**

In `crates/hydeclaw-core/src/main.rs`, delete these three lines (near top):

```rust
mod memory_graph;
mod graph_worker;
mod compression_worker;
```

- [ ] **Step 3: Remove worker spawns from main.rs**

Search for `graph_worker::spawn_worker` and `compression_worker::spawn_worker` in main.rs. Delete the blocks that spawn these workers (including any associated cancel tokens, config reads, and join handles).

Also remove any `graph_cancel` token declaration and `ChatActiveGuard` references if they were only used by graph_worker.

- [ ] **Step 4: Remove any remaining references**

Search main.rs for `memory_graph`, `graph_worker`, `compression_worker`, `graph_cancel`, `compression`. Remove all dead references. The compiler will guide you — run `cargo check` after each removal.

- [ ] **Step 5: Verify compilation**

Run: `cargo check`
Expected: May have errors in other files (memory_service, engine_memory) — that's OK, we fix those next.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: remove knowledge graph, compression worker, and graph worker"
```

---

### Task 3: Simplify MemoryService Trait

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/memory_service.rs`

- [ ] **Step 1: Remove these methods from the `MemoryService` trait**

Delete from the trait definition:
- `search_session_documents()`
- `fetch_chunks_for_graph()`
- `find_graph_related()`
- `fetch_compressible_groups()`
- `spawn_graph_extraction()`
- `compress_memory_group()`

Keep `wipe_agent_memory()` — it's useful for agent reset.

- [ ] **Step 2: Remove corresponding impl blocks**

Delete the implementations of those same methods from both:
- `impl MemoryService for MemoryStore { ... }` (the real implementation)
- `impl MemoryService for MockMemoryService { ... }` (the mock)

- [ ] **Step 3: Verify compilation**

Run: `cargo check`
Expected: Errors in `engine_memory.rs` and `memory.rs` — fixed in next tasks.

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/memory_service.rs
git commit -m "refactor: remove graph, compression, session_docs from MemoryService trait"
```

---

### Task 4: Clean engine_memory.rs — Remove Dead Tools

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine_memory.rs`

- [ ] **Step 1: Remove `handle_graph_search` method**

Find and delete the entire `handle_graph_search` method (calls `self.memory_store.find_graph_related`).

- [ ] **Step 2: Remove `handle_memory_compress` method**

Find and delete the entire `handle_memory_compress` method (calls `fetch_compressible_groups` + `compress_memory_group`).

- [ ] **Step 3: Remove session_documents search**

Find code that calls `self.memory_store.search_session_documents` and remove it. This is likely in the memory search tool handler — remove the session documents branch, keep the regular memory search.

- [ ] **Step 4: Remove graph extraction calls**

Search for `spawn_graph_extraction` calls and remove them (post-session graph extraction hook).

- [ ] **Step 5: Remove the tool dispatch entries**

In `engine_memory.rs` or `engine_tools.rs`, find where tool names like `"graph_search"` and `"memory_compress"` are dispatched. Remove those match arms.

- [ ] **Step 6: Verify compilation**

Run: `cargo check`

- [ ] **Step 7: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine_memory.rs crates/hydeclaw-core/src/agent/engine_tools.rs
git commit -m "refactor: remove graph_search, memory_compress, session_docs tools from engine"
```

---

### Task 5: Clean memory.rs — Remove Dead Methods

**Files:**
- Modify: `crates/hydeclaw-core/src/memory.rs`

- [ ] **Step 1: Remove graph-related methods from MemoryStore**

Delete these methods from `impl MemoryStore`:
- `find_graph_related()`
- `fetch_chunks_for_graph()`
- `spawn_graph_extraction()`
- Any helper methods only used by these (e.g. entity resolution, edge upsert)

- [ ] **Step 2: Remove compression methods**

Delete:
- `fetch_compressible_groups()`
- `compress_memory_group()`
- `compress_group()` (if exists)

- [ ] **Step 3: Remove session_documents methods**

Delete:
- `search_session_documents()`
- Any session_documents index/insert methods

- [ ] **Step 4: Remove unused imports**

Clean up `use` statements that referenced removed types/functions.

- [ ] **Step 5: Verify compilation**

Run: `cargo check --all-targets`
Expected: Clean compilation (or warnings only)

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/memory.rs
git commit -m "refactor: remove graph, compression, session_docs methods from MemoryStore"
```

---

### Task 6: Clean Tool Definitions

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine_tool_defs.rs`

- [ ] **Step 1: Remove tool definitions**

Find and delete the `ToolDefinition` entries for:
- `graph_search` (or `memory_graph_search`)
- `memory_compress`
- `memory_search_session_docs` (if separate from `memory_search`)

These are JSON schema definitions that register tools with the LLM.

- [ ] **Step 2: Verify compilation**

Run: `cargo check --all-targets`

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine_tool_defs.rs
git commit -m "refactor: remove graph_search, memory_compress tool definitions"
```

---

### Task 7: Clean API Endpoints

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/memory.rs`

- [ ] **Step 1: Remove endpoints**

Delete these handler functions and their route registrations:
- `api_memory_graph` + route `/api/memory/graph`
- `api_extraction_queue` + route `/api/memory/extraction-queue`
- `api_memory_categories` + route `/api/memory/categories`
- `api_memory_topics` + route `/api/memory/topics`

- [ ] **Step 2: Remove graph stats from api_memory_stats**

In `api_memory_stats`, remove the `"graph"` and `"extraction_queue"` sections from the JSON response. Keep the rest (total, pinned, embed_model, etc.).

- [ ] **Step 3: Remove session_documents endpoint if exists**

Check for any `/api/memory/documents` endpoint that serves session_documents specifically (distinct from the memory documents listing). If it queries `session_documents` table, remove it.

- [ ] **Step 4: Verify compilation**

Run: `cargo check --all-targets`

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/memory.rs
git commit -m "refactor: remove graph, extraction-queue, categories, topics API endpoints"
```

---

### Task 8: Remove UI Components

**Files:**
- Delete: `ui/src/components/chat/MemoryPalace.tsx`
- Delete: `ui/src/app/(authenticated)/chat/memory/page.tsx`
- Modify: `ui/package.json` (remove `react-force-graph-2d`)

- [ ] **Step 1: Delete files**

```bash
rm ui/src/components/chat/MemoryPalace.tsx
rm -rf ui/src/app/\(authenticated\)/chat/memory/
```

- [ ] **Step 2: Remove dependency**

```bash
cd ui && npm uninstall react-force-graph-2d
```

- [ ] **Step 3: Remove any imports of MemoryPalace**

Search for `MemoryPalace` across all UI files and remove imports/references.

- [ ] **Step 4: Remove graph-related API calls from memory page**

In `ui/src/app/(authenticated)/memory/page.tsx`, remove any references to `/api/memory/graph`, `/api/memory/categories`, `/api/memory/topics`, `/api/memory/extraction-queue`. Keep the main memory list page functional.

- [ ] **Step 5: Verify UI build**

Run: `cd ui && npm run build`
Expected: Build succeeds

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(ui): remove MemoryPalace, graph visualization, force-graph dependency"
```

---

### Task 9: Final Verification

- [ ] **Step 1: Full Rust build**

Run: `cargo check --all-targets`
Expected: 0 errors

- [ ] **Step 2: Run Rust tests**

Run: `cargo test`
Expected: All pass (some graph-related tests should have been deleted with the files)

- [ ] **Step 3: Full UI build**

Run: `cd ui && npm run build`
Expected: Build succeeds

- [ ] **Step 4: Run UI tests**

Run: `cd ui && npm test -- --run`
Expected: All pass

- [ ] **Step 5: Deploy and health check**

Deploy to Pi, verify `/api/doctor` shows all green with new migration count.

- [ ] **Step 6: Commit any remaining fixes**

```bash
git add -A
git commit -m "chore: cleanup remaining references after memory system simplification"
```
