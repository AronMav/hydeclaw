

export interface StatusInfo {
  status: string;
  version: string;
  uptime_seconds: number;
  db: boolean;
  listen: string;
  agents: string[];
  memory_chunks: number;
  scheduled_jobs: number;
  active_sessions: number;
  tools_registered: number;
}

export interface StatsInfo {
  messages_today: number;
  sessions_today: number;
  total_messages: number;
  total_sessions: number;
  recent_sessions?: { id: string; agent_id: string; channel: string; last_message_at: string; title: string | null }[];
}

export interface AgentInfo {
  name: string;
  language: string;
  model: string;
  provider: string;
  icon: string | null;
  temperature: number;
  has_access: boolean;
  access_mode: string | null;
  has_heartbeat: boolean;
  heartbeat_cron: string | null;
  heartbeat_timezone: string | null;
  tool_policy: { allow: string[]; deny: string[]; allow_all: boolean } | null;
  routing_count: number;
  is_running: boolean;
  config_dirty: boolean;
  pending_delete?: boolean;
  base?: boolean;
  provider_connection: string | null;
  fallback_provider?: string | null;
}

export interface RoutingRule {
  provider: string;
  model: string;
  condition: string;
  base_url?: string | null;
  api_key_env?: string | null;
  api_key_envs?: string[];
  temperature?: number | null;
  max_tokens?: number | null;
  prompt_cache?: boolean;
  cooldown_secs?: number;
}

export interface AgentDetail {
  name: string;
  language: string;
  provider: string;
  model: string;
  icon: string | null;
  temperature: number;
  max_tokens: number | null;
  access: { mode: string; owner_id: string | null } | null;
  heartbeat: { cron: string; timezone: string | null; announce_to: string | null } | null;
  tools: {
    allow: string[]; deny: string[]; allow_all: boolean; deny_all_others: boolean;
    groups: { git: boolean; tool_management: boolean; skill_editing: boolean; session_tools: boolean };
  } | null;
  compaction: {
    enabled: boolean; threshold: number; preserve_tool_calls: boolean;
    preserve_last_n: number; max_context_tokens: number | null;
  } | null;
  session: {
    dm_scope: string; ttl_days: number; max_messages: number;
    prune_tool_output_after_turns: number | null;
  } | null;
  max_tools_in_context: number | null;
  tool_loop: {
    max_iterations: number; compact_on_overflow: boolean; detect_loops: boolean;
    warn_threshold: number; break_threshold: number; max_consecutive_failures?: number;
    max_auto_continues?: number;
  } | null;
  routing: RoutingRule[];
  voice?: string;
  approval?: {
    enabled: boolean; require_for: string[]; require_for_categories: string[];
    timeout_seconds: number;
  } | null;
  watchdog?: { inactivity_secs: number } | null;
  hooks?: { log_all_tool_calls: boolean; block_tools: string[] } | null;
  max_history_messages?: number | null;
  daily_budget_tokens?: number;
  is_running: boolean;
  config_dirty: boolean;
  provider_connection: string | null;
  fallback_provider?: string | null;
}

export interface SessionRow {
  id: string;
  agent_id: string;
  user_id: string;
  channel: string;
  started_at: string;
  last_message_at: string;
  title: string | null;
  run_status: string | null;
  metadata: Record<string, unknown> | null;
  participants?: string[];
}

export interface MessageRow {
  id: string;
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  tool_calls: unknown;
  tool_call_id: string | null;
  created_at: string;
  agent_id?: string | null;
  status: string;
  feedback: number;
  edited_at: string | null;
  parent_message_id: string | null;
  branch_from_message_id: string | null;
}

export interface CronJob {
  id: string;
  name: string;
  agent: string;
  cron: string;
  timezone: string;
  task: string;
  enabled: boolean;
  silent: boolean;
  announce_to?: { channel: string; chat_id: number; channel_id?: string } | null;
  jitter_secs: number;
  run_once: boolean;
  run_at: string | null;
  created_at: string;
  last_run: string | null;
  next_run: string | null;
  tool_policy?: { allow: string[]; deny: string[] } | null;
}

export interface CronRun {
  id: string;
  job_id: string;
  job_name?: string;
  agent_id: string;
  started_at: string;
  finished_at: string | null;
  status: "running" | "success" | "error";
  error: string | null;
  response_preview: string | null;
}

export interface MemoryDocument {
  id: string;
  source: string | null;
  pinned: boolean;
  relevance_score: number;
  similarity?: number;
  created_at?: string;
  accessed_at?: string;
  preview: string | null;
  chunks_count: number;
  total_chars: number | null;
  category: string | null;
  topic: string | null;
}

export interface MemoryStats {
  total: number;
  total_chunks: number;
  pinned: number;
  avg_score: number;
}

export interface ToolEntry {
  name: string;
  url: string;
  tool_type: string;
  healthy: boolean;
  concurrency_limit: number | null;
  healthcheck?: string | null;
  depends_on?: string[];
  ui_path?: string | null;
  managed?: boolean;
}

export interface SkillEntry {
  name: string;
  description: string;
  triggers: string[];
  tools_required: string[];
  priority: number;
  instructions_len: number;
}

export interface YamlToolEntry {
  name: string;
  description: string;
  endpoint: string;
  method: string;
  status: "verified" | "draft" | "disabled";
  parameters_count: number;
  tags: string[];
}

export interface McpEntry {
  name: string;
  url: string | null;
  container: string | null;
  port: number | null;
  mode: string;
  idle_timeout?: string;
  protocol: string;
  enabled: boolean;
  status: string | null;
  tool_count: number | null;
}

export interface FileEntry {
  name: string;
  is_dir: boolean;
  display: string;
}

export interface SecretInfo {
  name: string;
  scope: string;
  description: string | null;
  has_value: boolean;
  created_at: string;
  updated_at: string;
}

export interface GitHubRepoInfo {
  id: string;
  agent_id: string;
  owner: string;
  repo: string;
  added_at: string;
}

export interface OAuthAccount {
  id: string;
  provider: string;
  display_name: string;
  user_email: string | null;
  scope: string;
  status: string;
  expires_at: string | null;
  connected_at: string | null;
  created_at: string;
}

export interface OAuthBinding {
  agent_id: string;
  provider: string;
  account_id: string;
  display_name: string;
  user_email: string | null;
  status: string;
  expires_at: string | null;
  connected_at: string | null;
  bound_at: string;
}

export interface LogEntry {
  level: string;
  message: string;
  target?: string;
  timestamp: string;
}

export interface UsageSummary {
  agent_id: string;
  provider: string;
  model: string;
  total_input: number;
  total_output: number;
  call_count: number;
  estimated_cost: number | null;
}

export interface UsageResponse {
  ok: boolean;
  days: number;
  usage: UsageSummary[];
}

export interface DailyUsageEntry {
  date: string;
  agent_id: string;
  provider: string;
  model: string;
  input_tokens: number;
  output_tokens: number;
  call_count: number;
}

export interface DailyUsageResponse {
  ok: boolean;
  days: number;
  daily: DailyUsageEntry[];
}



export interface AuditEvent {
  id: string;
  agent_id: string;
  event_type: string;
  actor: string | null;
  details: Record<string, unknown>;
  created_at: string;
}

export interface ChannelRow {
  id: string;
  agent_name: string;
  channel_type: string;
  display_name: string;
  config: Record<string, unknown>;
  status: string;
  error_msg: string | null;
}

export interface ActiveChannel {
  agent_name: string;
  channel_id: string | null;
  channel_type: string;
  display_name: string;
  adapter_version: string;
  connected_at: string;
  last_activity: string;
}

export interface BackupEntry {
  filename: string;
  size_bytes: number;
  created_at: string;
}

export interface WebhookEntry {
  id: string;
  name: string;
  agent_id: string;
  secret: string | null;
  prompt_prefix: string | null;
  enabled: boolean;
  created_at: string;
  last_triggered_at: string | null;
  trigger_count: number;
  webhook_type: "generic" | "github";
  event_filter: string[] | null;
}

export interface ApprovalEntry {
  id: string;
  agent_id: string;
  tool: string;
  arguments: Record<string, unknown>;
  status: "pending" | "approved" | "rejected";
  created_at: string;
  resolved_at: string | null;
  resolved_by: string | null;
}

export interface ProviderType {
  id: string;
  name: string;
  default_base_url: string;
  chat_path: string;
  default_secret_name: string;
  requires_api_key: boolean;
  supports_model_listing: boolean;
}

export interface Provider {
  id: string;
  name: string;
  type: string;
  provider_type: string;
  base_url: string | null;
  default_model: string | null;
  has_api_key: boolean;
  api_key: string | null;
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

export interface MediaDriverInfo {
  driver: string;
  label: string;
  requires_key: boolean;
}

export interface NotificationRow {
  id: string;
  type: string;
  title: string;
  body: string;
  data: Record<string, unknown> | null;
  read: boolean;
  created_at: string;
}

export interface NotificationsResponse {
  notifications?: NotificationRow[];
  items?: NotificationRow[];
  unread_count: number;
}

export interface TaskStep {
  id: string;
  title: string;
  status: "pending" | "in_progress" | "done" | "error";
  started_at: string | null;
  finished_at: string | null;
  error: string | null;
}

export interface AgentTask {
  task_id: string;
  agent: string;
  title: string;
  status: "planning" | "in_progress" | "done" | "error";
  created_at: string;
  updated_at: string;
  steps: TaskStep[];
}
