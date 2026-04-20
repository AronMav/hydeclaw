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
| 1 | agents/crud.rs | GET /api/agents | api_agents | hand-rolled | — | `AgentInfo[]` | `fallback_provider` absent from disk-agent branch; `pending_delete` only emitted for running-no-disk branch |
| 2 | agents/crud.rs | POST /api/agents | api_create_agent | hand-rolled | — | — (UI uses unknown) | N/A |
| 3 | agents/crud.rs | GET /api/agents/{name} | api_get_agent | hand-rolled | — | `AgentDetail` | `tool_loop` emits extra `max_loop_nudges`+`ngram_cycle_length`; `routing[].connection` vs TS `routing[].provider`; `routing[]` missing `base_url`, `api_key_env`, `api_key_envs`, `prompt_cache`, `max_tokens` |
| 4 | agents/crud.rs | PUT /api/agents/{name} | api_update_agent | hand-rolled | — | — (UI uses unknown) | N/A |
| 5 | agents/crud.rs | DELETE /api/agents/{name} | api_delete_agent | hand-rolled | — | — (UI uses unknown) | N/A |
| 6 | agents/crud.rs | GET /api/agents/{name}/tasks | api_agent_tasks | hand-rolled | — | `AgentTask[]` | Returns raw file JSON; no normalization — `id` vs TS `task_id`, `agent_id` vs TS `agent`, `input` vs TS `title`, `steps` absent |
| 7 | chat.rs (via agents/mod.rs) | POST /api/agents/{name}/model-override | set_model_override | hand-rolled | — | — (UI uses unknown) | N/A |
| 8 | agents/crud.rs | GET /api/approvals | api_list_approvals | hand-rolled | — | `ApprovalEntry[]` | ✓ all 8 fields match |
| 9 | agents/crud.rs | POST /api/approvals/{id}/resolve | api_resolve_approval | hand-rolled | — | — (UI uses unknown) | N/A |
| 10 | agents/crud.rs | GET /api/approvals/allowlist | api_list_allowlist | mixed | AllowlistEntry (db/approvals.rs) | — (UI uses unknown) | N/A |
| 11 | agents/crud.rs | POST /api/approvals/allowlist | api_add_to_allowlist | hand-rolled | — | — (UI uses unknown) | N/A |
| 12 | agents/crud.rs | DELETE /api/approvals/allowlist/{id} | api_delete_from_allowlist | hand-rolled | — | — (UI uses unknown) | N/A |
| 13 | chat.rs | GET /health | health | hand-rolled | — | — (UI uses unknown) | N/A |
| 14 | chat.rs | POST /api/mcp/callback | mcp_callback | none (StatusCode only) | — | N/A | N/A |
| 15 | chat.rs | POST /v1/chat/completions | chat_completions | mixed | ChatCompletionResponse (chat.rs:150) | — (UI uses unknown) | N/A |
| 16 | chat.rs | GET /v1/models | list_models | hand-rolled | — | — (UI uses unknown) | N/A |
| 17 | chat.rs | POST /v1/embeddings | embeddings_proxy | hand-rolled | — | — (UI uses unknown) | N/A |
| 18 | chat.rs | POST /api/chat | api_chat_sse | SSE — out of scope | — | N/A: see sse-events.ts | N/A: see sse-events.ts |
| 19 | chat.rs | GET /api/chat/{id}/stream | api_chat_resume_stream | SSE — out of scope | — | N/A: see sse-events.ts | N/A: see sse-events.ts |
| 20 | chat.rs | POST /api/chat/{id}/abort | api_chat_abort | hand-rolled | — | — (UI uses unknown) | N/A |
| 21 | sessions.rs | GET /api/sessions | api_list_sessions | hand-rolled | — | `SessionRow[]` | `user_id` absent from emitted JSON |
| 22 | sessions.rs | DELETE /api/sessions | api_delete_all_sessions | hand-rolled | — | — (UI uses unknown) | N/A |
| 23 | sessions.rs | GET /api/sessions/latest | api_latest_session | hand-rolled | Session+MessageRow fields inlined (db/sessions.rs:21,280) | `SessionRow` | Session sub-object missing `user_id`; messages missing `agent_id`, `parent_message_id`, `branch_from_message_id`, `abort_reason` |
| 24 | sessions.rs | GET /api/sessions/search | api_search_sessions | hand-rolled | SearchResult fields inlined (db/sessions.rs:806) | — (UI uses unknown) | N/A |
| 25 | sessions.rs | GET /api/sessions/stuck | api_stuck_sessions | hand-rolled | — | — (UI uses unknown) | N/A |
| 26 | sessions.rs | DELETE /api/sessions/{id} | api_delete_session | hand-rolled | — | — (UI uses unknown) | N/A |
| 27 | sessions.rs | PATCH /api/sessions/{id} | api_patch_session | hand-rolled | — | — (UI uses unknown) | N/A |
| 28 | sessions.rs | POST /api/sessions/{id}/compact | api_compact_session | hand-rolled | — | — (UI uses unknown) | N/A |
| 29 | sessions.rs | GET /api/sessions/{id}/export | api_export_session | mixed | serde_json::Value from export_session (db/sessions.rs:846) | — (UI uses unknown) | N/A |
| 30 | sessions.rs | POST /api/sessions/{id}/invite | api_invite_to_session | hand-rolled | — | — (UI uses unknown) | N/A |
| 31 | sessions.rs | GET /api/sessions/{id}/messages | api_session_messages | hand-rolled | MessageRow fields inlined (db/sessions.rs:280) | `MessageRow[]` | ✓ all 11 TS fields emitted |
| 32 | sessions.rs | DELETE /api/messages/{id} | api_delete_message | hand-rolled | — | — (UI uses unknown) | N/A |
| 33 | sessions.rs | PATCH /api/messages/{id} | api_patch_message | hand-rolled | — | — (UI uses unknown) | N/A |
| 34 | sessions.rs | POST /api/messages/{id}/feedback | api_message_feedback | hand-rolled | — | — (UI uses unknown) | N/A |
| 35 | sessions.rs | POST /api/sessions/{id}/fork | api_fork_session | hand-rolled | — | — (UI uses unknown) | N/A |
| 36 | sessions.rs | GET /api/sessions/{id}/active-path | api_active_path | hand-rolled | MessageRow fields inlined (db/sessions.rs:280) | `MessageRow[]` | ✓ same inline as row 31 — all 11 fields present |
| 37 | sessions.rs | POST /api/sessions/{id}/retry | api_retry_session | hand-rolled | — | — (UI uses unknown) | N/A |
| 38 | notifications.rs | GET /api/notifications | api_list_notifications | mixed | Notification (db/notifications.rs:7) wrapped in json!{} | `NotificationsResponse` | Rust emits `items` key only; TS `notifications?` field never populated; extra `limit`+`offset` keys not in TS interface |
| 39 | notifications.rs | POST /api/notifications/read-all | api_mark_all_notifications_read | hand-rolled | — | — (UI uses unknown) | N/A |
| 40 | notifications.rs | DELETE /api/notifications/clear | api_clear_all_notifications | hand-rolled | — | — (UI uses unknown) | N/A |
| 41 | notifications.rs | PATCH /api/notifications/{id} | api_mark_notification_read | hand-rolled | — | — (UI uses unknown) | N/A |
| 42 | cron.rs | GET /api/cron | api_list_cron | hand-rolled | — | `CronJob[]` | ✓ all 14 TS fields emitted including `tool_policy` |
| 43 | cron.rs | POST /api/cron | api_create_cron | hand-rolled | — | — (UI uses unknown) | N/A |
| 44 | cron.rs | PUT /api/cron/{id} | api_update_cron | hand-rolled | — | — (UI uses unknown) | N/A |
| 45 | cron.rs | DELETE /api/cron/{id} | api_delete_cron | hand-rolled | — | — (UI uses unknown) | N/A |
| 46 | cron.rs | POST /api/cron/{id}/run | api_run_cron | hand-rolled | — | — (UI uses unknown) | N/A |
| 47 | cron.rs | GET /api/cron/{id}/runs | api_cron_runs | hand-rolled | — | `CronRun[]` | `job_name` field absent (emitted by `api_cron_runs_all` but not by the per-job endpoint) |
| 48 | cron.rs | GET /api/cron/runs | api_cron_runs_all | hand-rolled | — | `CronRun[]` | ✓ all 8 fields including `job_name` |
| 49 | webhooks.rs | GET /api/webhooks | api_list_webhooks | hand-rolled | — | `WebhookEntry[]` | ✓ all 11 fields including `webhook_type` and `event_filter` |
| 50 | webhooks.rs | POST /api/webhooks | api_create_webhook | hand-rolled | — | — (UI uses unknown) | N/A |
| 51 | webhooks.rs | PUT /api/webhooks/{id} | api_update_webhook | hand-rolled | — | — (UI uses unknown) | N/A |
| 52 | webhooks.rs | DELETE /api/webhooks/{id} | api_delete_webhook | hand-rolled | — | — (UI uses unknown) | N/A |
| 53 | webhooks.rs | POST /api/webhooks/{id}/regenerate-secret | api_regenerate_webhook_secret | hand-rolled | — | — (UI uses unknown) | N/A |
| 54 | webhooks.rs | POST /webhook/{name} | webhook_handler | hand-rolled | — | — (UI uses unknown) | N/A |
| 55 | email_triggers.rs | POST /api/triggers/email/push | gmail_push_handler | none (StatusCode only) | — | N/A | N/A |
| 56 | email_triggers.rs | GET /api/triggers/email | api_list_gmail_triggers | hand-rolled | — | — (UI uses unknown) | N/A |
| 57 | email_triggers.rs | POST /api/triggers/email | api_create_gmail_trigger | hand-rolled | — | — (UI uses unknown) | N/A |
| 58 | email_triggers.rs | DELETE /api/triggers/email/{id} | api_delete_gmail_trigger | none (StatusCode only) | — | N/A | N/A |
| 59 | providers.rs | GET /api/provider-types | api_list_provider_types | hand-rolled | — | `ProviderType[]` | ✓ all 7 fields match |
| 60 | providers.rs | GET /api/media-drivers | api_list_media_drivers | hand-rolled | — | `MediaDriverInfo[]` | ✓ all 3 fields match (`driver`, `label`, `requires_key`) |
| 61 | providers.rs | GET /api/media-config | api_media_config_export | hand-rolled | — | — (UI uses unknown) | N/A |
| 62 | providers.rs | GET /api/providers | api_list_providers | mixed | ProviderRow (db/providers.rs:8) augmented with api_key/has_api_key via json!{} | `Provider[]` | ✓ `ProviderRow.category` serializes as `"type"` via `#[serde(rename="type")]`; `api_key` masked; all 14 TS fields present |
| 63 | providers.rs | POST /api/providers | api_create_provider | mixed | ProviderRow (db/providers.rs:8) augmented with api_key/has_api_key via json!{} | `Provider` | ✓ same as row 62 |
| 64 | providers.rs | GET /api/providers/{id} | api_get_provider | mixed | ProviderRow (db/providers.rs:8) augmented with api_key/has_api_key via json!{} | `Provider` | ✓ same as row 62 |
| 65 | providers.rs | PUT /api/providers/{id} | api_update_provider | mixed | ProviderRow (db/providers.rs:8) augmented with api_key/has_api_key via json!{} | `Provider` | ✓ same as row 62 |
| 66 | providers.rs | DELETE /api/providers/{id} | api_delete_provider | hand-rolled | — | — (UI uses unknown) | N/A |
| 67 | providers.rs | GET /api/providers/{id}/models | api_unified_provider_models | hand-rolled | — | — (UI uses unknown) | N/A |
| 68 | providers.rs | GET /api/providers/{id}/resolve | api_provider_resolve | hand-rolled | — | — (UI uses unknown) | N/A |
| 69 | providers.rs | POST /api/providers/{id}/test-cli | api_test_cli | typed | CliTestResult (providers.rs:683) | — (UI uses unknown) | N/A |
| 70 | providers.rs | PATCH /api/providers/{id} | api_patch_cli_options | mixed | ProviderRow + CliTestResult wrapped in json!{} | `Provider` | ✓ same shape as row 62 |
| 71 | providers.rs | GET /api/provider-active | api_list_provider_active | mixed | ProviderActiveRow (db/providers.rs:56) wrapped in json!{} | `ProviderActiveRow[]` | ✓ `{capability, provider_name}` matches; response wrapped in `{"active": [...]}` envelope (UI reads `.active`) |
| 72 | providers.rs | PUT /api/provider-active | api_set_provider_active | mixed | ProviderActiveRow (db/providers.rs:56) via json!(row) | `ProviderActiveRow` | ✓ `{capability, provider_name}` matches |
| 73 | secrets.rs | GET /api/secrets | list_secrets | hand-rolled | — | `SecretInfo[]` | ✓ all 6 fields: `name`, `scope`, `description`, `has_value`, `created_at`, `updated_at` |
| 74 | secrets.rs | POST /api/secrets | set_secret | hand-rolled | — | — (UI uses unknown) | N/A |
| 75 | secrets.rs | GET /api/secrets/{name} | get_secret | hand-rolled | — | `SecretInfo` | Emits `{name, masked, length}` — completely wrong shape; missing `scope`, `description`, `has_value`, `created_at`, `updated_at` |
| 76 | secrets.rs | DELETE /api/secrets/{name} | delete_secret | hand-rolled | — | — (UI uses unknown) | N/A |
| 77 | channels.rs | GET /api/channels | api_list_all_channels | hand-rolled | — | `ChannelRow[]` | ✓ all 7 fields: `id`, `agent_name`, `channel_type`, `display_name`, `config`, `status`, `error_msg` |
| 78 | channels.rs | GET /api/channels/active | api_channels_active | hand-rolled | — | `ActiveChannel[]` | ✓ all 6 fields: `agent_name`, `channel_id`, `channel_type`, `display_name`, `adapter_version`, `connected_at`, `last_activity` |
| 79 | channels.rs | POST /api/channels/notify | api_channel_notify | hand-rolled | — | — (UI uses unknown) | N/A |
| 80 | channels.rs | GET /api/agents/{name}/channels | api_channels_list | hand-rolled | — | `ChannelRow[]` | ✓ same as row 77 |
| 81 | channels.rs | POST /api/agents/{name}/channels | api_channel_create | hand-rolled | — | — (UI uses unknown) | N/A |
| 82 | channels.rs | DELETE /api/agents/{name}/channels/{id} | api_channel_delete | hand-rolled | — | — (UI uses unknown) | N/A |
| 83 | channels.rs | PUT /api/agents/{name}/channels/{id} | api_channel_update | hand-rolled | — | — (UI uses unknown) | N/A |
| 84 | channels.rs | POST /api/agents/{name}/channels/{id}/restart | api_channel_restart | hand-rolled | — | — (UI uses unknown) | N/A |
| 85 | channels.rs | POST /api/agents/{name}/channels/{id}/ack | api_channel_ack | hand-rolled | — | — (UI uses unknown) | N/A |
| 86 | channels.rs | GET /api/agents/{name}/channels/{id}/status | api_channel_status | hand-rolled | — | — (UI uses unknown) | N/A |
| 87 | services.rs | GET /api/services | api_list_services | hand-rolled | — | — (UI uses unknown) | N/A |
| 88 | services.rs | POST /api/services/{name}/{action} | api_service_action | hand-rolled | — | — (UI uses unknown) | N/A |
| 89 | services.rs | POST /api/containers/{name}/restart | api_container_restart | hand-rolled | — | — (UI uses unknown) | N/A |
| 90 | network.rs | GET /api/network/addresses | api_network_addresses | hand-rolled | — | — (UI uses unknown) | N/A |
| 91 | config.rs | GET /api/config/schema | api_get_config_schema | hand-rolled | — | — (UI uses unknown) | N/A |
| 92 | config.rs | GET /api/config | api_get_config | hand-rolled | — | — (UI uses unknown) | N/A |
| 93 | config.rs | PUT /api/config | api_update_config | hand-rolled | — | — (UI uses unknown) | N/A |
| 94 | config.rs | GET /api/config/export | api_export_config | hand-rolled | — | — (UI uses unknown) | N/A |
| 95 | config.rs | POST /api/config/import | api_import_config | hand-rolled | — | — (UI uses unknown) | N/A |
| 96 | config.rs | POST /api/restart | api_restart | hand-rolled | — | — (UI uses unknown) | N/A |
| 97 | config.rs | GET /api/tts/voices | api_tts_voices | hand-rolled | — | — (UI uses unknown) | N/A |
| 98 | config.rs | POST /api/tts/synthesize | api_tts_synthesize | none (raw bytes) | — | N/A | N/A |
| 99 | config.rs | GET /api/canvas/{agent} | api_canvas_state | hand-rolled | — | — (UI uses unknown) | N/A |
| 100 | config.rs | DELETE /api/canvas/{agent} | api_canvas_clear | none (StatusCode only) | — | N/A | N/A |
| 101 | auth.rs | POST /api/auth/ws-ticket | api_create_ws_ticket | hand-rolled | — | — (UI uses unknown) | N/A |
| 102 | memory.rs | GET /api/memory | api_list_memory | hand-rolled | — | `{ documents: MemoryDocument[]; stats: MemoryStats }` | Emits `{chunks: [...]}` — wrong envelope key (`chunks` not `documents`) and missing `stats` object entirely |
| 103 | memory.rs | POST /api/memory | api_create_memory | hand-rolled | — | — (UI uses unknown) | N/A |
| 104 | memory.rs | GET /api/memory/stats | api_memory_stats | hand-rolled | — | `MemoryStats` | Extra `tasks` sub-object emitted; TS `MemoryStats` has no `tasks` field (additive, non-breaking for existing reads) |
| 105 | memory.rs | GET /api/memory/export | api_export_memory | hand-rolled | — | — (UI uses unknown) | N/A |
| 106 | memory.rs | GET /api/memory/fts-language | api_get_fts_language | hand-rolled | — | — (UI uses unknown) | N/A |
| 107 | memory.rs | PUT /api/memory/fts-language | api_set_fts_language | hand-rolled | — | — (UI uses unknown) | N/A |
| 108 | memory.rs | GET /api/memory/tasks | api_memory_tasks | hand-rolled | — | — (UI uses unknown) | N/A |
| 109 | memory.rs | GET /api/memory/documents | api_list_documents | hand-rolled | — | `MemoryDocument[]` | ✓ wrapped in `{documents:[...], total}`; all TS fields present including `scope` |
| 110 | memory.rs | GET /api/memory/documents/{id} | api_get_document | hand-rolled | — | `MemoryDocument` | Missing `preview`, `category`, `topic`, `scope`, `similarity` from TS `MemoryDocument`; emits extra `content` field not in TS |
| 111 | memory.rs | PATCH /api/memory/documents/{id} | api_patch_document | hand-rolled | — | — (UI uses unknown) | N/A |
| 112 | memory.rs | DELETE /api/memory/{id} | api_delete_memory | hand-rolled | — | — (UI uses unknown) | N/A |
| 113 | memory.rs | PATCH /api/memory/{id} | api_patch_memory | hand-rolled | — | — (UI uses unknown) | N/A |
| 114 | backup.rs | POST /api/backup | api_create_backup | hand-rolled | — | — (UI uses unknown) | N/A |
| 115 | backup.rs | GET /api/backup | api_list_backups | hand-rolled | — | `BackupEntry[]` | ✓ all 3 fields: `filename`, `size_bytes`, `created_at` |
| 116 | backup.rs | GET /api/backup/{filename} | api_download_backup | none (raw bytes) | — | N/A | N/A |
| 117 | backup.rs | DELETE /api/backup/{filename} | api_delete_backup | hand-rolled | — | — (UI uses unknown) | N/A |
| 118 | backup.rs | POST /api/restore | api_restore | mixed | BackupFile (backup.rs:49) response is hand-rolled json!{} | — (UI uses unknown) | N/A |
| 119 | monitoring.rs | GET /api/setup/status | api_setup_status | hand-rolled | — | — (UI uses unknown) | N/A |
| 120 | monitoring.rs | GET /api/setup/requirements | api_setup_requirements | mixed | CheckResult (monitoring.rs:149) wrapped in json!{} | — (UI uses unknown) | N/A |
| 121 | monitoring.rs | POST /api/setup/complete | api_setup_complete | hand-rolled | — | — (UI uses unknown) | N/A |
| 122 | monitoring.rs | GET /api/status | api_status | hand-rolled | — | `StatusInfo` | ✓ all 10 fields match |
| 123 | monitoring.rs | GET /api/stats | api_stats | hand-rolled | — | `StatsInfo` | ✓ all 5 fields including optional `recent_sessions` |
| 124 | monitoring.rs | GET /api/usage | api_usage | mixed | UsageSummary (db/usage.rs:90) wrapped in json!{} | `UsageResponse` | ✓ `{ok, days, usage:[...]}` matches; `UsageSummary` fields match TS |
| 125 | monitoring.rs | GET /api/usage/daily | api_usage_daily | mixed | DailyUsage (db/usage.rs:132) wrapped in json!{} | `DailyUsageResponse` | ✓ `{ok, days, daily:[...]}` matches; `DailyUsage` fields match `DailyUsageEntry` |
| 126 | monitoring.rs | GET /api/usage/sessions | api_usage_sessions | mixed | SessionUsage (db/usage.rs:177) wrapped in json!{} | — (UI uses unknown) | N/A |
| 127 | monitoring.rs | GET /api/doctor | api_doctor | mixed | CheckResult (monitoring.rs:149) wrapped in json!{} | — (UI uses unknown) | N/A |
| 128 | monitoring.rs | GET /api/health/dashboard | api_health_dashboard | hand-rolled | — | — (UI uses unknown) | N/A |
| 129 | monitoring.rs | GET /api/audit | api_audit_events | mixed | AuditEvent (db/audit.rs:50) wrapped in json!{} | `AuditEvent[]` | ✓ all 6 fields: `id`, `agent_id`, `event_type`, `actor`, `details`, `created_at` |
| 130 | monitoring.rs | GET /api/audit/tools | api_tool_audit | hand-rolled | — | — (UI uses unknown) | N/A |
| 131 | monitoring.rs | GET /api/watchdog/status | api_watchdog_status | hand-rolled | — | — (UI uses unknown) | N/A |
| 132 | monitoring.rs | GET /api/watchdog/config | api_watchdog_config | hand-rolled | — | — (UI uses unknown) | N/A |
| 133 | monitoring.rs | PUT /api/watchdog/config | api_watchdog_config_update | hand-rolled | — | — (UI uses unknown) | N/A |
| 134 | monitoring.rs | GET /api/watchdog/settings | api_watchdog_settings | hand-rolled | — | — (UI uses unknown) | N/A |
| 135 | monitoring.rs | PUT /api/watchdog/settings | api_watchdog_settings_update | hand-rolled | — | — (UI uses unknown) | N/A |
| 136 | monitoring.rs | POST /api/watchdog/restart/{name} | api_watchdog_restart_check | hand-rolled | — | — (UI uses unknown) | N/A |
| 137 | access.rs | GET /api/access/{agent}/pending | api_access_pending | hand-rolled | — | — (UI uses unknown) | N/A |
| 138 | access.rs | POST /api/access/{agent}/approve/{code} | api_access_approve | hand-rolled | — | — (UI uses unknown) | N/A |
| 139 | access.rs | POST /api/access/{agent}/reject/{code} | api_access_reject | hand-rolled | — | — (UI uses unknown) | N/A |
| 140 | access.rs | GET /api/access/{agent}/users | api_access_list_users | hand-rolled | — | — (UI uses unknown) | N/A |
| 141 | access.rs | DELETE /api/access/{agent}/users/{user_id} | api_access_remove_user | hand-rolled | — | — (UI uses unknown) | N/A |
| 142 | channel_ws.rs | GET /ws | ws_handler | WebSocket — out of scope | — | N/A | N/A |
| 143 | channel_ws.rs | GET /ws/channel/{agent_name} | channel_ws_handler | WebSocket — out of scope | — | N/A | N/A |
| 144 | csp.rs | POST /api/csp-report | api_csp_report | none (204/400 only) | — | N/A | N/A |
| 145 | github_repos.rs | GET /api/agents/{name}/github/repos | api_list_github_repos | mixed | GitHubRepo (db/github.rs:18) wrapped in json!{} | `GitHubRepoInfo[]` | ✓ all 5 fields: `id`, `agent_id`, `owner`, `repo`, `added_at` |
| 146 | github_repos.rs | POST /api/agents/{name}/github/repos | api_add_github_repo | typed | GitHubRepo (db/github.rs:18) | `GitHubRepoInfo` | ✓ same shape |
| 147 | github_repos.rs | DELETE /api/agents/{name}/github/repos/{id} | api_delete_github_repo | none (StatusCode only) | — | N/A | N/A |
| 148 | media.rs | POST /api/media/upload | api_media_upload | hand-rolled | — | — (UI uses unknown) | N/A |
| 149 | media.rs | GET /uploads/{filename} | api_media_serve | none (raw bytes) | — | N/A | N/A |
| 150 | oauth.rs | GET /api/oauth/callback | api_oauth_callback | none (Redirect) | — | N/A | N/A |
| 151 | oauth.rs | GET /api/oauth/accounts | api_oauth_accounts_list | hand-rolled | — | `OAuthAccount[]` | ✓ all 9 fields: `id`, `provider`, `display_name`, `user_email`, `scope`, `status`, `expires_at`, `connected_at`, `created_at` |
| 152 | oauth.rs | POST /api/oauth/accounts | api_oauth_account_create | hand-rolled | — | — (UI uses unknown) | N/A |
| 153 | oauth.rs | DELETE /api/oauth/accounts/{id} | api_oauth_account_delete | none (StatusCode only) | — | N/A | N/A |
| 154 | oauth.rs | POST /api/oauth/accounts/{id}/connect | api_oauth_account_connect | hand-rolled | — | — (UI uses unknown) | N/A |
| 155 | oauth.rs | POST /api/oauth/accounts/{id}/revoke | api_oauth_account_revoke | hand-rolled | — | — (UI uses unknown) | N/A |
| 156 | oauth.rs | GET /api/oauth/providers | api_oauth_providers | hand-rolled | — | — (UI uses unknown) | N/A |
| 157 | oauth.rs | GET /api/agents/{name}/oauth/bindings | api_oauth_bindings_list | hand-rolled | — | `OAuthBinding[]` | ✓ all 8 fields: `agent_id`, `provider`, `account_id`, `display_name`, `user_email`, `status`, `expires_at`, `connected_at`, `bound_at` |
| 158 | oauth.rs | POST /api/agents/{name}/oauth/bindings | api_oauth_binding_create | hand-rolled | — | — (UI uses unknown) | N/A |
| 159 | oauth.rs | DELETE /api/agents/{name}/oauth/bindings/{provider} | api_oauth_binding_delete | hand-rolled | — | — (UI uses unknown) | N/A |
| 160 | skills.rs | GET /api/skills | api_skills_list_global | hand-rolled | — | `SkillEntry[]` | ✓ all 6 fields: `name`, `description`, `triggers`, `tools_required`, `priority`, `instructions_len` |
| 161 | skills.rs | GET /api/skills/{skill} | api_skill_get_global | hand-rolled | — | `SkillEntry` | Emits `{name, content, description, triggers, tools_required, priority, instructions}` — `content` and `instructions` are extra; TS expects `instructions_len: number` not the raw text |
| 162 | skills.rs | PUT /api/skills/{skill} | api_skill_upsert_global | hand-rolled | — | — (UI uses unknown) | N/A |
| 163 | skills.rs | DELETE /api/skills/{skill} | api_skill_delete_global | hand-rolled | — | — (UI uses unknown) | N/A |
| 164 | skills.rs | GET /api/agents/{name}/skills | api_skills_list | hand-rolled | — | `SkillEntry[]` | ✓ same as row 160 |
| 165 | skills.rs | GET /api/agents/{name}/skills/{skill} | api_skill_get | hand-rolled | — | `SkillEntry` | Same drift as row 161 — emits `content`+`instructions` instead of `instructions_len` |
| 166 | skills.rs | PUT /api/agents/{name}/skills/{skill} | api_skill_upsert | hand-rolled | — | — (UI uses unknown) | N/A |
| 167 | skills.rs | DELETE /api/agents/{name}/skills/{skill} | api_skill_delete | hand-rolled | — | — (UI uses unknown) | N/A |
| 168 | tasks.rs | GET /api/tasks | api_list_tasks | hand-rolled | TaskRow (tasks/mod.rs:7) wrapped in json!{} | `AgentTask[]` | `id` vs TS `task_id`; `agent_id` vs TS `agent`; `input` vs TS `title`; `steps` absent; extra `user_id`, `source`, `plan`, `result`, `error` not in TS |
| 169 | tasks.rs | POST /api/tasks | api_create_task_endpoint | hand-rolled | — | — (UI uses unknown) | N/A |
| 170 | tasks.rs | GET /api/tasks/audit | api_task_audit | mixed | ToolAuditEntry (db/tool_audit.rs) wrapped in json!{} | — (UI uses unknown) | N/A |
| 171 | tasks.rs | GET /api/tasks/{id} | api_get_task | mixed | TaskRow (tasks/mod.rs:7) via json!(task) | `AgentTask` | Same as row 168 — `id` vs `task_id`, `agent_id` vs `agent`, `input` vs `title`, `steps` absent |
| 172 | tasks.rs | DELETE /api/tasks/{id} | api_delete_task | hand-rolled | — | — (UI uses unknown) | N/A |
| 173 | tasks.rs | GET /api/tasks/{id}/steps | api_task_steps | hand-rolled | TaskStepRow fields inlined via json!{} | `TaskStep[]` | Emits `{id, step_order, mcp_name, action, params, status, output}`; TS `TaskStep` expects `{id, title, status, started_at, finished_at, error}` — completely different shape |
| 174 | tools.rs | GET /api/tool-definitions | api_tool_definitions | hand-rolled | — | — (UI uses unknown) | N/A |
| 175 | tools.rs | GET /api/tools | api_list_tools | hand-rolled | — | `ToolEntry[]` | ✓ all 9 fields: `name`, `url`, `tool_type`, `healthy`, `concurrency_limit`, `healthcheck`, `depends_on`, `ui_path`, `managed` |
| 176 | tools.rs | POST /api/tools | api_tool_service_create | hand-rolled | — | — (UI uses unknown) | N/A |
| 177 | tools.rs | PUT /api/tools/{name} | api_tool_service_update | hand-rolled | — | — (UI uses unknown) | N/A |
| 178 | tools.rs | DELETE /api/tools/{name} | api_tool_service_delete | hand-rolled | — | — (UI uses unknown) | N/A |
| 179 | tools.rs | GET /api/mcp | api_list_mcp | hand-rolled | — | `McpEntry[]` | `idle_timeout` field absent; TS `McpEntry.idle_timeout?: string` never emitted |
| 180 | tools.rs | POST /api/mcp | api_mcp_create | hand-rolled | — | — (UI uses unknown) | N/A |
| 181 | tools.rs | PUT /api/mcp/{name} | api_mcp_update | hand-rolled | — | — (UI uses unknown) | N/A |
| 182 | tools.rs | DELETE /api/mcp/{name} | api_mcp_delete | hand-rolled | — | — (UI uses unknown) | N/A |
| 183 | tools.rs | POST /api/mcp/{name}/reload | api_mcp_reload | hand-rolled | — | — (UI uses unknown) | N/A |
| 184 | tools.rs | POST /api/mcp/{name}/toggle | api_mcp_toggle | hand-rolled | — | — (UI uses unknown) | N/A |
| 185 | workspace.rs | GET /api/workspace | api_workspace_browse | hand-rolled | — | `FileEntry[]` | ✓ all 3 fields: `name`, `is_dir`, `display` |
| 186 | workspace.rs | GET /api/workspace/{*path} | api_workspace_browse | hand-rolled | — | `FileEntry[]` | ✓ same as row 185 |
| 187 | workspace.rs | PUT /api/workspace/{*path} | api_workspace_write | hand-rolled | — | — (UI uses unknown) | N/A |
| 188 | workspace.rs | DELETE /api/workspace/{*path} | api_workspace_delete | hand-rolled | — | — (UI uses unknown) | N/A |
| 189 | yaml_tools.rs | GET /api/yaml-tools | api_yaml_tools_list_global | hand-rolled | — | `YamlToolEntry[]` | ✓ all 7 fields: `name`, `description`, `endpoint`, `method`, `status`, `parameters_count`, `tags` |
| 190 | yaml_tools.rs | POST /api/yaml-tools | api_yaml_tool_create_global | hand-rolled | — | — (UI uses unknown) | N/A |
| 191 | yaml_tools.rs | POST /api/yaml-tools/{tool}/verify | api_yaml_tool_verify_global | hand-rolled | — | — (UI uses unknown) | N/A |
| 192 | yaml_tools.rs | POST /api/yaml-tools/{tool}/disable | api_yaml_tool_disable_global | hand-rolled | — | — (UI uses unknown) | N/A |
| 193 | yaml_tools.rs | POST /api/yaml-tools/{tool}/enable | api_yaml_tool_enable_global | hand-rolled | — | — (UI uses unknown) | N/A |
| 194 | yaml_tools.rs | GET /api/yaml-tools/{tool} | api_yaml_tool_get_global | hand-rolled | — | `YamlToolEntry` | ✓ same shape as row 189 |
| 195 | yaml_tools.rs | PUT /api/yaml-tools/{tool} | api_yaml_tool_update_global | hand-rolled | — | — (UI uses unknown) | N/A |
| 196 | yaml_tools.rs | DELETE /api/yaml-tools/{tool} | api_yaml_tool_delete_global | hand-rolled | — | — (UI uses unknown) | N/A |
| 197 | yaml_tools.rs | GET /api/agents/{name}/yaml-tools | api_yaml_tools_list | hand-rolled | — | `YamlToolEntry[]` | ✓ same as row 189 |
| 198 | yaml_tools.rs | POST /api/agents/{name}/yaml-tools/{tool}/verify | api_yaml_tool_verify | hand-rolled | — | — (UI uses unknown) | N/A |
| 199 | yaml_tools.rs | POST /api/agents/{name}/yaml-tools/{tool}/disable | api_yaml_tool_disable | hand-rolled | — | — (UI uses unknown) | N/A |

## Metrics

- **Total endpoints (in scope):** 184
- **Typed (`#[derive(Serialize)]`):** 2 — phase C scope
- **Hand-rolled (`json!{}`):** 161 — phase A scope (minus pilot B)
- **Mixed:** 21 — treated as hand-rolled
- **Handlers with no TS interface (UI uses `unknown`):** 129
- **TS interfaces with no backing handler (dead code):** 3 — removed during phase A
- **Typed ratio:** 2/(2+161+21) = 1.1%

## Drift Summary

### Drift: `AgentInfo` — `fallback_provider` absent from disk-agent branch
**UI declaration:** `api.ts:43` — `fallback_provider?: string | null;`
**Rust handler:** `agents/crud.rs` — the disk-agent code path (`json!{...}`) omits `fallback_provider`; only the running-but-no-disk branch sets `pending_delete: true` and also lacks `fallback_provider`
**Impact:** `AgentInfo.fallback_provider` is always `undefined` in the UI for all agents loaded from disk config; fallback-provider display in the UI never renders
**Fix at:** phase A iteration for `agents/crud.rs` `api_agents` — add `fallback_provider` to the `json!{}` block from `agent_state.config.fallback_provider`

---

### Drift: `AgentDetail.routing[].provider` — Rust emits `connection` instead
**UI declaration:** `api.ts:49` — `provider: string;` (in `RoutingRule`)
**Rust handler:** `agents/schema.rs` `agent_to_detail()` — emits `"connection"` key, not `"provider"`; also missing `base_url`, `api_key_env`, `api_key_envs`, `prompt_cache`, `max_tokens` from each rule
**Impact:** `RoutingRule.provider` is always `undefined` in the UI; the routing rules editor in `RoutingRulesEditor.tsx` renders empty provider fields
**Fix at:** phase A iteration for `agents/schema.rs` — rename key to `provider`, add missing optional fields

---

### Drift: `AgentDetail.tool_loop` — extra undeclared fields emitted
**UI declaration:** `api.ts:82-86` — `tool_loop` has `max_iterations`, `compact_on_overflow`, `detect_loops`, `warn_threshold`, `break_threshold`, `max_consecutive_failures?`, `max_auto_continues?`
**Rust handler:** `agents/schema.rs` `agent_to_detail()` — also emits `max_loop_nudges` and `ngram_cycle_length` (undeclared in TS)
**Impact:** additive only — TS ignores unknown fields; no UI breakage currently; fields are unused dead weight in the payload
**Fix at:** phase A cleanup — remove or declare in TS

---

### Drift: `AgentTask` — field names completely wrong for task endpoints
**UI declaration:** `api.ts:468-476` — `task_id`, `agent`, `title`, `status`, `created_at`, `updated_at`, `steps: TaskStep[]`
**Rust handler:** `tasks.rs` `api_list_tasks` / `api_get_task` — emits `id`, `agent_id`, `input`, `status`, `created_at`, `updated_at`; `steps` absent; extra `user_id`, `source`, `plan`, `result`, `error`
**Impact:** `TaskPlanPanel.tsx` and `tasks/page.tsx` see `undefined` for `task_id`, `agent`, `title`, `steps` — task UI is broken
**Fix at:** phase A iteration for `tasks.rs` — either rename DB columns or remap in the handler JSON; add step fetching or embed steps

---

### Drift: `TaskStep` — completely different shape
**UI declaration:** `api.ts:459-466` — `{id, title, status, started_at, finished_at, error}`
**Rust handler:** `tasks.rs` `api_task_steps` — emits `{id, step_order, mcp_name, action, params, status, output}`
**Impact:** `TaskPlanPanel.tsx` receives `undefined` for `title`, `started_at`, `finished_at`, `error`; task step display is broken
**Fix at:** phase A iteration for `tasks.rs` — remap `TaskStepRow` fields to TS contract, or update both sides together

---

### Drift: `SessionRow.user_id` — absent from list and latest endpoints
**UI declaration:** `api.ts:106` — `user_id: string;`
**Rust handler:** `sessions.rs` `api_list_sessions` / `api_latest_session` — `user_id` is not included in the `json!{}` blocks
**Impact:** `SessionRow.user_id` is `undefined` in the UI for all session list views and latest-session loads; any code relying on `user_id` silently fails
**Fix at:** phase A iteration for `sessions.rs` — add `"user_id": session.user_id` to both `json!{}` bodies

---

### Drift: `MessageRow` — four fields absent from `api_latest_session` messages
**UI declaration:** `api.ts:123-130` — `agent_id?`, `parent_message_id`, `branch_from_message_id`, `abort_reason?`
**Rust handler:** `sessions.rs` `api_latest_session` — the inline message `json!{}` omits `agent_id`, `parent_message_id`, `branch_from_message_id`, `abort_reason`
**Impact:** branching UI (`branch_from_message_id`) and agent attribution (`agent_id`) don't work on initial session load via the latest endpoint; only the dedicated `GET /api/sessions/{id}/messages` endpoint emits the full shape
**Fix at:** phase A iteration for `sessions.rs` `api_latest_session` — align message inline json with the full `api_session_messages` shape

---

### Drift: `NotificationsResponse` — `notifications?` key never populated
**UI declaration:** `api.ts:453-458` — `notifications?: NotificationRow[]; items?: NotificationRow[]; unread_count: number`
**Rust handler:** `notifications.rs` `api_list_notifications` — emits `{"items": [...], "unread_count": N, "limit": N, "offset": N}`; `notifications` key is never present; extra `limit`/`offset` keys not in TS
**Impact:** TS consumers that check `.notifications` always get `undefined`; consumers using `.items` work correctly; `limit`/`offset` are ignored but create noise
**Fix at:** phase A iteration for `notifications.rs` — emit both `notifications` and `items` (or remove the dead `notifications?` from TS); drop `limit`/`offset` from response or add to TS

---

### Drift: `CronRun.job_name` — absent from per-job endpoint
**UI declaration:** `api.ts:153-161` — `job_name?: string;`
**Rust handler:** `cron.rs` `api_cron_runs` (per-job endpoint) — does not emit `job_name`; only `api_cron_runs_all` emits it
**Impact:** CronRun rows loaded via the per-job endpoint have `job_name === undefined`; monitor page displaying per-job runs shows no job name
**Fix at:** phase A iteration for `cron.rs` `api_cron_runs` — add `"job_name": job_name` lookup or include in the query

---

### Drift: `SecretInfo` — GET single secret returns wrong shape
**UI declaration:** `api.ts:238-245` — `{name, scope, description, has_value, created_at, updated_at}`
**Rust handler:** `secrets.rs` `get_secret` — emits `{"name": ..., "masked": ..., "length": ...}`; missing `scope`, `description`, `has_value`, `created_at`, `updated_at`; extra `masked` and `length` not in TS
**Impact:** UI code treating a single secret as `SecretInfo` sees `undefined` for all fields except `name`; secret detail views are broken
**Fix at:** phase A iteration for `secrets.rs` `get_secret` — return full `SecretInfo` shape, move `masked`/`length` to separate optional fields or a different interface

---

### Drift: `GET /api/memory` — wrong envelope and missing stats
**UI declaration:** `api.ts:102` — response typed as `{ documents: MemoryDocument[]; stats: MemoryStats }`
**Rust handler:** `memory.rs` `api_list_memory` — emits `{"chunks": [...]}` with no `stats` object
**Impact:** `memory/page.tsx` using this endpoint receives `undefined` for both `documents` and `stats`; memory page cannot render documents or stats on this endpoint
**Fix at:** phase A iteration for `memory.rs` `api_list_memory` — rename key from `chunks` to `documents`, add `stats` sub-object (can reuse `api_memory_stats` query)

---

### Drift: `MemoryDocument` — single document endpoint missing 5 fields
**UI declaration:** `api.ts:163-177` — `preview`, `category`, `topic`, `scope`, `similarity`, `chunks_count`, `total_chars`, `relevance_score`, `pinned`, `source`, `id`, `created_at`, `accessed_at`
**Rust handler:** `memory.rs` `api_get_document` — emits `{id, source, pinned, relevance_score, created_at, accessed_at, content, chunks_count, total_chars}`; missing `preview`, `category`, `topic`, `scope`, `similarity`; extra `content` not in TS
**Impact:** Document detail views missing category/topic/scope labels; `similarity` score absent; `preview` field `undefined` so preview rendering falls back to empty
**Fix at:** phase A iteration for `memory.rs` `api_get_document` — add missing fields from DB query; rename/alias `content` → `preview` or add `preview` as a truncated field

---

### Drift: `SkillEntry` — single-skill GET returns `content`/`instructions` instead of `instructions_len`
**UI declaration:** `api.ts:200-207` — `instructions_len: number;` (not the raw content)
**Rust handler:** `skills.rs` `api_skill_get_global` / `api_skill_get` — emits `{name, content, description, triggers, tools_required, priority, instructions}` with full text; `instructions_len` absent
**Impact:** UI rendering `instructions_len` for a single skill gets `undefined`; full `content` is exposed unnecessarily (may be large); list endpoint correctly emits `instructions_len`
**Fix at:** phase A iteration for `skills.rs` get handlers — add `instructions_len: content.len()`, keep `content` as an additional field or remove it based on UI need

---

### Drift: `McpEntry.idle_timeout` — always absent
**UI declaration:** `api.ts:225` — `idle_timeout?: string;`
**Rust handler:** `tools.rs` `api_list_mcp` — does not emit `idle_timeout` key
**Impact:** MCP idle timeout config is invisible in the UI; any timeout display shows `undefined`
**Fix at:** phase A iteration for `tools.rs` `api_list_mcp` — add `"idle_timeout": entry.idle_timeout` from the MCP config struct

## Merge Gate Decision

(populated by task 12)
- Typed ratio: N₁/(N₁+N₂) = __%
- **Gate:** ≥20% typed threshold for C-first priority.
- **Decision:** __ (proceed to phase C | reorder to B-first)
- **Rationale:** __

## Dead TS Interfaces (candidate removal in phase A)

- `LogEntry` — no GET /api/logs endpoint found in handler inventory; UI consumers: `ui/src/app/(authenticated)/monitor/page.tsx`
- `DailyUsageEntry` — used only as member type within `DailyUsageResponse`; not directly consumed by any endpoint row; UI consumers: (none — only referenced inside `DailyUsageResponse` definition)
- `CreateProviderInput` — input DTO (request body shape), not a response type; no GET endpoint returns this; UI consumers: `ui/src/app/(authenticated)/providers/page.tsx`, `ui/src/lib/queries.ts`

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

## Scratchpad — TS Interfaces

### All exported interfaces/types from ui/src/types/api.ts

| Interface | External UI consumers (files outside api.ts) |
|---|---|
| `StatusInfo` | `ui/src/app/(authenticated)/monitor/page.tsx` |
| `StatsInfo` | `ui/src/app/(authenticated)/monitor/page.tsx` |
| `AgentInfo` | `ui/src/app/(authenticated)/agents/page.tsx`, `ui/src/lib/queries.ts`, `ui/src/stores/chat-history.ts`, multiple others |
| `RoutingRule` | `ui/src/app/(authenticated)/agents/AgentEditDialog.tsx`, `ui/src/app/(authenticated)/agents/RoutingRulesEditor.tsx`, `ui/src/app/(authenticated)/agents/page.tsx` |
| `AgentDetail` | `ui/src/app/(authenticated)/agents/page.tsx`, `ui/src/lib/queries.ts`, `ui/src/app/(authenticated)/agents/AgentEditDialog.tsx` |
| `SessionRow` | `ui/src/stores/chat-history.ts`, `ui/src/lib/queries.ts`, `ui/src/stores/chat/actions/session-crud.ts`, multiple others |
| `MessageRow` | `ui/src/app/(authenticated)/chat/ChatThread.tsx`, `ui/src/stores/chat-history.ts`, `ui/src/lib/queries.ts`, multiple others |
| `CronJob` | `ui/src/lib/queries.ts`, `ui/src/app/(authenticated)/monitor/page.tsx` |
| `CronRun` | `ui/src/lib/queries.ts`, `ui/src/app/(authenticated)/monitor/page.tsx` |
| `MemoryDocument` | `ui/src/app/(authenticated)/memory/page.tsx`, `ui/src/lib/queries.ts` |
| `MemoryStats` | `ui/src/app/(authenticated)/memory/page.tsx`, `ui/src/lib/queries.ts` |
| `ToolEntry` | `ui/src/app/(authenticated)/tools/page.tsx`, `ui/src/lib/queries.ts` |
| `SkillEntry` | `ui/src/app/(authenticated)/skills/page.tsx`, `ui/src/lib/queries.ts` |
| `YamlToolEntry` | `ui/src/lib/queries.ts` |
| `McpEntry` | `ui/src/lib/queries.ts` |
| `FileEntry` | `ui/src/app/(authenticated)/workspace/page.tsx` |
| `SecretInfo` | `ui/src/lib/queries.ts` |
| `GitHubRepoInfo` | `ui/src/app/(authenticated)/integrations/page.tsx` |
| `OAuthAccount` | `ui/src/app/(authenticated)/integrations/page.tsx`, `ui/src/lib/queries.ts` |
| `OAuthBinding` | `ui/src/app/(authenticated)/integrations/page.tsx`, `ui/src/lib/queries.ts` |
| `LogEntry` | `ui/src/app/(authenticated)/monitor/page.tsx` |
| `UsageSummary` | `ui/src/app/(authenticated)/monitor/page.tsx` |
| `UsageResponse` | `ui/src/lib/queries.ts` |
| `DailyUsageEntry` | (none — used only as member type within `DailyUsageResponse`) |
| `DailyUsageResponse` | `ui/src/lib/queries.ts` |
| `AuditEvent` | `ui/src/app/(authenticated)/monitor/page.tsx`, `ui/src/lib/queries.ts`, `ui/src/types/ws.ts` |
| `ChannelRow` | `ui/src/app/(authenticated)/channels/page.tsx`, `ui/src/lib/queries.ts` |
| `ActiveChannel` | `ui/src/lib/queries.ts` |
| `BackupEntry` | `ui/src/lib/queries.ts` |
| `WebhookEntry` | `ui/src/app/(authenticated)/webhooks/page.tsx`, `ui/src/lib/queries.ts` |
| `ApprovalEntry` | `ui/src/app/(authenticated)/agents/page.tsx`, `ui/src/lib/queries.ts` |
| `ProviderType` | `ui/src/app/(authenticated)/providers/page.tsx`, `ui/src/lib/queries.ts` |
| `TimeoutsConfig` | `ui/src/app/(authenticated)/providers/_parts/TimeoutsSection.tsx` |
| `ProviderOptions` | `ui/src/app/(authenticated)/providers/page.tsx` |
| `Provider` | `ui/src/app/(authenticated)/providers/page.tsx`, `ui/src/lib/queries.ts` |
| `CreateProviderInput` | `ui/src/app/(authenticated)/providers/page.tsx`, `ui/src/lib/queries.ts` |
| `ProviderActiveRow` | `ui/src/lib/queries.ts` |
| `MediaDriverInfo` | `ui/src/lib/queries.ts` |
| `NotificationRow` | `ui/src/stores/notification-store.ts` |
| `NotificationsResponse` | `ui/src/stores/notification-store.ts` |
| `TaskStep` | `ui/src/components/TaskPlanPanel.tsx` |
| `AgentTask` | `ui/src/components/TaskPlanPanel.tsx`, `ui/src/app/(authenticated)/tasks/page.tsx` |
