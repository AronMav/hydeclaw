use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use crate::config::AgentConfig;

// ── Deserialization helpers ──────────────────────────────

/// Deserialize a field that distinguishes absent (preserve) from explicit null (clear).
/// absent → None (outer), explicit null → Some(None) (inner), value → Some(Some(T)).
pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

// ── Validation ──────────────────────────────────────────

pub(crate) fn agent_config_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new("config/agents").join(format!("{name}.toml"))
}

pub(crate) fn validate_agent_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 32 {
        return Err("name must be 1-32 characters".into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("name must contain only alphanumeric, dash, or underscore".into());
    }
    Ok(())
}

// ── Response builders ───────────────────────────────────

pub(crate) fn agent_to_detail(cfg: &AgentConfig, is_running: bool, config_dirty: bool) -> Value {
    let a = &cfg.agent;
    json!({
        "name": a.name,
        "language": a.language,
        "provider": a.provider,
        "model": a.model,
        "provider_connection": a.provider_connection,
        "fallback_provider": a.fallback_provider,
        "temperature": a.temperature,
        "max_tokens": a.max_tokens,
        "access": a.access.as_ref().map(|ac| json!({
            "mode": ac.mode,
            "owner_id": ac.owner_id,
        })),
        "heartbeat": a.heartbeat.as_ref().map(|h| json!({
            "cron": h.cron,
            "timezone": h.timezone,
            "announce_to": h.announce_to,
        })),
        "tools": a.tools.as_ref().map(|t| json!({
            "allow": t.allow,
            "deny": t.deny,
            "allow_all": t.allow_all,
            "deny_all_others": t.deny_all_others,
            "groups": {
                "git": t.groups.git,
                "tool_management": t.groups.tool_management,
                "skill_editing": t.groups.skill_editing,
                "session_tools": t.groups.session_tools,
            },
        })),
        "compaction": a.compaction.as_ref().map(|c| json!({
            "enabled": c.enabled,
            "threshold": c.threshold,
            "preserve_tool_calls": c.preserve_tool_calls,
            "preserve_last_n": c.preserve_last_n,
            "max_context_tokens": c.max_context_tokens,
        })),
        "session": a.session.as_ref().map(|s| json!({
            "dm_scope": s.dm_scope,
            "ttl_days": s.ttl_days,
            "max_messages": s.max_messages,
            "prune_tool_output_after_turns": s.prune_tool_output_after_turns,
        })),
        "icon": a.icon,
        "max_tools_in_context": a.max_tools_in_context,
        "tool_loop": a.tool_loop.as_ref().map(|tl| json!({
            "max_iterations": tl.max_iterations,
            "compact_on_overflow": tl.compact_on_overflow,
            "detect_loops": tl.detect_loops,
            "warn_threshold": tl.warn_threshold,
            "break_threshold": tl.break_threshold,
            "max_consecutive_failures": tl.max_consecutive_failures,
            "max_auto_continues": tl.max_auto_continues,
            "max_loop_nudges": tl.max_loop_nudges,
            "ngram_cycle_length": tl.ngram_cycle_length,
        })),
        "approval": a.approval.as_ref().map(|ap| json!({
            "enabled": ap.enabled,
            "require_for": ap.require_for,
            "require_for_categories": ap.require_for_categories,
            "timeout_seconds": ap.timeout_seconds,
        })),
        "routing": a.routing.iter().map(|r| json!({
            "condition": r.condition,
            "connection": r.connection,
            "model": r.model,
            "temperature": r.temperature,
            "cooldown_secs": r.cooldown_secs,
        })).collect::<Vec<_>>(),
        "watchdog": a.watchdog.as_ref().map(|w| json!({
            "inactivity_secs": w.inactivity_secs,
        })),
        "hooks": a.hooks.as_ref().map(|h| json!({
            "log_all_tool_calls": h.log_all_tool_calls,
            "block_tools": h.block_tools,
        })),
        "max_history_messages": a.max_history_messages,
        "daily_budget_tokens": a.daily_budget_tokens,
        "max_agent_turns": a.max_agent_turns,
        "is_running": is_running,
        "config_dirty": config_dirty,
    })
}

// ── Payload types ───────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct AgentCreatePayload {
    pub name: String,
    pub language: Option<String>,
    pub provider: String,
    pub model: String,
    /// Named LLM provider connection (overrides provider/model when set).
    pub provider_connection: Option<String>,
    /// Optional fallback provider connection name for consecutive-failure switching.
    pub fallback_provider: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    /// Nullable fields: absent = preserve existing, explicit null = clear, value = update.
    #[serde(default, deserialize_with = "nullable")]
    pub access: Option<Option<AccessPayload>>,
    #[serde(default, deserialize_with = "nullable")]
    pub heartbeat: Option<Option<HeartbeatPayload>>,
    #[serde(default, deserialize_with = "nullable")]
    pub tools: Option<Option<ToolPolicyPayload>>,
    #[serde(default, deserialize_with = "nullable")]
    pub compaction: Option<Option<CompactionPayload>>,
    #[serde(default, deserialize_with = "nullable")]
    pub session: Option<Option<SessionPayload>>,
    pub max_tools_in_context: Option<usize>,
    #[serde(default, deserialize_with = "nullable")]
    pub routing: Option<Option<Vec<RoutingRulePayload>>>,
    pub voice: Option<String>,
    pub icon: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub approval: Option<Option<ApprovalPayload>>,
    #[serde(default, deserialize_with = "nullable")]
    pub tool_loop: Option<Option<ToolLoopPayload>>,
    #[serde(default, deserialize_with = "nullable")]
    pub watchdog: Option<Option<WatchdogPayload>>,
    #[serde(default, deserialize_with = "nullable")]
    pub hooks: Option<Option<HooksPayload>>,
    pub max_history_messages: Option<usize>,
    pub daily_budget_tokens: Option<u64>,
    pub max_agent_turns: Option<usize>,
}

#[derive(Deserialize)]
pub(crate) struct HooksPayload {
    pub log_all_tool_calls: Option<bool>,
    pub block_tools: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub(crate) struct ApprovalPayload {
    pub enabled: Option<bool>,
    pub require_for: Option<Vec<String>>,
    pub require_for_categories: Option<Vec<String>>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct RoutingRulePayload {
    pub condition: Option<String>,
    pub connection: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f64>,
    pub cooldown_secs: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct AccessPayload {
    pub mode: Option<String>,
    pub owner_id: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct HeartbeatPayload {
    pub cron: String,
    pub timezone: Option<String>,
    pub announce_to: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ToolPolicyPayload {
    pub allow: Option<Vec<String>>,
    pub deny: Option<Vec<String>>,
    pub allow_all: Option<bool>,
    pub deny_all_others: Option<bool>,
    pub groups: Option<crate::config::ToolGroups>,
}

#[derive(Deserialize)]
pub(crate) struct CompactionPayload {
    pub enabled: Option<bool>,
    pub threshold: Option<f64>,
    pub preserve_tool_calls: Option<bool>,
    pub preserve_last_n: Option<u32>,
    pub max_context_tokens: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct SessionPayload {
    pub dm_scope: Option<String>,
    pub ttl_days: Option<u32>,
    pub max_messages: Option<u32>,
    pub prune_tool_output_after_turns: Option<usize>,
}

#[derive(Deserialize)]
pub(crate) struct ToolLoopPayload {
    pub max_iterations: Option<usize>,
    pub compact_on_overflow: Option<bool>,
    pub detect_loops: Option<bool>,
    pub warn_threshold: Option<usize>,
    pub break_threshold: Option<usize>,
    pub max_consecutive_failures: Option<usize>,
    pub max_auto_continues: Option<u8>,
    pub max_loop_nudges: Option<usize>,
    pub ngram_cycle_length: Option<usize>,
    pub error_break_threshold: Option<usize>,
}

#[derive(Deserialize)]
pub(crate) struct WatchdogPayload {
    pub inactivity_secs: Option<u64>,
}

// ── Config builder ──────────────────────────────────────

pub(crate) fn build_agent_config(name: String, p: AgentCreatePayload) -> AgentConfig {
    use crate::config::{AgentConfig, AgentSettings, AgentAccessConfig, HeartbeatConfig, AgentToolPolicy, CompactionConfig};

    AgentConfig {
        agent: AgentSettings {
            name,
            language: p.language.unwrap_or_else(|| "ru".to_string()),
            provider: p.provider,
            model: p.model,
            provider_connection: p.provider_connection,
            fallback_provider: p.fallback_provider,
            temperature: p.temperature.unwrap_or(1.0),
            max_tokens: p.max_tokens,
            access: p.access.flatten().map(|a| AgentAccessConfig {
                mode: a.mode.unwrap_or_else(|| "open".to_string()),
                owner_id: a.owner_id,
            }),
            heartbeat: p.heartbeat.flatten().map(|h| HeartbeatConfig {
                cron: h.cron,
                timezone: h.timezone,
                announce_to: h.announce_to,
            }),
            tools: p.tools.flatten().map(|t| AgentToolPolicy {
                allow: t.allow.unwrap_or_default(),
                deny: t.deny.unwrap_or_default(),
                allow_all: t.allow_all.unwrap_or(false),
                deny_all_others: t.deny_all_others.unwrap_or(false),
                groups: t.groups.unwrap_or_default(),
            }),
            compaction: p.compaction.flatten().map(|c| CompactionConfig {
                enabled: c.enabled.unwrap_or(true),
                threshold: c.threshold.unwrap_or(0.8),
                preserve_tool_calls: c.preserve_tool_calls.unwrap_or(false),
                preserve_last_n: c.preserve_last_n.unwrap_or(10),
                max_context_tokens: c.max_context_tokens,
            }),
            icon: p.icon,
            max_tools_in_context: p.max_tools_in_context,
            routing: p.routing.flatten().unwrap_or_default().into_iter().map(|r| {
                crate::config::ProviderRouteConfig {
                    condition: r.condition.unwrap_or_else(|| "default".to_string()),
                    connection: r.connection,
                    model: r.model,
                    temperature: r.temperature,
                    cooldown_secs: r.cooldown_secs.unwrap_or(60),
                }
            }).collect(),
            session: p.session.flatten().map(|s| crate::config::SessionConfig {
                dm_scope: s.dm_scope.unwrap_or_else(|| "per-channel-peer".to_string()),
                ttl_days: s.ttl_days.unwrap_or(30),
                max_messages: s.max_messages.unwrap_or(0),
                prune_tool_output_after_turns: s.prune_tool_output_after_turns,
            }),
            approval: p.approval.flatten().map(|a| crate::config::ApprovalConfig {
                enabled: a.enabled.unwrap_or(false),
                require_for: a.require_for.unwrap_or_default(),
                require_for_categories: a.require_for_categories.unwrap_or_default(),
                timeout_seconds: a.timeout_seconds.unwrap_or(300),
            }),
            tool_loop: p.tool_loop.flatten().map(|tl| crate::config::ToolLoopSettings {
                max_iterations: tl.max_iterations.unwrap_or(50),
                compact_on_overflow: tl.compact_on_overflow.unwrap_or(true),
                detect_loops: tl.detect_loops.unwrap_or(true),
                warn_threshold: tl.warn_threshold.unwrap_or(5),
                break_threshold: tl.break_threshold.unwrap_or(10),
                max_consecutive_failures: tl.max_consecutive_failures.unwrap_or(3),
                max_auto_continues: tl.max_auto_continues.unwrap_or(5),
                max_loop_nudges: tl.max_loop_nudges.unwrap_or(3),
                ngram_cycle_length: tl.ngram_cycle_length.unwrap_or(6),
                error_break_threshold: tl.error_break_threshold,
            }),
            watchdog: p.watchdog.flatten().map(|w| crate::config::WatchdogConfig {
                inactivity_secs: w.inactivity_secs.unwrap_or(600),
            }),
            max_history_messages: p.max_history_messages,
            hooks: p.hooks.flatten().map(|h| crate::config::HooksConfig {
                log_all_tool_calls: h.log_all_tool_calls.unwrap_or(false),
                block_tools: h.block_tools.unwrap_or_default(),
            }),
            daily_budget_tokens: p.daily_budget_tokens.unwrap_or(0),
            max_agent_turns: p.max_agent_turns,
            base: false,
        },
    }
}
