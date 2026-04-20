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
| 102 | memory.rs | GET /api/memory | api_list_memory | hand-rolled | — | TBD | TBD |
| 103 | memory.rs | POST /api/memory | api_create_memory | hand-rolled | — | TBD | TBD |
| 104 | memory.rs | GET /api/memory/stats | api_memory_stats | hand-rolled | — | TBD | TBD |
| 105 | memory.rs | GET /api/memory/export | api_export_memory | hand-rolled | — | TBD | TBD |
| 106 | memory.rs | GET /api/memory/fts-language | api_get_fts_language | hand-rolled | — | TBD | TBD |
| 107 | memory.rs | PUT /api/memory/fts-language | api_set_fts_language | hand-rolled | — | TBD | TBD |
| 108 | memory.rs | GET /api/memory/tasks | api_memory_tasks | hand-rolled | — | TBD | TBD |
| 109 | memory.rs | GET /api/memory/documents | api_list_documents | hand-rolled | — | TBD | TBD |
| 110 | memory.rs | GET /api/memory/documents/{id} | api_get_document | hand-rolled | — | TBD | TBD |
| 111 | memory.rs | PATCH /api/memory/documents/{id} | api_patch_document | hand-rolled | — | TBD | TBD |
| 112 | memory.rs | DELETE /api/memory/{id} | api_delete_memory | hand-rolled | — | TBD | TBD |
| 113 | memory.rs | PATCH /api/memory/{id} | api_patch_memory | hand-rolled | — | TBD | TBD |
| 114 | backup.rs | POST /api/backup | api_create_backup | hand-rolled | — | TBD | TBD |
| 115 | backup.rs | GET /api/backup | api_list_backups | hand-rolled | — | TBD | TBD |
| 116 | backup.rs | GET /api/backup/{filename} | api_download_backup | none (raw bytes) | — | N/A | N/A |
| 117 | backup.rs | DELETE /api/backup/{filename} | api_delete_backup | hand-rolled | — | TBD | TBD |
| 118 | backup.rs | POST /api/restore | api_restore | mixed | BackupFile (backup.rs:49) response is hand-rolled json!{} | TBD | TBD |
| 119 | monitoring.rs | GET /api/setup/status | api_setup_status | hand-rolled | — | TBD | TBD |
| 120 | monitoring.rs | GET /api/setup/requirements | api_setup_requirements | mixed | CheckResult (monitoring.rs:149) wrapped in json!{} | TBD | TBD |
| 121 | monitoring.rs | POST /api/setup/complete | api_setup_complete | hand-rolled | — | TBD | TBD |
| 122 | monitoring.rs | GET /api/status | api_status | hand-rolled | — | TBD | TBD |
| 123 | monitoring.rs | GET /api/stats | api_stats | hand-rolled | — | TBD | TBD |
| 124 | monitoring.rs | GET /api/usage | api_usage | mixed | UsageSummary (db/usage.rs:90) wrapped in json!{} | TBD | TBD |
| 125 | monitoring.rs | GET /api/usage/daily | api_usage_daily | mixed | DailyUsage (db/usage.rs:132) wrapped in json!{} | TBD | TBD |
| 126 | monitoring.rs | GET /api/usage/sessions | api_usage_sessions | mixed | SessionUsage (db/usage.rs:177) wrapped in json!{} | TBD | TBD |
| 127 | monitoring.rs | GET /api/doctor | api_doctor | mixed | CheckResult (monitoring.rs:149) wrapped in json!{} | TBD | TBD |
| 128 | monitoring.rs | GET /api/health/dashboard | api_health_dashboard | hand-rolled | — | TBD | TBD |
| 129 | monitoring.rs | GET /api/audit | api_audit_events | mixed | AuditEvent (db/audit.rs:50) wrapped in json!{} | TBD | TBD |
| 130 | monitoring.rs | GET /api/audit/tools | api_tool_audit | hand-rolled | — | TBD | TBD |
| 131 | monitoring.rs | GET /api/watchdog/status | api_watchdog_status | hand-rolled | — | TBD | TBD |
| 132 | monitoring.rs | GET /api/watchdog/config | api_watchdog_config | hand-rolled | — | TBD | TBD |
| 133 | monitoring.rs | PUT /api/watchdog/config | api_watchdog_config_update | hand-rolled | — | TBD | TBD |
| 134 | monitoring.rs | GET /api/watchdog/settings | api_watchdog_settings | hand-rolled | — | TBD | TBD |
| 135 | monitoring.rs | PUT /api/watchdog/settings | api_watchdog_settings_update | hand-rolled | — | TBD | TBD |
| 136 | monitoring.rs | POST /api/watchdog/restart/{name} | api_watchdog_restart_check | hand-rolled | — | TBD | TBD |

| 137 | access.rs | GET /api/access/{agent}/pending | api_access_pending | hand-rolled | — | TBD | TBD |
| 138 | access.rs | POST /api/access/{agent}/approve/{code} | api_access_approve | hand-rolled | — | TBD | TBD |
| 139 | access.rs | POST /api/access/{agent}/reject/{code} | api_access_reject | hand-rolled | — | TBD | TBD |
| 140 | access.rs | GET /api/access/{agent}/users | api_access_list_users | hand-rolled | — | TBD | TBD |
| 141 | access.rs | DELETE /api/access/{agent}/users/{user_id} | api_access_remove_user | hand-rolled | — | TBD | TBD |
| 142 | channel_ws.rs | GET /ws | ws_handler | WebSocket — out of scope | — | N/A | N/A |
| 143 | channel_ws.rs | GET /ws/channel/{agent_name} | channel_ws_handler | WebSocket — out of scope | — | N/A | N/A |
| 144 | csp.rs | POST /api/csp-report | api_csp_report | none (204/400 only) | — | N/A | N/A |
| 145 | github_repos.rs | GET /api/agents/{name}/github/repos | api_list_github_repos | hand-rolled | — | TBD | TBD |
| 146 | github_repos.rs | POST /api/agents/{name}/github/repos | api_add_github_repo | typed | GitHubRepo (db/github.rs:18) | TBD | TBD |
| 147 | github_repos.rs | DELETE /api/agents/{name}/github/repos/{id} | api_delete_github_repo | none (StatusCode only) | — | N/A | N/A |
| 148 | media.rs | POST /api/media/upload | api_media_upload | hand-rolled | — | TBD | TBD |
| 149 | media.rs | GET /uploads/{filename} | api_media_serve | none (raw bytes) | — | N/A | N/A |
| 150 | oauth.rs | GET /api/oauth/callback | api_oauth_callback | none (Redirect) | — | N/A | N/A |
| 151 | oauth.rs | GET /api/oauth/accounts | api_oauth_accounts_list | hand-rolled | — | TBD | TBD |
| 152 | oauth.rs | POST /api/oauth/accounts | api_oauth_account_create | hand-rolled | — | TBD | TBD |
| 153 | oauth.rs | DELETE /api/oauth/accounts/{id} | api_oauth_account_delete | none (StatusCode only) | — | N/A | N/A |
| 154 | oauth.rs | POST /api/oauth/accounts/{id}/connect | api_oauth_account_connect | hand-rolled | — | TBD | TBD |
| 155 | oauth.rs | POST /api/oauth/accounts/{id}/revoke | api_oauth_account_revoke | hand-rolled | — | TBD | TBD |
| 156 | oauth.rs | GET /api/oauth/providers | api_oauth_providers | hand-rolled | — | TBD | TBD |
| 157 | oauth.rs | GET /api/agents/{name}/oauth/bindings | api_oauth_bindings_list | hand-rolled | — | TBD | TBD |
| 158 | oauth.rs | POST /api/agents/{name}/oauth/bindings | api_oauth_binding_create | hand-rolled | — | TBD | TBD |
| 159 | oauth.rs | DELETE /api/agents/{name}/oauth/bindings/{provider} | api_oauth_binding_delete | hand-rolled | — | TBD | TBD |
| 160 | skills.rs | GET /api/skills | api_skills_list_global | hand-rolled | — | TBD | TBD |
| 161 | skills.rs | GET /api/skills/{skill} | api_skill_get_global | hand-rolled | — | TBD | TBD |
| 162 | skills.rs | PUT /api/skills/{skill} | api_skill_upsert_global | hand-rolled | — | TBD | TBD |
| 163 | skills.rs | DELETE /api/skills/{skill} | api_skill_delete_global | hand-rolled | — | TBD | TBD |
| 164 | skills.rs | GET /api/agents/{name}/skills | api_skills_list | hand-rolled | — | TBD | TBD |
| 165 | skills.rs | GET /api/agents/{name}/skills/{skill} | api_skill_get | hand-rolled | — | TBD | TBD |
| 166 | skills.rs | PUT /api/agents/{name}/skills/{skill} | api_skill_upsert | hand-rolled | — | TBD | TBD |
| 167 | skills.rs | DELETE /api/agents/{name}/skills/{skill} | api_skill_delete | hand-rolled | — | TBD | TBD |
| 168 | tasks.rs | GET /api/tasks | api_list_tasks | hand-rolled | TaskRow (tasks/mod.rs:7) wrapped in json!{} | TBD | TBD |
| 169 | tasks.rs | POST /api/tasks | api_create_task_endpoint | hand-rolled | — | TBD | TBD |
| 170 | tasks.rs | GET /api/tasks/audit | api_task_audit | mixed | ToolAuditEntry (db/tool_audit.rs) wrapped in json!{} | TBD | TBD |
| 171 | tasks.rs | GET /api/tasks/{id} | api_get_task | mixed | TaskRow (tasks/mod.rs:7) via json!(task) | TBD | TBD |
| 172 | tasks.rs | DELETE /api/tasks/{id} | api_delete_task | hand-rolled | — | TBD | TBD |
| 173 | tasks.rs | GET /api/tasks/{id}/steps | api_task_steps | hand-rolled | TaskStepRow fields inlined via json!{} | TBD | TBD |
| 174 | tools.rs | GET /api/tool-definitions | api_tool_definitions | hand-rolled | — | TBD | TBD |
| 175 | tools.rs | GET /api/tools | api_list_tools | hand-rolled | — | TBD | TBD |
| 176 | tools.rs | POST /api/tools | api_tool_service_create | hand-rolled | — | TBD | TBD |
| 177 | tools.rs | PUT /api/tools/{name} | api_tool_service_update | hand-rolled | — | TBD | TBD |
| 178 | tools.rs | DELETE /api/tools/{name} | api_tool_service_delete | hand-rolled | — | TBD | TBD |
| 179 | tools.rs | GET /api/mcp | api_list_mcp | hand-rolled | — | TBD | TBD |
| 180 | tools.rs | POST /api/mcp | api_mcp_create | hand-rolled | — | TBD | TBD |
| 181 | tools.rs | PUT /api/mcp/{name} | api_mcp_update | hand-rolled | — | TBD | TBD |
| 182 | tools.rs | DELETE /api/mcp/{name} | api_mcp_delete | hand-rolled | — | TBD | TBD |
| 183 | tools.rs | POST /api/mcp/{name}/reload | api_mcp_reload | hand-rolled | — | TBD | TBD |
| 184 | tools.rs | POST /api/mcp/{name}/toggle | api_mcp_toggle | hand-rolled | — | TBD | TBD |
| 185 | workspace.rs | GET /api/workspace | api_workspace_browse | hand-rolled | — | TBD | TBD |
| 186 | workspace.rs | GET /api/workspace/{*path} | api_workspace_browse | hand-rolled | — | TBD | TBD |
| 187 | workspace.rs | PUT /api/workspace/{*path} | api_workspace_write | hand-rolled | — | TBD | TBD |
| 188 | workspace.rs | DELETE /api/workspace/{*path} | api_workspace_delete | hand-rolled | — | TBD | TBD |
| 189 | yaml_tools.rs | GET /api/yaml-tools | api_yaml_tools_list_global | hand-rolled | — | TBD | TBD |
| 190 | yaml_tools.rs | POST /api/yaml-tools | api_yaml_tool_create_global | hand-rolled | — | TBD | TBD |
| 191 | yaml_tools.rs | POST /api/yaml-tools/{tool}/verify | api_yaml_tool_verify_global | hand-rolled | — | TBD | TBD |
| 192 | yaml_tools.rs | POST /api/yaml-tools/{tool}/disable | api_yaml_tool_disable_global | hand-rolled | — | TBD | TBD |
| 193 | yaml_tools.rs | POST /api/yaml-tools/{tool}/enable | api_yaml_tool_enable_global | hand-rolled | — | TBD | TBD |
| 194 | yaml_tools.rs | GET /api/yaml-tools/{tool} | api_yaml_tool_get_global | hand-rolled | — | TBD | TBD |
| 195 | yaml_tools.rs | PUT /api/yaml-tools/{tool} | api_yaml_tool_update_global | hand-rolled | — | TBD | TBD |
| 196 | yaml_tools.rs | DELETE /api/yaml-tools/{tool} | api_yaml_tool_delete_global | hand-rolled | — | TBD | TBD |
| 197 | yaml_tools.rs | GET /api/agents/{name}/yaml-tools | api_yaml_tools_list | hand-rolled | — | TBD | TBD |
| 198 | yaml_tools.rs | POST /api/agents/{name}/yaml-tools/{tool}/verify | api_yaml_tool_verify | hand-rolled | — | TBD | TBD |
| 199 | yaml_tools.rs | POST /api/agents/{name}/yaml-tools/{tool}/disable | api_yaml_tool_disable | hand-rolled | — | TBD | TBD |

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
