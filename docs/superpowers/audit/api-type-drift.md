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
| 13 | chat.rs | GET /health | health | hand-rolled | — | TBD | TBD |
| 14 | chat.rs | POST /api/mcp/callback | mcp_callback | none (StatusCode only) | — | TBD | TBD |
| 15 | chat.rs | POST /v1/chat/completions | chat_completions | mixed | ChatCompletionResponse (chat.rs:150) | TBD | TBD |
| 16 | chat.rs | GET /v1/models | list_models | hand-rolled | — | TBD | TBD |
| 17 | chat.rs | POST /v1/embeddings | embeddings_proxy | hand-rolled | — | TBD | TBD |
| 18 | chat.rs | POST /api/chat | api_chat_sse | SSE — out of scope | — | N/A: see sse-events.ts | N/A: see sse-events.ts |
| 19 | chat.rs | GET /api/chat/{id}/stream | api_chat_resume_stream | SSE — out of scope | — | N/A: see sse-events.ts | N/A: see sse-events.ts |
| 20 | chat.rs | POST /api/chat/{id}/abort | api_chat_abort | hand-rolled | — | TBD | TBD |
| 21 | sessions.rs | GET /api/sessions | api_list_sessions | hand-rolled | — | TBD | TBD |
| 22 | sessions.rs | DELETE /api/sessions | api_delete_all_sessions | hand-rolled | — | TBD | TBD |
| 23 | sessions.rs | GET /api/sessions/latest | api_latest_session | hand-rolled | Session+MessageRow fields inlined (db/sessions.rs:21,280) | TBD | TBD |
| 24 | sessions.rs | GET /api/sessions/search | api_search_sessions | hand-rolled | SearchResult fields inlined (db/sessions.rs:806) | TBD | TBD |
| 25 | sessions.rs | GET /api/sessions/stuck | api_stuck_sessions | hand-rolled | — | TBD | TBD |
| 26 | sessions.rs | DELETE /api/sessions/{id} | api_delete_session | hand-rolled | — | TBD | TBD |
| 27 | sessions.rs | PATCH /api/sessions/{id} | api_patch_session | hand-rolled | — | TBD | TBD |
| 28 | sessions.rs | POST /api/sessions/{id}/compact | api_compact_session | hand-rolled | — | TBD | TBD |
| 29 | sessions.rs | GET /api/sessions/{id}/export | api_export_session | mixed | serde_json::Value from export_session (db/sessions.rs:846) | TBD | TBD |
| 30 | sessions.rs | POST /api/sessions/{id}/invite | api_invite_to_session | hand-rolled | — | TBD | TBD |
| 31 | sessions.rs | GET /api/sessions/{id}/messages | api_session_messages | hand-rolled | MessageRow fields inlined (db/sessions.rs:280) | TBD | TBD |
| 32 | sessions.rs | DELETE /api/messages/{id} | api_delete_message | hand-rolled | — | TBD | TBD |
| 33 | sessions.rs | PATCH /api/messages/{id} | api_patch_message | hand-rolled | — | TBD | TBD |
| 34 | sessions.rs | POST /api/messages/{id}/feedback | api_message_feedback | hand-rolled | — | TBD | TBD |
| 35 | sessions.rs | POST /api/sessions/{id}/fork | api_fork_session | hand-rolled | — | TBD | TBD |
| 36 | sessions.rs | GET /api/sessions/{id}/active-path | api_active_path | hand-rolled | MessageRow fields inlined (db/sessions.rs:280) | TBD | TBD |
| 37 | sessions.rs | POST /api/sessions/{id}/retry | api_retry_session | hand-rolled | — | TBD | TBD |
| 38 | notifications.rs | GET /api/notifications | api_list_notifications | mixed | Notification (db/notifications.rs:7) wrapped in json!{} | TBD | TBD |
| 39 | notifications.rs | POST /api/notifications/read-all | api_mark_all_notifications_read | hand-rolled | — | TBD | TBD |
| 40 | notifications.rs | DELETE /api/notifications/clear | api_clear_all_notifications | hand-rolled | — | TBD | TBD |
| 41 | notifications.rs | PATCH /api/notifications/{id} | api_mark_notification_read | hand-rolled | — | TBD | TBD |
| 42 | cron.rs | GET /api/cron | api_list_cron | hand-rolled | — | TBD | TBD |
| 43 | cron.rs | POST /api/cron | api_create_cron | hand-rolled | — | TBD | TBD |
| 44 | cron.rs | PUT /api/cron/{id} | api_update_cron | hand-rolled | — | TBD | TBD |
| 45 | cron.rs | DELETE /api/cron/{id} | api_delete_cron | hand-rolled | — | TBD | TBD |
| 46 | cron.rs | POST /api/cron/{id}/run | api_run_cron | hand-rolled | — | TBD | TBD |
| 47 | cron.rs | GET /api/cron/{id}/runs | api_cron_runs | hand-rolled | — | TBD | TBD |
| 48 | cron.rs | GET /api/cron/runs | api_cron_runs_all | hand-rolled | — | TBD | TBD |
| 49 | webhooks.rs | GET /api/webhooks | api_list_webhooks | hand-rolled | — | TBD | TBD |
| 50 | webhooks.rs | POST /api/webhooks | api_create_webhook | hand-rolled | — | TBD | TBD |
| 51 | webhooks.rs | PUT /api/webhooks/{id} | api_update_webhook | hand-rolled | — | TBD | TBD |
| 52 | webhooks.rs | DELETE /api/webhooks/{id} | api_delete_webhook | hand-rolled | — | TBD | TBD |
| 53 | webhooks.rs | POST /api/webhooks/{id}/regenerate-secret | api_regenerate_webhook_secret | hand-rolled | — | TBD | TBD |
| 54 | webhooks.rs | POST /webhook/{name} | webhook_handler | hand-rolled | — | TBD | TBD |
| 55 | email_triggers.rs | POST /api/triggers/email/push | gmail_push_handler | none (StatusCode only) | — | N/A | N/A |
| 56 | email_triggers.rs | GET /api/triggers/email | api_list_gmail_triggers | hand-rolled | — | TBD | TBD |
| 57 | email_triggers.rs | POST /api/triggers/email | api_create_gmail_trigger | hand-rolled | — | TBD | TBD |
| 58 | email_triggers.rs | DELETE /api/triggers/email/{id} | api_delete_gmail_trigger | none (StatusCode only) | — | N/A | N/A |
| 59 | providers.rs | GET /api/provider-types | api_list_provider_types | hand-rolled | — | TBD | TBD |
| 60 | providers.rs | GET /api/media-drivers | api_list_media_drivers | hand-rolled | — | TBD | TBD |
| 61 | providers.rs | GET /api/media-config | api_media_config_export | hand-rolled | — | TBD | TBD |
| 62 | providers.rs | GET /api/providers | api_list_providers | mixed | ProviderRow (db/providers.rs:8) augmented with api_key/has_api_key via json!{} | TBD | TBD |
| 63 | providers.rs | POST /api/providers | api_create_provider | mixed | ProviderRow (db/providers.rs:8) augmented with api_key/has_api_key via json!{} | TBD | TBD |
| 64 | providers.rs | GET /api/providers/{id} | api_get_provider | mixed | ProviderRow (db/providers.rs:8) augmented with api_key/has_api_key via json!{} | TBD | TBD |
| 65 | providers.rs | PUT /api/providers/{id} | api_update_provider | mixed | ProviderRow (db/providers.rs:8) augmented with api_key/has_api_key via json!{} | TBD | TBD |
| 66 | providers.rs | DELETE /api/providers/{id} | api_delete_provider | hand-rolled | — | TBD | TBD |
| 67 | providers.rs | GET /api/providers/{id}/models | api_unified_provider_models | hand-rolled | — | TBD | TBD |
| 68 | providers.rs | GET /api/providers/{id}/resolve | api_provider_resolve | hand-rolled | — | TBD | TBD |
| 69 | providers.rs | POST /api/providers/{id}/test-cli | api_test_cli | typed | CliTestResult (providers.rs:683) | TBD | TBD |
| 70 | providers.rs | PATCH /api/providers/{id} | api_patch_cli_options | mixed | ProviderRow + CliTestResult wrapped in json!{} | TBD | TBD |
| 71 | providers.rs | GET /api/provider-active | api_list_provider_active | mixed | ProviderActiveRow (db/providers.rs:56) wrapped in json!{} | TBD | TBD |
| 72 | providers.rs | PUT /api/provider-active | api_set_provider_active | mixed | ProviderActiveRow (db/providers.rs:56) via json!(row) | TBD | TBD |
| 73 | secrets.rs | GET /api/secrets | list_secrets | hand-rolled | — | TBD | TBD |
| 74 | secrets.rs | POST /api/secrets | set_secret | hand-rolled | — | TBD | TBD |
| 75 | secrets.rs | GET /api/secrets/{name} | get_secret | hand-rolled | — | TBD | TBD |
| 76 | secrets.rs | DELETE /api/secrets/{name} | delete_secret | hand-rolled | — | TBD | TBD |
| 77 | channels.rs | GET /api/channels | api_list_all_channels | hand-rolled | — | TBD | TBD |
| 78 | channels.rs | GET /api/channels/active | api_channels_active | hand-rolled | — | TBD | TBD |
| 79 | channels.rs | POST /api/channels/notify | api_channel_notify | hand-rolled | — | TBD | TBD |
| 80 | channels.rs | GET /api/agents/{name}/channels | api_channels_list | hand-rolled | — | TBD | TBD |
| 81 | channels.rs | POST /api/agents/{name}/channels | api_channel_create | hand-rolled | — | TBD | TBD |
| 82 | channels.rs | DELETE /api/agents/{name}/channels/{id} | api_channel_delete | hand-rolled | — | TBD | TBD |
| 83 | channels.rs | PUT /api/agents/{name}/channels/{id} | api_channel_update | hand-rolled | — | TBD | TBD |
| 84 | channels.rs | POST /api/agents/{name}/channels/{id}/restart | api_channel_restart | hand-rolled | — | TBD | TBD |
| 85 | channels.rs | POST /api/agents/{name}/channels/{id}/ack | api_channel_ack | hand-rolled | — | TBD | TBD |
| 86 | channels.rs | GET /api/agents/{name}/channels/{id}/status | api_channel_status | hand-rolled | — | TBD | TBD |
| 87 | services.rs | GET /api/services | api_list_services | hand-rolled | — | TBD | TBD |
| 88 | services.rs | POST /api/services/{name}/{action} | api_service_action | hand-rolled | — | TBD | TBD |
| 89 | services.rs | POST /api/containers/{name}/restart | api_container_restart | hand-rolled | — | TBD | TBD |
| 90 | network.rs | GET /api/network/addresses | api_network_addresses | hand-rolled | — | TBD | TBD |
| 91 | config.rs | GET /api/config/schema | api_get_config_schema | hand-rolled | — | TBD | TBD |
| 92 | config.rs | GET /api/config | api_get_config | hand-rolled | — | TBD | TBD |
| 93 | config.rs | PUT /api/config | api_update_config | hand-rolled | — | TBD | TBD |
| 94 | config.rs | GET /api/config/export | api_export_config | hand-rolled | — | TBD | TBD |
| 95 | config.rs | POST /api/config/import | api_import_config | hand-rolled | — | TBD | TBD |
| 96 | config.rs | POST /api/restart | api_restart | hand-rolled | — | TBD | TBD |
| 97 | config.rs | GET /api/tts/voices | api_tts_voices | hand-rolled | — | TBD | TBD |
| 98 | config.rs | POST /api/tts/synthesize | api_tts_synthesize | none (raw bytes) | — | N/A | N/A |
| 99 | config.rs | GET /api/canvas/{agent} | api_canvas_state | hand-rolled | — | TBD | TBD |
| 100 | config.rs | DELETE /api/canvas/{agent} | api_canvas_clear | none (StatusCode only) | — | N/A | N/A |
| 101 | auth.rs | POST /api/auth/ws-ticket | api_create_ws_ticket | hand-rolled | — | TBD | TBD |

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
