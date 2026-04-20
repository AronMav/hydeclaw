// @generated — do not edit by hand.
// Source of truth: crates/hydeclaw-core/src/gateway/handlers/agents/dto_structs.rs (Phase B),
//                  crates/hydeclaw-core/src/db/github.rs + approvals.rs (Phase C),
//                  crates/hydeclaw-core/src/db/notifications.rs + sessions.rs (Phase A W1)
//                  crates/hydeclaw-core/src/gateway/handlers/channels_dto_structs.rs (Phase A W2)
// Regenerate with: make gen-types

export type AgentDetailAccessDto = { mode: string, owner_id: string | null, };

export type AgentDetailHeartbeatDto = { cron: string, timezone: string | null, announce_to: string | null, };

export type AgentDetailToolGroupsDto = { git: boolean, tool_management: boolean, skill_editing: boolean, session_tools: boolean, };

export type AgentDetailToolsDto = { allow: Array<string>, deny: Array<string>, allow_all: boolean, deny_all_others: boolean, groups: AgentDetailToolGroupsDto, };

export type AgentDetailCompactionDto = { enabled: boolean, threshold: number, preserve_tool_calls: boolean, preserve_last_n: number, max_context_tokens: number | null, };

export type AgentDetailSessionDto = { dm_scope: string, ttl_days: number, max_messages: number, prune_tool_output_after_turns: number | null, };

export type AgentDetailToolLoopDto = { max_iterations: number, compact_on_overflow: boolean, detect_loops: boolean, warn_threshold: number, break_threshold: number, max_consecutive_failures: number, max_auto_continues: number, max_loop_nudges: number, ngram_cycle_length: number, };

export type AgentDetailApprovalDto = { enabled: boolean, require_for: Array<string>, require_for_categories: Array<string>, timeout_seconds: number, };

export type AgentDetailRoutingDto = { condition: string, connection: string | null, model: string | null, temperature: number | null, cooldown_secs: number, };

export type AgentDetailWatchdogDto = { inactivity_secs: number, };

export type AgentDetailHooksDto = { log_all_tool_calls: boolean, block_tools: Array<string>, };

export type AgentDetailDto = { name: string, language: string, provider: string, model: string, provider_connection: string | null, fallback_provider: string | null, temperature: number, max_tokens: number | null, access: AgentDetailAccessDto | null, heartbeat: AgentDetailHeartbeatDto | null, tools: AgentDetailToolsDto | null, compaction: AgentDetailCompactionDto | null, session: AgentDetailSessionDto | null, icon: string | null, max_tools_in_context: number | null, tool_loop: AgentDetailToolLoopDto | null, approval: AgentDetailApprovalDto | null, routing: Array<AgentDetailRoutingDto>, watchdog: AgentDetailWatchdogDto | null, hooks: AgentDetailHooksDto | null, max_history_messages: number | null, daily_budget_tokens: number, max_agent_turns: number | null, max_failover_attempts: number, is_running: boolean, config_dirty: boolean, 
/**
 * Injected by the handler from scoped TTS_VOICE secret; absent when not set.
 */
voice?: string, };

export type GitHubRepo = { id: string, agent_id: string, owner: string, repo: string, added_at: string, };

export type AllowlistEntry = { id: string, agent_id: string, tool_pattern: string, created_at: string, created_by: string | null, };

export type AgentInfoToolPolicyDto = { allow: Array<string>, deny: Array<string>, allow_all: boolean, };

export type AgentInfoDto = { name: string, language: string, model: string, provider: string, provider_connection: string | null, fallback_provider: string | null, icon: string | null, temperature: number, has_access: boolean, access_mode: string | null, has_heartbeat: boolean, heartbeat_cron: string | null, heartbeat_timezone: string | null, tool_policy: AgentInfoToolPolicyDto | null, routing_count: number, is_running: boolean, config_dirty: boolean, base?: boolean, pending_delete?: boolean, };

export type Notification = { id: string, type: string, title: string, body: string, data: Record<string, unknown>, read: boolean, created_at: string, };

export type NotificationsResponseDto = { items: Array<Notification>, unread_count: number, limit: number, offset: number, };

export type Session = { id: string, agent_id: string, user_id: string, channel: string, started_at: string, last_message_at: string, title: string | null, metadata: Record<string, unknown> | null, run_status: string | null, participants: Array<string>, };

export type MessageRow = { id: string, role: string, content: string, tool_calls: unknown, tool_call_id: string | null, created_at: string, agent_id: string | null, feedback: number | null, edited_at: string | null, status: string, thinking_blocks: unknown, parent_message_id: string | null, branch_from_message_id: string | null, abort_reason: string | null, };

export type ChannelRowDto = { id: string, agent_name: string, channel_type: string, display_name: string, config: Record<string, unknown>, status: string, error_msg: string | null, };

export type ActiveChannelDto = { agent_name: string, channel_id: string | null, channel_type: string, display_name: string, adapter_version: string, connected_at: string, last_activity: string, };
