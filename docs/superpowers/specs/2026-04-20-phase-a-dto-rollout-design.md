# Phase A: Typed DTO Rollout — Design Spec

**Date:** 2026-04-20
**Programme:** UI API Type Codegen (Phases B → C → **A**)
**Prereqs:** Phase B (AgentDetailDto pipeline), Phase C (GitHubRepo + AllowlistEntry + CI drift-check)

---

## Goal

Migrate all 38 remaining hand-written TypeScript interfaces in `ui/src/types/api.ts` to generated types via the `ts-rs` codegen pipeline established in Phases B and C. Fix 17 drift findings discovered in the Phase D audit as part of each migration. Achieve 100% typed coverage for UI-facing JSON endpoints that have a declared TS interface.

---

## Architecture

### Approach: DB-first annotation with targeted new DTO structs

Most handler responses are DB rows returned directly (`sqlx::FromRow`). For these, we add two `#[cfg_attr(feature = "ts-gen", ...)]` lines to the existing struct in `src/db/*.rs` — no new files, no new constructors. For response shapes that are not DB rows (computed summaries, wrapper structs), we create a `dto_structs.rs` leaf file alongside the handler module.

This approach was validated in Phase C (`GitHubRepo`, `AllowlistEntry`) and is consistent with Phase B's `AgentDetailDto` pattern.

### Pipeline per type (unchanged from Phase B/C)

```text
DB struct or new dto_structs.rs
  → lib.rs dto_export (ts-gen only)
  → gen_ts_types.rs collect_decl<T>()
  → make gen-types
  → api.generated.ts
  → api.ts re-export alias
```

### Drift fix rule

When migrating a handler, compare (DB struct fields) ↔ (json!{} emitted fields) ↔ (api.ts declared fields). Resolve every discrepancy explicitly — either add the missing field to the emitted JSON, or add `#[serde(skip)]` with justification, or remove from api.ts. No silent mismatches after a wave commits.

---

## Wave Decomposition

Phase A executes in 5 waves. Each wave = one implementation plan. Each wave must leave `cargo check --features ts-gen`, `make gen-types`, and `cd ui && npm run build` green before committing.

| Wave | Handler modules | TS interfaces migrated | Drift fixes | Plan status |
|------|----------------|------------------------|-------------|-------------|
| **W1** | `agents/crud.rs`, `notifications.rs`, `sessions.rs` | `AgentInfo`, `NotificationRow`, `NotificationsResponse`, `SessionRow`, `MessageRow` (+dead code deletion) | 4 | **Write now** |
| **W2** | `channels.rs`, `cron.rs`, `memory.rs` | `ChannelRow`, `ActiveChannel`, `CronJob`, `CronRun`, `MemoryDocument`, `MemoryStats` | 1 | Future |
| **W3** | `tools.rs`, `webhooks.rs`, `approvals.rs`, `backup.rs` | `ToolEntry`, `McpEntry`, `WebhookEntry`, `ApprovalEntry`, `BackupEntry` | 0 | Future |
| **W4** | `providers.rs`, `secrets.rs`, `monitoring.rs` | `Provider`, `ProviderType`, `ProviderActiveRow`, `MediaDriverInfo`, `SecretInfo`, `StatusInfo`, `StatsInfo`, `UsageSummary`, `UsageResponse`, `DailyUsageEntry`, `DailyUsageResponse`, `AuditEvent` | 1 | Future |
| **W5** | `skills.rs`, `tasks.rs`, `workspace.rs`, `oauth.rs`, `yaml_tools.rs` | `SkillEntry`, `TaskStep`, `AgentTask`, `FileEntry`, `OAuthAccount`, `OAuthBinding`, `YamlToolEntry` | 4 | Future |

**Out of scope for Phase A:**

- SSE endpoints (`/api/chat`, `/api/chat/{id}/stream`) — typed via `sse-events.ts`, not `api.ts`
- Request body types (`CreateProviderInput`) — request-side type, not response; kept as hand-written TS
- Endpoints with no TS interface in api.ts (UI uses `unknown`) — unless a future phase adds UI for them
- OpenAI-compatible endpoints (`/v1/chat/completions`, `/v1/models`, `/v1/embeddings`)

---

## Wave 1 Detailed Scope

### Types to migrate

**A. `AgentInfo`** — `handlers/agents/crud.rs`, `GET /api/agents`

Not a DB row. Computed from `AgentConfig` + runtime engine state. Needs new `handlers/agents/dto_structs.rs` entry (the file already exists for Phase B's `AgentDetailDto` — add `AgentInfoDto` to it).

Current `json!{}` fields (from both branches of `api_agents`):

```text
name, language, model, provider, provider_connection, icon, temperature,
has_access, access_mode, has_heartbeat, heartbeat_cron, heartbeat_timezone,
tool_policy {allow, deny, allow_all}, routing_count, is_running, config_dirty,
base (disk-agent branch only), pending_delete (running-no-disk branch only)
```

Drift finding (audit row 1): `fallback_provider` absent from disk-agent branch.

New `AgentInfoDto` struct:
```rust
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts-gen", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-gen", ts(export))]
pub struct AgentInfoDto {
    pub name: String,
    pub language: String,
    pub model: String,
    pub provider: String,
    pub provider_connection: Option<String>,
    pub fallback_provider: Option<String>,
    pub icon: Option<String>,
    pub temperature: f64,
    pub has_access: bool,
    pub access_mode: Option<String>,
    pub has_heartbeat: bool,
    pub heartbeat_cron: Option<String>,
    pub heartbeat_timezone: Option<String>,
    pub tool_policy: Option<AgentInfoToolPolicyDto>,
    pub routing_count: usize,
    pub is_running: bool,
    pub config_dirty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-gen", ts(optional))]
    pub base: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-gen", ts(optional))]
    pub pending_delete: Option<bool>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts-gen", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-gen", ts(export))]
pub struct AgentInfoToolPolicyDto {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub allow_all: bool,
}
```

In `api_agents`, replace `json!({...})` pushes with struct construction + `Json(json!({ "agents": agents }))` where `agents: Vec<AgentInfoDto>`.

`api.ts` change: replace `AgentInfo` interface + `RoutingRule` sub-interface with `export type { AgentInfoDto as AgentInfo } from "./api.generated"`. (`RoutingRule` is only used as part of `AgentDetail`, not `AgentInfo` — remove if unused after check.)

---

**B. `NotificationRow` + `NotificationsResponse`** — `notifications.rs`, `GET /api/notifications`

`Notification` struct in `db/notifications.rs` is already `#[derive(Serialize)]`. Annotate directly (DB-first).

Drift finding (audit row 38): handler wraps in `json!({ "items": [...], "unread_count": N })` — both `items` key and `unread_count` are present. TS interface has both `notifications?` (never populated) and `items?` (optional). Fix: standardise on `items` key in TS. Remove `notifications?` from `NotificationsResponse`.

New `NotificationsResponse` is a simple wrapper DTO (not a DB row), lives in `handlers/notifications/dto_structs.rs` or inline in a new handlers-level file.

`Notification` → `NotificationRow` alias in api.ts. TS `data` field is `Record<string, unknown> | null` — Rust has `serde_json::Value` → maps to `unknown` via `serde-json-impl`. Add `#[ts(type = "Record<string, unknown> | null")]` override.

Actually, checking: `Notification.data` is `serde_json::Value` (not `Option<...>`). The DB column might return NULL. Check `db/notifications.rs` query — if `data` can be null from DB, change to `Option<serde_json::Value>`. If not, keep as `serde_json::Value` (ts-rs emits `unknown`). Adjust `api.ts` accordingly.

---

**C. `SessionRow`** — `sessions.rs`, `GET /api/sessions` + `GET /api/sessions/latest`

`Session` struct in `db/sessions.rs` at line 21. Not `#[derive(Serialize)]` yet. Annotate with Serialize + ts-gen attrs.

Drift finding (audit row 21): `user_id` absent from `GET /api/sessions` emitted JSON. Fix: ensure `user_id` is emitted.

`Session` has extra fields the current `api.ts SessionRow` doesn't declare: `activity_at`, `retry_count`. These are internal — add `#[serde(skip)]` to exclude from API response. (`participants` is in both.)

`api.ts` change: replace `SessionRow` interface with `export type { Session as SessionRow } from "./api.generated"`. Alias `Session` → `SessionRow` to preserve all consumer imports.

---

**D. `MessageRow`** — `sessions.rs`, `GET /api/sessions/latest` + `GET /api/sessions/{id}/messages` + `GET /api/sessions/{id}/active-path`

`MessageRow` struct in `db/sessions.rs` at line 280. Already `#[derive(FromRow)]`. Annotate with Serialize + ts-gen.

Drift finding (audit row 23): `GET /api/sessions/latest` messages block missing `agent_id`, `parent_message_id`, `branch_from_message_id`, `abort_reason`. These exist in the struct. Fix: change `api_latest_session` to use `Json(MessageRow)` instead of manual `json!({...})` construction.

`MessageRow` has `thinking_blocks: Option<serde_json::Value>` — not in current `api.ts`. Add to generated type (it's a real field the UI may want). No `#[serde(skip)]` needed.

`api.ts` change: replace `MessageRow` interface with `export type { MessageRow } from "./api.generated"`.

---

### E. Interfaces that stay hand-written

The Phase D audit labelled `LogEntry` and `CreateProviderInput` as dead. This is incorrect:

- `LogEntry` — used in `monitor/page.tsx` (live logs WebSocket, not HTTP response). Keep as-is.
- `CreateProviderInput` — used in `providers/page.tsx` + `queries.ts` as a POST request body. Keep as-is.
- `RoutingRule` — used by `AgentEditDialog.tsx` + `RoutingRulesEditor.tsx` as a UI form model. Not an API response type; keep as-is, do not generate from Rust.

These three interfaces stay as hand-written TypeScript; they are not candidates for ts-rs codegen.

---

## lib.rs dto_export — Wave 1 Additions

```rust
// Phase A Wave 1: AgentInfoDto (computed summary — dto_structs.rs leaf).
// AgentDetailDto already present via agents_dto (Phase B).
// AgentInfoDto and AgentInfoToolPolicyDto added to the same file.
// No new #[path] needed — agents_dto already includes dto_structs.rs.

// Phase A Wave 1: DB-layer notification type.
#[path = "../db/notifications.rs"]
pub mod notifications_dto;

// Phase A Wave 1: DB-layer session + message types.
// db::sessions is not yet in the always-on lib surface — add it here (ts-gen only).
#[path = "../db/sessions.rs"]
pub mod sessions_dto;
```

**Important:** `db/sessions.rs` is 800+ lines with many async query functions. When included via `#[path]` into `dto_export`, all those functions will be compiled under `ts-gen`. They reference `sqlx::PgPool` etc. — all external deps, no `crate::*`. This is safe (no lib-facade cascade). Verified pattern: same approach used for `github.rs` in Phase C.

Alternatively: add `db::sessions` and `db::notifications` to the always-on `pub mod db` block in lib.rs (like `db::approvals` and `db::usage`). This is cleaner since the modules are large — avoids duplicating them under dto_export. **Preferred: add to always-on db block, then re-export from dto_export via `pub use`.**

---

## gen_ts_types.rs — Wave 1 Additions

```rust
// Phase A Wave 1
use hydeclaw_core::dto_export::{
    agents_dto::{AgentInfoDto, AgentInfoToolPolicyDto},  // added
    notifications_dto::{Notification},
    sessions_dto::{Session, MessageRow},
    // ... existing imports
};

// In main():
// Phase A Wave 1
collect_decl::<AgentInfoDto>(),
collect_decl::<AgentInfoToolPolicyDto>(),
collect_decl::<Notification>(),          // aliased as NotificationRow in api.ts
collect_decl::<Session>(),               // aliased as SessionRow in api.ts
collect_decl::<MessageRow>(),
```

Type count after W1: 14 (current) + 5 = **19 types**

---

## Testing Strategy

Each wave follows this verification sequence (no unit tests for struct shapes — TypeScript build IS the test):

1. `cd crates/hydeclaw-core && cargo check` — default build clean (ts-rs not compiled)
2. `cd crates/hydeclaw-core && cargo check --features ts-gen` — ts-gen build clean
3. `make gen-types` — regenerates `api.generated.ts`; verify new types present with correct field names
4. `grep -E "NewType1|NewType2" ui/src/types/api.generated.ts` — spot-check shapes
5. `cd ui && npm run build` — TypeScript checker validates all consumers

---

## Non-Goals

- Typing request bodies (POST/PUT/PATCH payloads) — those are parsed by serde on the Rust side and never emitted as TypeScript
- Typing error responses (`{"error": "..."}`) — these stay as `json!({"error": ...})`
- Typing SSE events — already covered by `sse-events.ts`
- OpenAI-compat endpoints — no UI consumers
- Typing the 100+ endpoints that have no TS interface in `api.ts` (UI uses `unknown`) — out of scope unless a UI page is added

---

## Success Criteria

- After Wave 5: `api.ts` has zero `export interface` or `export type alias` declarations that duplicate a type from `api.generated.ts`
- `api.generated.ts` contains ≥ 40 types (all migrated interfaces)
- `git diff --exit-code ui/src/types/api.generated.ts` passes in CI (types-drift job, Phase C3)
- All 17 Phase D drift findings resolved
- `cd ui && npm run build` green throughout
