# Unified Providers — Design Spec

## Goal

Merge `llm_providers` and `media_providers` into a single `providers` table. One data model, one API, one UI. Providers are distinguished by a `type` field (`text | stt | tts | vision | imagegen | embedding`). Changing a provider's type is a simple UPDATE.

## Database Schema

```sql
CREATE TABLE providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL UNIQUE,
    type TEXT NOT NULL,              -- text | stt | tts | vision | imagegen | embedding
    provider_type TEXT NOT NULL,     -- openai | anthropic | ollama | google | elevenlabs | ...
    base_url TEXT,
    default_model TEXT,             -- required for type=text, optional for media types
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    options JSONB NOT NULL DEFAULT '{}',
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE provider_active (
    capability TEXT PRIMARY KEY,    -- graph_extraction | stt | tts | vision | imagegen | embedding
    provider_name TEXT REFERENCES providers(name) ON DELETE SET NULL ON UPDATE CASCADE
);
```

### Field mapping from old tables

| New field | From `llm_providers` | From `media_providers` |
|-----------|---------------------|----------------------|
| `id` | `id` (UUID, kept) | gen_random_uuid() (was TEXT slug) |
| `name` | `name` | old `id` slug (e.g. `ollama-local`) |
| `type` | `'text'` | old `type` column (stt/tts/vision/imagegen/embedding) |
| `provider_type` | `provider_type` | `driver` |
| `base_url` | `base_url` | `base_url` |
| `default_model` | `default_model` | `model` |
| `enabled` | `TRUE` (default) | `enabled` |
| `options` | `'{}'` (default) | `options` |
| `notes` | `notes` | `NULL` |

### Dropped fields

- `api_key_secret_name` (llm_providers) — legacy, vault already uses UUID scope. The env-var fallback in `create_provider_from_connection()` that reads this field will pass `""` (empty string), which means `resolve_credential()` skips the env lookup. This is acceptable — all providers on Pi are already vault-migrated.
- `api_key` (media_providers) — legacy column, always NULL after migration to vault

### Vault

All API keys stored under single secret name `PROVIDER_CREDENTIALS`, scoped by provider UUID string. Replaces both `LLM_CREDENTIALS` and `MEDIA_CREDENTIALS`.

**Call sites to update** (rename constant):
- `agent/providers.rs` — definition (line 730) + usage in `create_provider_from_connection` (line 739)
- `gateway/handlers/llm_providers.rs` → becomes `handlers/providers.rs` — 7 usages
- `gateway/handlers/monitoring.rs` — doctor handler (line 274)
- Comment references in `providers_openai.rs`, `providers_anthropic.rs`, `providers_google.rs`
- `gateway/handlers/media_providers.rs` — `MEDIA_CREDENTIALS` definition + 5 usages → merged into `handlers/providers.rs`

### Dropped tables

`llm_providers`, `media_providers`, `media_active`, `llm_active` — all dropped after data migration.

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/providers` | List all providers (optional `?type=text` filter) |
| POST | `/api/providers` | Create provider |
| GET | `/api/providers/{id}` | Get provider by UUID |
| PUT | `/api/providers/{id}` | Update provider (including type change) |
| DELETE | `/api/providers/{id}` | Delete provider + vault cleanup |
| GET | `/api/providers/{id}/models` | Discover models via provider API |
| GET | `/api/providers/{id}/resolve` | Unmasked credentials (internal) |
| GET | `/api/provider-active` | List capability → provider mappings |
| PUT | `/api/provider-active` | Set active provider for capability |
| GET | `/api/provider-types` | Available backend types (kept as-is) |
| GET | `/api/media-config` | Toolgate compatibility proxy (same JSON format) |
| GET | `/api/media-drivers` | Media driver list (kept as-is) |

### Removed endpoints

`/api/llm-providers/*`, `/api/media-providers/*`, `/api/llm-active`, `/api/media-active`

### Validation rules

- `type=text` → `default_model` required
- `name` must match `[a-zA-Z0-9_-]+`, unique
- `type` must be one of: `text`, `stt`, `tts`, `vision`, `imagegen`, `embedding`
- On type change via PUT: if provider was active for old capability, auto-clear `provider_active` for that capability

### Active provider capabilities

Valid capabilities for `provider_active`: `graph_extraction`, `stt`, `tts`, `vision`, `imagegen`, `embedding`.

## Agent Resolution

`resolve_provider_for_agent()` changes from:
```sql
SELECT * FROM llm_providers WHERE name = $1
```
to:
```sql
SELECT * FROM providers WHERE name = $1 AND type = 'text'
```

`create_provider_from_connection()` — unchanged logic, receives `ProviderRow` (renamed from `LlmProviderRow`). The `api_key_secret_name` field is removed from the struct; `key_env` becomes `""` (empty), so the env-var fallback in provider constructors is a no-op. All providers must use vault-scoped credentials.

## Graph Worker

Reads from `provider_active` WHERE capability = 'graph_extraction', then looks up provider by name. Same logic, new table names.

## Toolgate Compatibility

`GET /api/media-config` is preserved as a proxy endpoint. Internally queries `providers WHERE type IN ('stt','tts','vision','imagegen','embedding')` and `provider_active`, formats response in the same JSON structure Toolgate expects:

```json
{
  "version": 1,
  "active": {"stt": "provider-name", "tts": "provider-name", ...},
  "providers": {
    "provider-name": {
      "type": "stt",
      "driver": "<provider_type value>",
      "base_url": "...",
      "model": "...",
      "api_key": "...",
      "enabled": true,
      "options": {}
    }
  }
}
```

**Critical:** The JSON must emit `"driver"` (not `"provider_type"`) because Toolgate's `ProviderConfig` model matches on `(type, driver)` tuple. The handler maps `providers.provider_type` → JSON `"driver"` field. Toolgate code is not modified.

## Doctor / Health Check

`GET /api/doctor` in `monitoring.rs` currently queries `llm_providers` and checks `LLM_CREDENTIALS`. Must be updated to query `providers WHERE type = 'text'` and check `PROVIDER_CREDENTIALS`.

## Startup Migrations

Two existing startup hooks must be replaced:
- `migrate_llm_keys_to_vault()` (in `handlers/llm_providers.rs`) — queries old `llm_providers`
- `migrate_media_keys_to_vault()` (in `handlers/media_providers.rs`) — queries old `media_providers`

Replace with single `migrate_provider_keys_to_vault()` that:
1. For each provider, checks if `PROVIDER_CREDENTIALS::{uuid}` exists in vault
2. If not, checks legacy `LLM_CREDENTIALS::{uuid}` or `MEDIA_CREDENTIALS::{name}` and copies to new scope
3. Idempotent — safe to run on every startup

These are exported from `gateway/mod.rs` lines 21-22 and called in `main.rs` — update both.

## Cross-module Dependencies

`secrets.rs:85` calls `super::media_providers::notify_toolgate_reload()`. After refactor, this module path changes to `super::providers::notify_toolgate_reload()`. The function itself is unchanged.

## Backup/Restore

`backup.rs` has its own raw SQL queries to old tables (not through `db::` modules). Full rewrite needed:

### New backup structures

```rust
struct BackupProvider {
    id: String,
    name: String,
    type_: String,         // serde rename "type"
    provider_type: String,
    base_url: Option<String>,
    default_model: Option<String>,
    enabled: bool,
    options: Value,
    notes: Option<String>,
}

struct BackupProviderActive {
    capability: String,
    provider_name: String,
}
```

### New BackupData fields

Replace `llm_providers`, `media_providers`, `media_active` with `providers` and `provider_active`.

### Collect functions

- `collect_providers()` — `SELECT * FROM providers` (replaces both `collect_llm_providers` and `collect_media_providers`)
- `collect_provider_active()` — `SELECT * FROM provider_active WHERE provider_name IS NOT NULL`

### Restore functions

- `restore_providers()` — DELETE `provider_active`, DELETE `providers`, INSERT from backup
- No backward compatibility with old backup format

## Data Migration

Migration `003_unified_providers.sql`:

1. Create `providers` and `provider_active` tables
2. INSERT from `llm_providers` → `providers` (type='text', enabled=true, options='{}')
3. INSERT from `media_providers` → `providers` (type from old `type`, provider_type from `driver`, new UUID, name from old `id`)
4. INSERT from `media_active` + `llm_active` → `provider_active` (map old provider_id/provider_name to new name)
5. DROP old tables: `llm_active`, `media_active`, `media_providers`, `llm_providers`

Vault credential migration (Rust, one-time at startup via `migrate_provider_keys_to_vault`):
- Former LLM providers: copy from `LLM_CREDENTIALS::{uuid}` → `PROVIDER_CREDENTIALS::{uuid}` (UUID unchanged)
- Former media providers: copy from `MEDIA_CREDENTIALS::{old_text_id}` → `PROVIDER_CREDENTIALS::{new_uuid}`, using name→uuid lookup from `providers` table

## Rust Module Changes

- `db/llm_providers.rs` → `db/providers.rs` (unified CRUD + active operations)
- `db/media_providers.rs` → deleted (merged into `db/providers.rs`)
- `gateway/handlers/llm_providers.rs` → `gateway/handlers/providers.rs` (all unified API handlers)
- `gateway/handlers/media_providers.rs` → deleted (merged, `notify_toolgate_reload` moves to `handlers/providers.rs`)
- `db/mod.rs` — update module declarations
- `gateway/handlers/mod.rs` — update module declarations
- `gateway/mod.rs` — update route definitions, re-exports

## Frontend

### Hooks (in `lib/queries.ts`)

- `useProviders()` replaces `useLlmProviders()` + `useMediaProviders()`
- `useProviderActive()` replaces `useMediaActive()` + `useLlmActive()`
- `useSetProviderActive()` replaces `useSetMediaActive()` + `useSetLlmActive()`
- `useProviderModels(id)` replaces `useLlmProviderModels(id)`
- Remove old hooks: `useLlmProviders`, `useMediaProviders`, `useMediaActive`, `useLlmActive`, `useSetMediaActive`, `useSetLlmActive`, `useLlmProviderModels`

### Types (in `types/api.ts`)

- `Provider` replaces `LlmProvider` + `MediaProvider`
- `CreateProviderInput` replaces `CreateLlmProviderInput` + `CreateMediaProviderInput`
- `ProviderActiveRow` replaces `MediaActiveRow` + `LlmActiveRow`
- Remove old types

### Pages

- `providers/page.tsx` — unified provider list, one form, `type` always editable
- `agents/AgentEditDialog.tsx` — uses `useLlmProviders` → change to `useProviders()` filtered by `type=text`, `useLlmProviderModels` → `useProviderModels`
- `watchdog/page.tsx` — no provider references remain (already cleaned)

### Tests

- `__tests__/pages-smoke.test.tsx` — update mocks: replace old hook mocks with unified `useProviders`, `useProviderActive`, `useSetProviderActive`
- `__tests__/queries.test.ts` — update query key tests
- `__tests__/api-coverage.test.ts` — update endpoint assertions to new `/api/providers/*` paths

## Tests (Rust)

- Validation tests for unified handler (type validation, name format, required fields per type)
- `ProviderRow` serialization tests
- `VALID_CAPABILITIES` and `VALID_TYPES` constant tests
- Toolgate config export format test (verify `"driver"` field present)
