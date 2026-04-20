# API Type Drift Audit — Phase D Output

**Date:** 2026-04-20
**Purpose:** Map every UI-facing HTTP endpoint to its `api.ts` interface, classify serialization method, record drift. Input for phases C/B/A of the UI API Type Codegen programme.

## Methodology

Three parallel scans per handler:
1. **Handler inventory** — `ls crates/hydeclaw-core/src/gateway/handlers/` + each `pub(crate) fn routes() -> Router<AppState>`.
2. **Serialization classification** — `grep -n "json!\|Json(json!" handlers/<file>.rs` → hand-rolled; `grep -n "^#\[derive.*Serialize\]" handlers/<file>.rs db/<file>.rs` → typed; both → mixed.
3. **TS mapping** — match endpoint/shape against interfaces in [ui/src/types/api.ts](../../../ui/src/types/api.ts).

## Handler Inventory & Classification

| # | File | Endpoint | Handler fn | Serialization | Rust type | TS interface | Drift |
|---|---|---|---|---|---|---|---|
| 1 | agents/crud.rs | GET /api/agents | api_agents | hand-rolled | — | TBD | TBD |
| 2 | agents/crud.rs | POST /api/agents | api_create_agent | hand-rolled | — | TBD | TBD |
| 3 | agents/crud.rs | GET /api/agents/{name} | api_get_agent | hand-rolled | — | TBD | TBD |
| 4 | agents/crud.rs | PUT /api/agents/{name} | api_update_agent | hand-rolled | — | TBD | TBD |
| 5 | agents/crud.rs | DELETE /api/agents/{name} | api_delete_agent | hand-rolled | — | TBD | TBD |
| 6 | agents/crud.rs | GET /api/agents/{name}/tasks | api_agent_tasks | hand-rolled | — | TBD | TBD |
| 7 | chat.rs (via agents/mod.rs) | POST /api/agents/{name}/model-override | set_model_override | hand-rolled | — | TBD | TBD |
| 8 | agents/crud.rs | GET /api/approvals | api_list_approvals | hand-rolled | — | TBD | TBD |
| 9 | agents/crud.rs | POST /api/approvals/{id}/resolve | api_resolve_approval | hand-rolled | — | TBD | TBD |
| 10 | agents/crud.rs | GET /api/approvals/allowlist | api_list_allowlist | mixed | AllowlistEntry (db/approvals.rs) | TBD | TBD |
| 11 | agents/crud.rs | POST /api/approvals/allowlist | api_add_to_allowlist | hand-rolled | — | TBD | TBD |
| 12 | agents/crud.rs | DELETE /api/approvals/allowlist/{id} | api_delete_from_allowlist | hand-rolled | — | TBD | TBD |

(populated by tasks 3-8)

## Metrics

- **Total endpoints:** N (populated by task 11)
- **Typed (`#[derive(Serialize)]`):** N₁ — phase C scope
- **Hand-rolled (`json!{}`):** N₂ — phase A scope (minus pilot B)
- **Mixed:** N₃ — treated as hand-rolled
- **Handlers with no TS interface (UI uses `unknown`):** N₄
- **TS interfaces with no backing handler (dead code):** N₅ — removed during phase A

## Drift Summary

(list of concrete drifts found, populated by task 10)

## Merge Gate Decision

(populated by task 12)
- Typed ratio: N₁/(N₁+N₂) = __%
- **Gate:** ≥20% typed threshold for C-first priority.
- **Decision:** __ (proceed to phase C | reorder to B-first)
- **Rationale:** __

## Scratchpad — Handler Modules

### All handler files

handlers/access.rs
handlers/agents/crud.rs
handlers/agents/lifecycle.rs
handlers/agents/mod.rs
handlers/agents/schema.rs
handlers/auth.rs
handlers/backup.rs
handlers/cancel_grace.rs
handlers/channel_ws.rs
handlers/channels.rs
handlers/chat.rs
handlers/config.rs
handlers/cron.rs
handlers/csp.rs
handlers/email_triggers.rs
handlers/github_events.rs
handlers/github_repos.rs
handlers/media.rs
handlers/memory.rs
handlers/mod.rs
handlers/monitoring.rs
handlers/network.rs
handlers/notifications.rs
handlers/oauth.rs
handlers/providers.rs
handlers/secrets.rs
handlers/services.rs
handlers/sessions.rs
handlers/skills.rs
handlers/tasks.rs
handlers/tools.rs
handlers/webhooks.rs
handlers/workspace.rs
handlers/yaml_tools.rs

### routes() locations

handlers/access.rs:13
handlers/agents/mod.rs:17
handlers/auth.rs:6
handlers/backup.rs:27
handlers/channel_ws.rs:20
handlers/channels.rs:14
handlers/chat.rs:22
handlers/config.rs:14
handlers/cron.rs:15
handlers/csp.rs:28
handlers/email_triggers.rs:15
handlers/github_repos.rs:12
handlers/media.rs:27
handlers/memory.rs:14
handlers/network.rs:7
handlers/notifications.rs:14
handlers/oauth.rs:13
handlers/providers.rs:24
handlers/secrets.rs:14
handlers/services.rs:16
handlers/sessions.rs:17
handlers/skills.rs:11
handlers/tasks.rs:16
handlers/tools.rs:14
handlers/webhooks.rs:19
handlers/workspace.rs:12
handlers/yaml_tools.rs:13

### Files without routes() (helper modules)

handlers/agents/crud.rs — NO routes()
handlers/agents/lifecycle.rs — NO routes()
handlers/agents/schema.rs — NO routes()
handlers/cancel_grace.rs — NO routes()
handlers/github_events.rs — NO routes()
handlers/mod.rs — NO routes()
handlers/monitoring.rs — NO routes()
