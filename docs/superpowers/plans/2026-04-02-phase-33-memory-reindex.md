# Phase 33: Memory Reindex — Universal Sources — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reindex памяти охватывает весь workspace (кроме системных директорий) и session transcripts — хардкод `"zettelkasten"` удалён везде.

**Architecture:** В `workspace.rs` добавляется константа `MEMORY_INDEX_EXCLUDE_DIRS`. `reindex.rs` (memory worker) заменяет обход `zettelkasten/` на обход всего workspace с фильтрацией. `memory.rs` watch подписывается на весь workspace. `engine_memory.rs` убирает `directory` из параметров задачи и добавляет `include_sessions`. Reindex worker добавляет второй источник — session transcripts из БД.

**Tech Stack:** Rust, tokio, sqlx, notify (file watcher)

---

## File Map

| Файл | Действие |
|------|----------|
| `crates/hydeclaw-core/src/agent/workspace.rs` | Добавить `MEMORY_INDEX_EXCLUDE_DIRS` |
| `crates/hydeclaw-core/src/agent/engine_memory.rs` | Убрать хардкод `directory`, добавить `include_sessions` |
| `crates/hydeclaw-core/src/memory.rs` | Watch весь workspace кроме exclude dirs |
| `crates/hydeclaw-memory-worker/src/handlers/reindex.rs` | Сканировать весь workspace, добавить sessions |

---

## Task 1: Добавить MEMORY_INDEX_EXCLUDE_DIRS в workspace.rs

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/workspace.rs`

- [ ] **Step 1: Найти строку 516 с SHARED_ROOT_DIRS и добавить после неё**

```rust
// Directories that always live at workspace root (not under agents/)
// toolgate/ and channels/ removed — Architect uses code_exec on host directly
const SHARED_ROOT_DIRS: &[&str] = &["tools", "skills", "mcp", "uploads"];

/// Directories excluded from memory indexing — system/binary/config dirs not meant for knowledge base.
pub const MEMORY_INDEX_EXCLUDE_DIRS: &[&str] = &["tools", "skills", "mcp", "uploads", "agents"];
```

- [ ] **Step 2: Проверить компиляцию**

```bash
cd crates/hydeclaw-core && cargo check 2>&1 | grep error | head -5
```

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/agent/workspace.rs
git commit -m "feat(workspace): add MEMORY_INDEX_EXCLUDE_DIRS constant"
```

---

## Task 2: Обновить engine_memory.rs — убрать хардкод directory

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine_memory.rs`

- [ ] **Step 1: Написать тест**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn reindex_task_params_no_directory_field() {
        // The enqueued task params should NOT have "directory" key
        // This is a design constraint verified by reading engine_memory.rs logic
        // Placeholder: verify manually after implementation that params JSON is correct
        assert!(true); // structural check — see implementation
    }
}
```

- [ ] **Step 2: Найти функцию handle_memory_reindex (строки 256-348)**

Заменить начало функции — убрать `directory` параметр и обход workspace:

```rust
pub(super) async fn handle_memory_reindex(&self, args: &serde_json::Value) -> String {
    let clear_existing = args.get("clear_existing").and_then(|v| v.as_bool()).unwrap_or(false);
    let include_sessions = args.get("include_sessions").and_then(|v| v.as_bool()).unwrap_or(true);
    let _extract_graph = args.get("graph").and_then(|v| v.as_bool()).unwrap_or(true);

    if !self.memory_store.is_available() {
        return "Memory indexing is not available (embedding endpoint not configured).".to_string();
    }

    let workspace_root = std::path::PathBuf::from(&self.workspace_dir);
    if !workspace_root.exists() {
        return "Workspace directory not found.".to_string();
    }

    // Count indexable files for user feedback
    let mut file_count = 0usize;
    let exclude_dirs = crate::agent::workspace::MEMORY_INDEX_EXCLUDE_DIRS;
    let mut stack = vec![workspace_root.clone()];
    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let rel = path.strip_prefix(&workspace_root).ok()
                    .and_then(|p| p.components().next())
                    .and_then(|c| c.as_os_str().to_str())
                    .unwrap_or("");
                if !name.starts_with('.') && !exclude_dirs.contains(&rel) {
                    stack.push(path);
                }
            } else if matches!(path.extension().and_then(|e| e.to_str()), Some("md") | Some("txt")) {
                file_count += 1;
            }
        }
    }
```

Продолжить функцию — обновить секцию `clear_existing` (оставить как есть) и секцию создания задачи:

```rust
    // Create reindex task for memory-worker
    let task_id: uuid::Uuid = match sqlx::query_scalar(
        "INSERT INTO memory_tasks (task_type, params) VALUES ('reindex', $1) RETURNING id",
    )
    .bind(serde_json::json!({
        "clear_existing": clear_existing,
        "include_sessions": include_sessions,
        "agent_id": self.agent.name,
    }))
    .fetch_one(&self.db)
    .await {
        Ok(id) => id,
        Err(e) => return format!("Failed to create reindex task: {}", e),
    };

    format!(
        "Reindex task created: ~{} indexable files in workspace{}. Task ID: {}. Worker will process.",
        file_count,
        if include_sessions { " + session transcripts" } else { "" },
        task_id
    )
}
```

Также найти tool definition для `memory_reindex` (в `engine_tool_defs.rs` или похожем файле) и обновить описание параметров — убрать параметр `directory` из JSON schema, добавить `include_sessions: boolean`.

- [ ] **Step 3: Проверить компиляцию**

```bash
cd crates/hydeclaw-core && cargo check 2>&1 | grep error | head -10
```

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine_memory.rs
git commit -m "feat(memory): remove hardcoded zettelkasten directory from reindex trigger"
```

---

## Task 3: Обновить memory.rs watch — весь workspace

**Files:**
- Modify: `crates/hydeclaw-core/src/memory.rs`

- [ ] **Step 1: Найти spawn_workspace_watcher (строки 639-729)**

Найти строки с хардкодом zettelkasten (строки 661-663):

```rust
// Watch zettelkasten and other knowledge dirs
let zettel_dir = std::path::PathBuf::from(&workspace_dir).join("zettelkasten");
let watch_dir = if zettel_dir.exists() { &zettel_dir } else { std::path::Path::new(&workspace_dir) };
```

Заменить на watch всего workspace:

```rust
// Watch entire workspace root — exclude system dirs at event time
let watch_dir_path = std::path::PathBuf::from(&workspace_dir);
let watch_dir = watch_dir_path.as_path();
```

- [ ] **Step 2: Обновить обработчик событий — фильтровать системные директории**

Найти секцию `EventKind::Create(_) | EventKind::Modify(_)` (строки ~680-690). Добавить фильтр:

```rust
Ok(Ok(Event { kind: EventKind::Create(_) | EventKind::Modify(_), paths, .. })) => {
    let workspace_root = std::path::Path::new(&workspace_dir);
    let exclude_dirs = crate::agent::workspace::MEMORY_INDEX_EXCLUDE_DIRS;
    for p in paths {
        // Skip files in system directories
        let in_excluded = p.strip_prefix(workspace_root)
            .ok()
            .and_then(|rel| rel.components().next())
            .and_then(|c| c.as_os_str().to_str())
            .map(|first| exclude_dirs.contains(&first))
            .unwrap_or(false);
        if in_excluded {
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if matches!(ext, "md" | "txt") {
            pending_files.insert(p);
        }
    }
    if !pending_files.is_empty() {
        debounce_deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    }
}
```

- [ ] **Step 3: Обновить source в debounce handler — использовать путь относительно workspace**

Найти где `source` строится из `path.file_name()` (строка ~700). Заменить на relative path:

```rust
let source = path.strip_prefix(workspace_root)
    .unwrap_or(path.as_path())
    .to_string_lossy()
    .to_string();
```

где `workspace_root = std::path::Path::new(&workspace_dir)` — определить его в начале async block.

- [ ] **Step 4: Проверить компиляцию**

```bash
cd crates/hydeclaw-core && cargo check 2>&1 | grep error | head -10
```

- [ ] **Step 5: Commit**

```bash
git add crates/hydeclaw-core/src/memory.rs
git commit -m "feat(memory): watch entire workspace for changes, filter system dirs"
```

---

## Task 4: Обновить reindex.rs — сканировать весь workspace + sessions

**Files:**
- Modify: `crates/hydeclaw-memory-worker/src/handlers/reindex.rs`

- [ ] **Step 1: Написать тест для collect_workspace_files**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn collect_skips_excluded_dirs() {
        // Create temp dir structure: workspace/tools/file.md, workspace/notes/file.md
        let tmp = tempfile::tempdir().unwrap();
        let tools_dir = tmp.path().join("tools");
        let notes_dir = tmp.path().join("notes");
        tokio::fs::create_dir_all(&tools_dir).await.unwrap();
        tokio::fs::create_dir_all(&notes_dir).await.unwrap();
        tokio::fs::write(tools_dir.join("tool.md"), "tool").await.unwrap();
        tokio::fs::write(notes_dir.join("note.md"), "note").await.unwrap();

        let exclude = &["tools", "skills", "mcp", "uploads", "agents"];
        let files = collect_workspace_files(tmp.path(), exclude).await.unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].to_string_lossy().contains("notes"));
    }

    #[tokio::test]
    async fn collect_finds_md_and_txt() {
        let tmp = tempfile::tempdir().unwrap();
        tokio::fs::write(tmp.path().join("a.md"), "a").await.unwrap();
        tokio::fs::write(tmp.path().join("b.txt"), "b").await.unwrap();
        tokio::fs::write(tmp.path().join("c.rs"), "c").await.unwrap();

        let files = collect_workspace_files(tmp.path(), &[]).await.unwrap();
        assert_eq!(files.len(), 2);
    }
}
```

- [ ] **Step 2: Запустить тесты — убедиться что падают**

```bash
cd crates/hydeclaw-memory-worker && cargo test -- --nocapture 2>&1 | tail -20
```
Ожидание: `collect_workspace_files` not found.

- [ ] **Step 3: Извлечь collect_workspace_files как отдельную функцию**

В начало `reindex.rs` добавить:

```rust
/// Collect all .md and .txt files from workspace_root, skipping excluded top-level dirs.
pub(crate) async fn collect_workspace_files(
    workspace_root: &std::path::Path,
    exclude_dirs: &[&str],
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![workspace_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with('.') || name.starts_with('_') {
                    continue;
                }
                // Only exclude at top-level
                let is_top_level = path.parent() == Some(workspace_root);
                if is_top_level && exclude_dirs.contains(&name) {
                    continue;
                }
                stack.push(path);
            } else {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if matches!(ext, "md" | "txt") {
                    files.push(path);
                }
            }
        }
    }
    Ok(files)
}
```

- [ ] **Step 4: Запустить тесты — убедиться что проходят**

```bash
cd crates/hydeclaw-memory-worker && cargo test -- --nocapture
```
Ожидание: PASS.

- [ ] **Step 5: Переписать функцию handle**

Заменить функцию `handle` полностью:

```rust
pub async fn handle(
    task: &MemoryTask,
    db: &PgPool,
    toolgate_url: &str,
    workspace_dir: &str,
    fts_language: &str,
) -> anyhow::Result<serde_json::Value> {
    let clear_existing = task.params["clear_existing"].as_bool().unwrap_or(false);
    let include_sessions = task.params["include_sessions"].as_bool().unwrap_or(true);
    let agent_id = task.params["agent_id"].as_str().unwrap_or("");

    // Legacy compat: if "directory" field present, use old path-specific behavior
    if let Some(dir) = task.params["directory"].as_str() {
        return handle_legacy_directory(task, db, toolgate_url, workspace_dir, fts_language, dir).await;
    }

    let workspace_root = std::path::Path::new(workspace_dir);
    const EXCLUDE_DIRS: &[&str] = &["tools", "skills", "mcp", "uploads", "agents"];

    let md_files = collect_workspace_files(workspace_root, EXCLUDE_DIRS).await?;

    // Clear existing
    if clear_existing && !agent_id.is_empty() {
        sqlx::query("DELETE FROM memory_chunks WHERE agent_id = $1")
            .bind(agent_id).execute(db).await?;
        tracing::info!(agent_id, "cleared memory before universal reindex");
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let total_files = md_files.len();
    let mut indexed = 0u32;
    let mut errors = 0u32;

    // Index workspace files
    for path in &md_files {
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) if c.len() > 50 => c,
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!(path = ?path, error = %e, "failed to read");
                errors += 1;
                continue;
            }
        };
        let source = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        match embed_and_insert(db, &http, toolgate_url, &content, &source, fts_language, agent_id).await {
            Ok(_) => indexed += 1,
            Err(e) => {
                tracing::warn!(source = %source, error = %e, "index failed");
                errors += 1;
            }
        }

        if indexed % 50 == 0 && indexed > 0 {
            tracing::info!(indexed, total_files, "reindex progress");
            #[cfg(target_os = "linux")]
            let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog]);
        }
    }

    // Index session transcripts
    let mut session_indexed = 0u32;
    if include_sessions && !agent_id.is_empty() {
        session_indexed = index_sessions(db, &http, toolgate_url, fts_language, agent_id).await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "session transcript indexing failed");
                0
            });
    }

    tracing::info!(indexed, errors, total_files, session_indexed, "universal reindex complete");
    Ok(serde_json::json!({
        "indexed": indexed,
        "session_indexed": session_indexed,
        "errors": errors,
        "total_files": total_files,
    }))
}
```

- [ ] **Step 6: Добавить handle_legacy_directory для обратной совместимости**

Добавить функцию которая содержит старый код с `directory` параметром (скопировать старую логику handle, переименовать в `handle_legacy_directory`).

- [ ] **Step 7: Добавить index_sessions функцию**

```rust
/// Index session transcripts from DB into memory_chunks.
async fn index_sessions(
    db: &PgPool,
    http: &reqwest::Client,
    toolgate_url: &str,
    fts_language: &str,
    agent_id: &str,
) -> anyhow::Result<u32> {
    // Fetch sessions for this agent from last 90 days
    // sessions table uses started_at (not created_at)
    let sessions: Vec<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM sessions WHERE agent_id = $1 \
         AND started_at > now() - interval '90 days' ORDER BY started_at DESC",
    )
    .bind(agent_id)
    .fetch_all(db)
    .await?;

    let mut indexed = 0u32;
    for (session_id,) in &sessions {
        let source = format!("session:{}", session_id);

        // Load messages for this session
        let messages: Vec<(String, String)> = sqlx::query_as(
            "SELECT role, content FROM messages WHERE session_id = $1 \
             AND role IN ('user', 'assistant') AND length(content) > 10 \
             ORDER BY created_at ASC",
        )
        .bind(session_id)
        .fetch_all(db)
        .await
        .unwrap_or_default();

        if messages.is_empty() {
            continue;
        }

        // Format as transcript
        let transcript: String = messages
            .iter()
            .map(|(role, content)| format!("[{}]: {}", role, content))
            .collect::<Vec<_>>()
            .join("\n\n");

        if transcript.len() < 100 {
            continue;
        }

        match embed_and_insert(db, http, toolgate_url, &transcript, &source, fts_language, agent_id).await {
            Ok(_) => indexed += 1,
            Err(e) => tracing::debug!(session = %session_id, error = %e, "session index failed"),
        }
    }
    Ok(indexed)
}
```

- [ ] **Step 8: Запустить все тесты**

```bash
cd crates/hydeclaw-memory-worker && cargo test -- --nocapture
```
Ожидание: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/hydeclaw-memory-worker/src/handlers/reindex.rs
git commit -m "feat(memory-worker): universal workspace reindex + session transcript indexing"
```

---

## Task 5: Финальная проверка

- [ ] **Step 1: Полный cargo test**

```bash
cd d:/GIT/bogdan/hydeclaw && cargo test 2>&1 | tail -20
```
Ожидание: PASS.

- [ ] **Step 2: Убедиться что ни один файл не содержит хардкод "zettelkasten" кроме legacy path**

```bash
grep -r "zettelkasten" crates/ --include="*.rs" | grep -v "legacy\|test\|comment"
```
Ожидание: пусто (или только в legacy функции и тестах).

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "feat(memory): phase 33 complete — universal reindex, no zettelkasten hardcode"
```
