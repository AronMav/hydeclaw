

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

// AgentInfo is now generated from Rust DTO via ts-rs codegen.
// Source: crates/hydeclaw-core/src/gateway/handlers/agents/dto_structs.rs
// Regenerate: make gen-types
export type { AgentInfoDto as AgentInfo } from "./api.generated";

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

// AgentDetail is now generated from Rust DTOs via ts-rs codegen.
// Source: crates/hydeclaw-core/src/gateway/handlers/agents/dto.rs
// Regenerate: make gen-types
export type { AgentDetailDto as AgentDetail } from "./api.generated";

// SessionRow is now generated from Rust DTO via ts-rs codegen.
// Source: crates/hydeclaw-core/src/db/sessions.rs
// Regenerate: make gen-types
export type { Session as SessionRow } from "./api.generated";

// MessageRow is now generated from Rust DTO via ts-rs codegen.
// Note: feedback is number | null (DB-accurate); the old type had number (incorrect).
// Source: crates/hydeclaw-core/src/db/sessions.rs
// Regenerate: make gen-types
export type { MessageRow } from "./api.generated";

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
  scope?: string | null;
}

export interface MemoryStats {
  total: number;
  total_chunks: number;
  pinned: number;
  avg_score: number;
  embed_model?: string | null;
  embed_dim?: number | null;
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

export type { GitHubRepo as GitHubRepoInfo } from "./api.generated";

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

// ChannelRow is now generated from Rust DTO via ts-rs codegen.
// Source: crates/hydeclaw-core/src/gateway/handlers/channels_dto_structs.rs
// Regenerate: make gen-types
export type { ChannelRowDto as ChannelRow } from "./api.generated";

// ActiveChannel is now generated from Rust DTO via ts-rs codegen.
// Source: crates/hydeclaw-core/src/gateway/handlers/channels_dto_structs.rs
// Regenerate: make gen-types
export type { ActiveChannelDto as ActiveChannel } from "./api.generated";

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

export type TimeoutsConfig = {
  connect_secs: number;              // 1..=120
  request_secs: number;              // 0..=3600, 0 = no limit
  stream_inactivity_secs: number;    // 0..=3600, 0 = no limit
  stream_max_duration_secs: number;  // 0..=7200, 0 = no limit
};

export type ProviderOptions = {
  timeouts?: Partial<TimeoutsConfig>;
  api_key_envs?: string[];
  // Unknown fields land here — UI will preserve them on round-trip.
  [extra: string]: unknown;
};

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
  options: ProviderOptions;
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
  options?: ProviderOptions;
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

// NotificationRow is now generated from Rust DTO via ts-rs codegen.
// Source: crates/hydeclaw-core/src/db/notifications.rs
// Regenerate: make gen-types
export type { Notification as NotificationRow } from "./api.generated";

// NotificationsResponse is now generated. `items` is required (not optional);
// the stale `notifications?` key has been removed.
// Regenerate: make gen-types
export type { NotificationsResponseDto as NotificationsResponse } from "./api.generated";

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
