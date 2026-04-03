# Unified Providers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Merge `llm_providers` and `media_providers` into a single `providers` table with unified API, vault, backup, and UI.

**Architecture:** SQL migration creates new tables, copies data, drops old. Rust DB module and handlers rewritten as single unified module. Frontend consolidated into one hook/type set. Toolgate compatibility preserved via proxy endpoint.

**Tech Stack:** Rust (axum, sqlx, tokio, serde), TypeScript/React (Next.js, Zustand, React Query), SQL (PostgreSQL)

---

### Task 1: SQL migration — create unified tables, migrate data, drop old

**Files:**
- Create: `migrations/003_unified_providers.sql`
- Modify: `migrations/001_init.sql` (add `providers` + `provider_active` DDL for fresh installs)

- [ ] **Step 1: Write migration file**

```sql
-- migrations/003_unified_providers.sql

-- 1. Create unified tables
CREATE TABLE IF NOT EXISTS providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL UNIQUE,
    type TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    base_url TEXT,
    default_model TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    options JSONB NOT NULL DEFAULT '{}',
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS provider_active (
    capability TEXT PRIMARY KEY,
    provider_name TEXT REFERENCES providers(name) ON DELETE SET NULL ON UPDATE CASCADE
);

-- 2. Migrate LLM providers (keep UUID, type='text')
INSERT INTO providers (id, name, type, provider_type, base_url, default_model, enabled, options, notes, created_at, updated_at)
SELECT id, name, 'text', provider_type, base_url, default_model, TRUE, '{}'::jsonb, notes, created_at, updated_at
FROM llm_providers
ON CONFLICT (name) DO NOTHING;

-- 3. Migrate media providers (new UUID, name = old id slug)
INSERT INTO providers (name, type, provider_type, base_url, default_model, enabled, options, notes, created_at, updated_at)
SELECT id, type, driver, NULLIF(base_url, ''), model, enabled, options, NULL, created_at, updated_at
FROM media_providers
ON CONFLICT (name) DO NOTHING;

-- 4. Migrate active mappings
INSERT INTO provider_active (capability, provider_name)
SELECT capability, provider_name FROM llm_active WHERE provider_name IS NOT NULL
ON CONFLICT (capability) DO NOTHING;

INSERT INTO provider_active (capability, provider_name)
SELECT ma.capability, mp.id
FROM media_active ma
JOIN media_providers mp ON mp.id = ma.provider_id
WHERE ma.provider_id IS NOT NULL
ON CONFLICT (capability) DO NOTHING;

-- 5. Drop old tables (order matters for FK)
DROP TABLE IF EXISTS llm_active;
DROP TABLE IF EXISTS media_active;
DROP TABLE IF EXISTS media_providers;
DROP TABLE IF EXISTS llm_providers;
```

- [ ] **Step 2: Update 001_init.sql for fresh installs**

In `migrations/001_init.sql`, replace the `llm_providers`, `media_providers`, `media_active` DDL block (lines 344-377) with the new `providers` + `provider_active` schema. Remove the `llm_active` DDL added earlier. This ensures fresh installs get the unified schema directly.

- [ ] **Step 3: Remove migrations/002_llm_active.sql**

Delete this file — its content is superseded by the unified `providers` + `provider_active` in 001 and 003.

- [ ] **Step 4: Commit**

```bash
git add migrations/
git commit -m "migration: add unified providers table, migrate data, drop old tables"
```

---

### Task 2: Rust DB module — `db/providers.rs`

**Files:**
- Create: `crates/hydeclaw-core/src/db/providers.rs`
- Modify: `crates/hydeclaw-core/src/db/mod.rs`
- Delete: `crates/hydeclaw-core/src/db/llm_providers.rs` (after all consumers migrated)
- Delete: `crates/hydeclaw-core/src/db/media_providers.rs` (after all consumers migrated)

- [ ] **Step 1: Create `db/providers.rs` with row structs**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProviderRow {
    pub id: Uuid,
    pub name: String,
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    pub provider_type_category: String,   // text | stt | tts | vision | imagegen | embedding
    pub provider_type: String,            // openai | anthropic | ollama | elevenlabs | ...
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub enabled: bool,
    pub options: Value,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

Wait — the DB column is `type` but we also have `provider_type`. The serde rename creates a collision. Let's use a different approach: rename the `type` column field to avoid the Rust reserved word issue.

Actually, looking at how `MediaProviderRow` handles this (line 9-10 of media_providers.rs): it uses `#[serde(rename = "type")] #[sqlx(rename = "type")] pub provider_type: String`. But now we also have a `provider_type` column in the new table. This is a naming collision.

**Resolution:** In `ProviderRow`, map DB `type` column to Rust field `category`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProviderRow {
    pub id: Uuid,
    pub name: String,
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    pub category: String,          // text | stt | tts | vision | imagegen | embedding
    pub provider_type: String,     // openai | anthropic | ollama | elevenlabs | ...
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub enabled: bool,
    pub options: Value,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProvider {
    pub name: String,
    #[serde(rename = "type")]
    pub category: String,
    pub provider_type: String,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub enabled: Option<bool>,
    pub options: Option<Value>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProvider {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub category: Option<String>,
    pub provider_type: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub enabled: Option<bool>,
    pub options: Option<Value>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProviderActiveRow {
    pub capability: String,
    pub provider_name: Option<String>,
}
```

- [ ] **Step 2: Add CRUD functions**

```rust
pub async fn list_providers(db: &PgPool) -> sqlx::Result<Vec<ProviderRow>> {
    sqlx::query_as::<_, ProviderRow>("SELECT * FROM providers ORDER BY type, name")
        .fetch_all(db)
        .await
}

pub async fn list_providers_by_type(db: &PgPool, category: &str) -> sqlx::Result<Vec<ProviderRow>> {
    sqlx::query_as::<_, ProviderRow>("SELECT * FROM providers WHERE type = $1 ORDER BY name")
        .bind(category)
        .fetch_all(db)
        .await
}

pub async fn get_provider(db: &PgPool, id: Uuid) -> sqlx::Result<Option<ProviderRow>> {
    sqlx::query_as::<_, ProviderRow>("SELECT * FROM providers WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn get_provider_by_name(db: &PgPool, name: &str) -> sqlx::Result<Option<ProviderRow>> {
    sqlx::query_as::<_, ProviderRow>("SELECT * FROM providers WHERE name = $1")
        .bind(name)
        .fetch_optional(db)
        .await
}

pub async fn create_provider(db: &PgPool, input: CreateProvider) -> sqlx::Result<ProviderRow> {
    sqlx::query_as::<_, ProviderRow>(
        r#"INSERT INTO providers (name, type, provider_type, base_url, default_model, enabled, options, notes)
           VALUES ($1, $2, $3, $4, $5, COALESCE($6, TRUE), COALESCE($7, '{}'), $8)
           RETURNING *"#,
    )
    .bind(&input.name)
    .bind(&input.category)
    .bind(&input.provider_type)
    .bind(&input.base_url)
    .bind(&input.default_model)
    .bind(input.enabled)
    .bind(&input.options)
    .bind(&input.notes)
    .fetch_one(db)
    .await
}

pub async fn update_provider(db: &PgPool, id: Uuid, input: UpdateProvider) -> sqlx::Result<Option<ProviderRow>> {
    sqlx::query_as::<_, ProviderRow>(
        r#"UPDATE providers SET
               name           = COALESCE($2, name),
               type           = COALESCE($3, type),
               provider_type  = COALESCE($4, provider_type),
               base_url       = $5,
               default_model  = CASE WHEN $6::text IS NOT NULL THEN $6 ELSE default_model END,
               enabled        = COALESCE($7, enabled),
               options        = COALESCE($8, options),
               notes          = CASE WHEN $9::text IS NOT NULL THEN $9 ELSE notes END,
               updated_at     = NOW()
           WHERE id = $1
           RETURNING *"#,
    )
    .bind(id)
    .bind(&input.name)
    .bind(&input.category)
    .bind(&input.provider_type)
    .bind(&input.base_url)
    .bind(&input.default_model)
    .bind(input.enabled)
    .bind(&input.options)
    .bind(&input.notes)
    .fetch_optional(db)
    .await
}

pub async fn delete_provider(db: &PgPool, id: Uuid) -> sqlx::Result<bool> {
    let res = sqlx::query("DELETE FROM providers WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(res.rows_affected() > 0)
}
```

- [ ] **Step 3: Add provider_active functions**

```rust
pub async fn list_provider_active(db: &PgPool) -> sqlx::Result<Vec<ProviderActiveRow>> {
    sqlx::query_as::<_, ProviderActiveRow>("SELECT * FROM provider_active ORDER BY capability")
        .fetch_all(db)
        .await
}

pub async fn set_provider_active(
    db: &PgPool,
    capability: &str,
    provider_name: Option<&str>,
) -> sqlx::Result<ProviderActiveRow> {
    sqlx::query_as::<_, ProviderActiveRow>(
        r#"INSERT INTO provider_active (capability, provider_name)
           VALUES ($1, $2)
           ON CONFLICT (capability) DO UPDATE SET provider_name = EXCLUDED.provider_name
           RETURNING *"#,
    )
    .bind(capability)
    .bind(provider_name)
    .fetch_one(db)
    .await
}

pub async fn get_provider_active(db: &PgPool, capability: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT provider_name FROM provider_active WHERE capability = $1",
    )
    .bind(capability)
    .fetch_optional(db)
    .await
    .map(|opt| opt.flatten())
}
```

- [ ] **Step 4: Update `db/mod.rs`**

Replace `pub mod llm_providers;` and `pub mod media_providers;` with `pub mod providers;`.

- [ ] **Step 5: Verify compilation**

```bash
cargo check -p hydeclaw-core
```

Expected: errors in consumers of old modules (handlers, backup, main, etc.) — this is expected, we fix them in subsequent tasks.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/db/providers.rs crates/hydeclaw-core/src/db/mod.rs
git commit -m "feat: add unified db/providers.rs module"
```

---

### Task 3: Rust API handlers — `handlers/providers.rs`

**Files:**
- Create: `crates/hydeclaw-core/src/gateway/handlers/providers.rs`
- Modify: `crates/hydeclaw-core/src/gateway/handlers/mod.rs`
- Modify: `crates/hydeclaw-core/src/gateway/mod.rs` (routes + re-exports)

- [ ] **Step 1: Create unified handler file**

Create `crates/hydeclaw-core/src/gateway/handlers/providers.rs` with:

1. **Vault constant:** `pub(crate) const PROVIDER_CREDENTIALS: &str = "PROVIDER_CREDENTIALS";`

2. **Valid types and capabilities:**
```rust
const VALID_TYPES: &[&str] = &["text", "stt", "tts", "vision", "imagegen", "embedding"];
const VALID_CAPABILITIES: &[&str] = &["graph_extraction", "stt", "tts", "vision", "imagegen", "embedding"];
```

3. **`notify_toolgate_reload()`** — move from `media_providers.rs`, same implementation.

4. **`resolve_key()`** — resolve API key from vault: `secrets.get_scoped(PROVIDER_CREDENTIALS, &provider.id.to_string())`

5. **`provider_json()`** — build public JSON with masked key, `has_api_key` boolean.

6. **CRUD handlers:**
   - `api_list_providers(State, Query<Option<type>>)` — list all, optional `?type=` filter
   - `api_create_provider(State, Json<CreateProvider>)` — validate type, name format, default_model required for text; extract api_key to vault
   - `api_get_provider(State, Path<Uuid>)`
   - `api_update_provider(State, Path<Uuid>, Json)` — on type change, clear `provider_active` for capabilities that referenced this provider
   - `api_delete_provider(State, Path<Uuid>)` — delete vault credentials

7. **Model discovery:** `api_provider_models(State, Path<Uuid>)` — same as current `api_llm_provider_models`

8. **Resolve:** `api_provider_resolve(State, Path<Uuid>)` — return unmasked credentials

9. **Active handlers:**
   - `api_list_provider_active(State)` — returns `{"active": [...]}`
   - `api_set_provider_active(State, Json)` — validate capability, upsert

10. **Toolgate proxy:** `api_media_config_export(State)` — query `providers WHERE type IN ('stt','tts','vision','imagegen','embedding')`, build JSON with `"driver"` field mapped from `provider_type`, include unmasked api_key. Same response format as current endpoint.

11. **`api_list_media_drivers()`** — move from media_providers.rs as-is (static data).

12. **Vault migration:** `migrate_provider_keys_to_vault(db, secrets)` — for each provider, check if `PROVIDER_CREDENTIALS::{uuid}` exists. If not, try `LLM_CREDENTIALS::{uuid}` then `MEDIA_CREDENTIALS::{name}`, copy to new scope.

13. **Tests module:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_types_complete() {
        for t in VALID_TYPES {
            assert!(!t.is_empty());
        }
        assert!(VALID_TYPES.contains(&"text"));
        assert!(VALID_TYPES.contains(&"embedding"));
    }

    #[test]
    fn invalid_type_rejected() {
        assert!(!VALID_TYPES.contains(&"audio"));
        assert!(!VALID_TYPES.contains(&""));
    }

    #[test]
    fn valid_capabilities_complete() {
        assert!(VALID_CAPABILITIES.contains(&"graph_extraction"));
        assert!(VALID_CAPABILITIES.contains(&"stt"));
        assert!(VALID_CAPABILITIES.contains(&"embedding"));
    }

    #[test]
    fn provider_active_row_serializes() {
        let row = crate::db::providers::ProviderActiveRow {
            capability: "stt".into(),
            provider_name: Some("whisper-local".into()),
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["capability"], "stt");
        assert_eq!(json["provider_name"], "whisper-local");
    }

    #[test]
    fn create_provider_deserializes() {
        let json = serde_json::json!({
            "name": "my-provider",
            "type": "text",
            "provider_type": "openai",
            "default_model": "gpt-4o"
        });
        let input: crate::db::providers::CreateProvider = serde_json::from_value(json).unwrap();
        assert_eq!(input.category, "text");
        assert_eq!(input.provider_type, "openai");
    }
}
```

- [ ] **Step 2: Update `handlers/mod.rs`**

Replace `pub(crate) mod llm_providers;` and `pub(crate) mod media_providers;` with `pub(crate) mod providers;`. Update re-exports accordingly.

- [ ] **Step 3: Update `gateway/mod.rs` — routes**

Replace old route blocks (lines 99-110) with:

```rust
// Unified providers
.route("/api/providers", get(api_list_providers).post(api_create_provider))
.route("/api/providers/{id}", get(api_get_provider).put(api_update_provider).delete(api_delete_provider))
.route("/api/providers/{id}/models", get(api_provider_models))
.route("/api/providers/{id}/resolve", get(api_provider_resolve))
.route("/api/provider-active", get(api_list_provider_active).put(api_set_provider_active))
.route("/api/provider-types", get(api_list_provider_types))
.route("/api/media-config", get(api_media_config_export))
.route("/api/media-drivers", get(api_list_media_drivers))
```

Update re-exports: replace `migrate_llm_keys_to_vault` and `migrate_media_keys_to_vault` with `migrate_provider_keys_to_vault`.

- [ ] **Step 4: Update `handlers/secrets.rs` line 85**

Change `super::media_providers::notify_toolgate_reload(...)` to `super::providers::notify_toolgate_reload(...)`.

- [ ] **Step 5: Verify compilation**

```bash
cargo check -p hydeclaw-core
```

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/
git commit -m "feat: unified providers API handlers, routes, and vault migration"
```

---

### Task 4: Update agent resolution, graph worker, doctor, main.rs

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/providers.rs` (lines 730, 750-813, 819-827)
- Modify: `crates/hydeclaw-core/src/main.rs` (lines 318-320, 736-765)
- Modify: `crates/hydeclaw-core/src/gateway/handlers/monitoring.rs` (lines 271-280)

- [ ] **Step 1: Update `agent/providers.rs`**

1. Rename `LLM_CREDENTIALS` to `PROVIDER_CREDENTIALS` (line 730):
```rust
pub const PROVIDER_CREDENTIALS: &str = "PROVIDER_CREDENTIALS";
```

2. In `create_provider_from_connection()` (line 750): change parameter type from `&LlmProviderRow` to `&crate::db::providers::ProviderRow`. Remove `api_key_secret_name` usage — set `key_env = ""`.

3. In `resolve_provider_for_agent()` (line 819): change query from `llm_providers::get_llm_provider_by_name` to `providers::get_provider_by_name`, and add `type = 'text'` check on result:
```rust
match crate::db::providers::get_provider_by_name(db, conn_name).await {
    Ok(Some(p)) if p.category == "text" => {
        return create_provider_from_connection(&p, ...);
    }
    ...
}
```

- [ ] **Step 2: Update `main.rs` startup**

1. Replace vault migration calls (lines 318-320):
```rust
gateway::migrate_provider_keys_to_vault(&db_pool, &secrets_mgr).await;
```

2. Update graph worker section (lines 736-765): replace `db::llm_providers::get_llm_active` with `db::providers::get_provider_active`, and `db::llm_providers::get_llm_provider_by_name` with `db::providers::get_provider_by_name`.

- [ ] **Step 3: Update `monitoring.rs` doctor handler**

Replace `llm_providers::list_llm_providers` (line 271) with `providers::list_providers_by_type(&state.db, "text")`. Replace `LLM_CREDENTIALS` with `PROVIDER_CREDENTIALS`. Remove `api_key_secret_name` fallback check.

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p hydeclaw-core
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p hydeclaw-core
```

Expected: all existing tests pass + new provider handler tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/
git commit -m "feat: update agent resolution, graph worker, doctor to use unified providers"
```

---

### Task 5: Update backup/restore

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/backup.rs`

- [ ] **Step 1: Replace backup structs**

Replace `BackupLlmProvider` (lines 90-99), `BackupMediaProvider` (lines 101-110), `BackupMediaActive` (lines 112-116) with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupProvider {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub category: String,
    pub provider_type: String,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub enabled: bool,
    pub options: Value,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupProviderActive {
    pub capability: String,
    pub provider_name: String,
}
```

- [ ] **Step 2: Update `BackupFile` struct**

Replace fields `llm_providers`, `media_providers`, `media_active` with:
```rust
pub providers: Vec<BackupProvider>,
pub provider_active: Vec<BackupProviderActive>,
```

- [ ] **Step 3: Replace collect functions**

Replace `collect_llm_providers`, `collect_media_providers`, `collect_media_active` with:

```rust
async fn collect_providers(db: &PgPool) -> sqlx::Result<Vec<BackupProvider>> {
    let rows = crate::db::providers::list_providers(db).await?;
    Ok(rows.iter().map(|r| BackupProvider {
        id: r.id.to_string(),
        name: r.name.clone(),
        category: r.category.clone(),
        provider_type: r.provider_type.clone(),
        base_url: r.base_url.clone(),
        default_model: r.default_model.clone(),
        enabled: r.enabled,
        options: r.options.clone(),
        notes: r.notes.clone(),
    }).collect())
}

async fn collect_provider_active(db: &PgPool) -> sqlx::Result<Vec<BackupProviderActive>> {
    let rows = crate::db::providers::list_provider_active(db).await?;
    Ok(rows.iter().filter_map(|r| {
        r.provider_name.as_ref().map(|pn| BackupProviderActive {
            capability: r.capability.clone(),
            provider_name: pn.clone(),
        })
    }).collect())
}
```

- [ ] **Step 4: Replace restore functions**

Replace `restore_llm_providers` and `restore_media` with:

```rust
async fn restore_providers(db: &PgPool, providers: &[BackupProvider], active: &[BackupProviderActive]) -> sqlx::Result<usize> {
    sqlx::query("DELETE FROM provider_active").execute(db).await?;
    sqlx::query("DELETE FROM providers").execute(db).await?;

    let mut count = 0;
    for p in providers {
        let id: Uuid = p.id.parse().unwrap_or_else(|_| Uuid::new_v4());
        sqlx::query(
            "INSERT INTO providers (id, name, type, provider_type, base_url, default_model, enabled, options, notes) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        )
        .bind(id).bind(&p.name).bind(&p.category).bind(&p.provider_type)
        .bind(&p.base_url).bind(&p.default_model).bind(p.enabled)
        .bind(&p.options).bind(&p.notes)
        .execute(db).await?;
        count += 1;
    }

    for a in active {
        sqlx::query(
            "INSERT INTO provider_active (capability, provider_name) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        )
        .bind(&a.capability).bind(&a.provider_name)
        .execute(db).await?;
    }

    Ok(count)
}
```

- [ ] **Step 5: Update backup creation and restore call sites**

In the backup creation function, replace calls to `collect_llm_providers`, `collect_media_providers`, `collect_media_active` with `collect_providers`, `collect_provider_active`.

In the restore function, replace calls to `restore_llm_providers` and `restore_media` with `restore_providers`.

- [ ] **Step 6: Verify compilation and tests**

```bash
cargo check -p hydeclaw-core && cargo test -p hydeclaw-core
```

- [ ] **Step 7: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/backup.rs
git commit -m "feat: update backup/restore for unified providers"
```

---

### Task 6: Delete old Rust modules

**Files:**
- Delete: `crates/hydeclaw-core/src/db/llm_providers.rs`
- Delete: `crates/hydeclaw-core/src/db/media_providers.rs`
- Delete: `crates/hydeclaw-core/src/gateway/handlers/llm_providers.rs`
- Delete: `crates/hydeclaw-core/src/gateway/handlers/media_providers.rs`

- [ ] **Step 1: Grep for any remaining imports**

```bash
grep -rn "llm_providers\|media_providers" crates/hydeclaw-core/src/ --include="*.rs" | grep -v "// " | grep -v "target/"
```

Expected: no matches (all consumers migrated in Tasks 2-5).

- [ ] **Step 2: Delete old files**

```bash
rm crates/hydeclaw-core/src/db/llm_providers.rs
rm crates/hydeclaw-core/src/db/media_providers.rs
rm crates/hydeclaw-core/src/gateway/handlers/llm_providers.rs
rm crates/hydeclaw-core/src/gateway/handlers/media_providers.rs
```

- [ ] **Step 3: Full compilation and test**

```bash
cargo test -p hydeclaw-core -p hydeclaw-types -p hydeclaw-watchdog -p hydeclaw-memory-worker
```

Expected: all tests pass, zero errors.

- [ ] **Step 4: Commit**

```bash
git add -A crates/hydeclaw-core/src/
git commit -m "chore: remove old llm_providers and media_providers modules"
```

---

### Task 7: Frontend — types and hooks

**Files:**
- Modify: `ui/src/types/api.ts` (lines 378-432)
- Modify: `ui/src/lib/queries.ts` (lines 362-530)

- [ ] **Step 1: Replace types in `api.ts`**

Replace `LlmProvider`, `CreateLlmProviderInput`, `MediaProvider`, `CreateMediaProviderInput`, `MediaActiveRow`, `LlmActiveRow` with:

```typescript
export interface Provider {
  id: string;
  name: string;
  type: string;            // text | stt | tts | vision | imagegen | embedding
  provider_type: string;   // openai | anthropic | ollama | elevenlabs | ...
  base_url: string | null;
  default_model: string | null;
  has_api_key: boolean;
  enabled: boolean;
  options: Record<string, unknown>;
  notes: string | null;
  created_at: string;
  updated_at: string;
}

export interface CreateProviderInput {
  name: string;
  type: string;
  provider_type: string;
  base_url?: string;
  api_key?: string;
  default_model?: string;
  enabled?: boolean;
  options?: Record<string, unknown>;
  notes?: string;
}

export interface ProviderActiveRow {
  capability: string;
  provider_name: string | null;
}
```

Remove old interfaces: `LlmProvider`, `CreateLlmProviderInput`, `MediaProvider`, `CreateMediaProviderInput`, `MediaActiveRow`, `LlmActiveRow`.

- [ ] **Step 2: Replace hooks in `queries.ts`**

Replace all LLM/media provider hooks with:

```typescript
// Query keys
export const qk = {
  // ... existing keys ...
  providers: ["providers"] as const,
  providerActive: ["provider-active"] as const,
  // remove: llmProviders, mediaProviders, mediaDrivers, mediaActive, llmActive
}

// Hooks
export function useProviders() {
  return useQuery({
    queryKey: qk.providers,
    queryFn: () => apiGet<{ providers: Provider[] }>("/api/providers"),
    select: (d) => d.providers,
    staleTime: 30_000,
  })
}

export function useProviderActive() {
  return useQuery({
    queryKey: qk.providerActive,
    queryFn: () => apiGet<{ active: ProviderActiveRow[] }>("/api/provider-active"),
    select: (d) => d.active,
    staleTime: 30_000,
  })
}

export function useCreateProvider() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (data: CreateProviderInput) => apiPost("/api/providers", data),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.providers })
      qc.invalidateQueries({ queryKey: qk.providerActive })
    },
  })
}

export function useUpdateProvider() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, ...body }: { id: string } & Partial<CreateProviderInput>) =>
      apiPut(`/api/providers/${id}`, body),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.providers })
      qc.invalidateQueries({ queryKey: qk.providerActive })
    },
  })
}

export function useDeleteProvider() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => apiDelete(`/api/providers/${id}`),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.providers })
      qc.invalidateQueries({ queryKey: qk.providerActive })
    },
  })
}

export function useSetProviderActive() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (data: { capability: string; provider_name: string | null }) =>
      apiPut("/api/provider-active", data),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.providerActive }),
  })
}

export function useProviderModels(id: string | null) {
  return useQuery({
    queryKey: qk.providerModels(id ?? ""),
    queryFn: () => apiGet<{ models: string[] }>(`/api/providers/${id}/models`),
    select: (d) => d.models,
    enabled: !!id,
  })
}
```

Remove old hooks: `useLlmProviders`, `useCreateLlmProvider`, `useUpdateLlmProvider`, `useDeleteLlmProvider`, `useLlmProviderModels`, `useMediaProviders`, `useMediaDrivers`, `useMediaActive`, `useCreateMediaProvider`, `useUpdateMediaProvider`, `useDeleteMediaProvider`, `useSetMediaActive`, `useLlmActive`, `useSetLlmActive`.

Keep: `useProviderTypes`, `useMediaDrivers` (static data, still needed for media form).

- [ ] **Step 3: Verify build**

```bash
cd ui && npx tsc --noEmit
```

Expected: type errors in `providers/page.tsx`, `AgentEditDialog.tsx`, test files — fixed in next tasks.

- [ ] **Step 4: Commit**

```bash
git add ui/src/types/api.ts ui/src/lib/queries.ts
git commit -m "feat: unified provider types and hooks"
```

---

### Task 8: Frontend — Providers page + AgentEditDialog

**Files:**
- Modify: `ui/src/app/(authenticated)/providers/page.tsx`
- Modify: `ui/src/app/(authenticated)/agents/AgentEditDialog.tsx`

- [ ] **Step 1: Rewrite providers page imports and hooks**

Replace all split imports/hooks with unified:
- `useProviders()` instead of `useLlmProviders()` + `useMediaProviders()`
- `useProviderActive()` instead of `useMediaActive()` + `useLlmActive()`
- `useSetProviderActive()` instead of `useSetMediaActive()` + `useSetLlmActive()`
- `useCreateProvider()`, `useUpdateProvider()`, `useDeleteProvider()` instead of split versions
- Type `Provider` instead of `LlmProvider | MediaProvider`

Remove the `UnifiedProvider` type and `allProviders` mapping — now there's just one list of `Provider[]`.

- [ ] **Step 2: Unify Active Providers section**

Replace the split Graph + Media sections with a single loop over all capabilities:

```tsx
const ALL_CAPABILITIES = ["graph_extraction", "stt", "tts", "vision", "imagegen", "embedding"] as const;

// For each capability, show dropdown of providers matching the right type
{ALL_CAPABILITIES.map((cap) => {
  const capType = cap === "graph_extraction" ? "text" : cap;
  const capProviders = providers.filter((p) => p.type === capType);
  if (capProviders.length === 0) return null;
  const activeName = active.find((a) => a.capability === cap)?.provider_name ?? "__none__";
  return (
    <div key={cap} className="flex items-center gap-2">
      <span className={badgeClass}>{icon} {label}</span>
      <Select value={activeName} onValueChange={...}>
        ...
      </Select>
    </div>
  );
})}
```

- [ ] **Step 3: Unify provider form**

One form with `type` always editable. Show `default_model` as required when `type=text`. Show `enabled` toggle for all. Show `options` JSON for media types. Remove the split `llmForm` / `mediaForm` state — use a single form state.

- [ ] **Step 4: Update AgentEditDialog**

In `ui/src/app/(authenticated)/agents/AgentEditDialog.tsx`:
- Replace `useLlmProviders()` (line 40) with `useProviders()` and filter by `type === 'text'`
- Replace `useLlmProviderModels(id)` with `useProviderModels(id)`

- [ ] **Step 5: Build and manual test**

```bash
cd ui && npm run build
```

Expected: exit 0.

- [ ] **Step 6: Commit**

```bash
git add ui/src/app/
git commit -m "feat: unified providers UI page and agent dialog"
```

---

### Task 9: Frontend — tests

**Files:**
- Modify: `ui/src/__tests__/pages-smoke.test.tsx`
- Modify: `ui/src/__tests__/queries.test.ts`
- Modify: `ui/src/__tests__/api-coverage.test.ts`

- [ ] **Step 1: Update smoke test mocks**

Replace all old hook mocks with unified:
```typescript
useProviders: () => ({ ...emptyQuery, data: [] }),
useProviderActive: () => ({ ...emptyQuery, data: [] }),
useProviderModels: () => ({ ...emptyQuery, data: [] }),
useCreateProvider: () => ({ ...emptyMutation }),
useUpdateProvider: () => ({ ...emptyMutation }),
useDeleteProvider: () => ({ ...emptyMutation }),
useSetProviderActive: () => ({ ...emptyMutation }),
```

Remove mocks for: `useLlmProviders`, `useMediaProviders`, `useMediaActive`, `useLlmActive`, `useCreateLlmProvider`, `useUpdateLlmProvider`, `useDeleteLlmProvider`, `useCreateMediaProvider`, `useUpdateMediaProvider`, `useDeleteMediaProvider`, `useSetMediaActive`, `useSetLlmActive`, `useLlmProviderModels`.

- [ ] **Step 2: Update query key tests**

Replace old key tests with:
```typescript
it("providers key is stable", () => {
  expect(qk.providers).toEqual(["providers"]);
});

it("providerActive key is stable", () => {
  expect(qk.providerActive).toEqual(["provider-active"]);
});
```

Remove tests for: `llmProviders`, `mediaProviders`, `mediaDrivers`, `mediaActive`, `llmActive`.

- [ ] **Step 3: Update api-coverage.test.ts**

Update endpoint assertions from old paths (`/api/llm-providers`, `/api/media-providers`) to new (`/api/providers`, `/api/provider-active`).

- [ ] **Step 4: Run all tests**

```bash
cd ui && npx vitest run
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add ui/src/__tests__/
git commit -m "test: update all UI tests for unified providers"
```

---

### Task 10: Final verification and deploy

- [ ] **Step 1: Full Rust test suite**

```bash
cargo test -p hydeclaw-core -p hydeclaw-types -p hydeclaw-watchdog -p hydeclaw-memory-worker
```

Expected: all pass.

- [ ] **Step 2: Full UI test suite**

```bash
cd ui && npx vitest run
```

Expected: all pass.

- [ ] **Step 3: UI build**

```bash
cd ui && npm run build
```

Expected: exit 0.

- [ ] **Step 4: Shell script syntax**

```bash
bash -n setup.sh && bash -n update.sh && bash -n uninstall.sh
```

- [ ] **Step 5: Stale references check**

```bash
grep -rn "llm_providers\|media_providers\|LlmProviderRow\|MediaProviderRow\|LLM_CREDENTIALS\|MEDIA_CREDENTIALS\|llm_active\|media_active\|useLlmProviders\|useMediaProviders\|useMediaActive\|useLlmActive" crates/ ui/src/ --include="*.rs" --include="*.ts" --include="*.tsx" -l
```

Expected: no matches.

- [ ] **Step 6: Cross-compile and deploy**

```bash
cargo zigbuild --release --target aarch64-unknown-linux-gnu -p hydeclaw-core -p hydeclaw-watchdog -p hydeclaw-memory-worker
```

Deploy binary, migrations, and UI to Pi. Restart service. Verify `/api/doctor` returns ok and providers page works.

- [ ] **Step 7: Update CLAUDE.md**

Update database section to reference `providers` and `provider_active` instead of old tables. Remove references to `llm_providers`, `media_providers`, `media_active`, `llm_active`.
