// @generated — do not edit by hand.
// Source of truth: crates/hydeclaw-core/src/gateway/handlers/agents/dto_structs.rs (Phase B),
//                  crates/hydeclaw-core/src/db/github.rs + approvals.rs (Phase C)
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
