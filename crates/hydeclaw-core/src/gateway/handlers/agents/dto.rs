use serde::Serialize;

use crate::config::AgentConfig;

// ── Nested DTOs ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AgentDetailAccessDto {
    pub mode: Option<String>,
    pub owner_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailHeartbeatDto {
    pub cron: String,
    pub timezone: Option<String>,
    pub announce_to: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailToolGroupsDto {
    pub git: bool,
    pub tool_management: bool,
    pub skill_editing: bool,
    pub session_tools: bool,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailToolsDto {
    pub allow: Option<Vec<String>>,
    pub deny: Option<Vec<String>>,
    pub allow_all: Option<bool>,
    pub deny_all_others: Option<bool>,
    pub groups: AgentDetailToolGroupsDto,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailCompactionDto {
    pub enabled: bool,
    pub threshold: f64,
    pub preserve_tool_calls: bool,
    pub preserve_last_n: u32,
    pub max_context_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailSessionDto {
    pub dm_scope: String,
    pub ttl_days: u32,
    pub max_messages: u32,
    pub prune_tool_output_after_turns: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailToolLoopDto {
    pub max_iterations: usize,
    pub compact_on_overflow: bool,
    pub detect_loops: bool,
    pub warn_threshold: usize,
    pub break_threshold: usize,
    pub max_consecutive_failures: usize,
    pub max_auto_continues: u8,
    pub max_loop_nudges: usize,
    pub ngram_cycle_length: usize,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailApprovalDto {
    pub enabled: Option<bool>,
    pub require_for: Option<Vec<String>>,
    pub require_for_categories: Option<Vec<String>>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailRoutingDto {
    pub condition: String,
    pub connection: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f64>,
    pub cooldown_secs: u64,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailWatchdogDto {
    pub inactivity_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailHooksDto {
    pub log_all_tool_calls: Option<bool>,
    pub block_tools: Option<Vec<String>>,
}

// ── Top-level DTO ───────────────────────────────────────────────────────────

/// Response shape for GET /api/agents/{name}.
/// Field order matches the json!{} literal in schema.rs for diff readability.
/// No skip_serializing_if on Option fields (must emit null to match original shape).
#[derive(Debug, Serialize)]
pub struct AgentDetailDto {
    pub name: String,
    pub language: String,
    pub provider: String,
    pub model: String,
    pub provider_connection: Option<String>,
    pub fallback_provider: Option<String>,
    pub temperature: f64,
    pub max_tokens: Option<u32>,
    pub access: Option<AgentDetailAccessDto>,
    pub heartbeat: Option<AgentDetailHeartbeatDto>,
    pub tools: Option<AgentDetailToolsDto>,
    pub compaction: Option<AgentDetailCompactionDto>,
    pub session: Option<AgentDetailSessionDto>,
    pub icon: Option<String>,
    pub max_tools_in_context: Option<usize>,
    pub tool_loop: Option<AgentDetailToolLoopDto>,
    pub approval: Option<AgentDetailApprovalDto>,
    pub routing: Vec<AgentDetailRoutingDto>,
    pub watchdog: Option<AgentDetailWatchdogDto>,
    pub hooks: Option<AgentDetailHooksDto>,
    pub max_history_messages: Option<usize>,
    pub daily_budget_tokens: u64,
    pub max_agent_turns: Option<usize>,
    pub max_failover_attempts: u32,
    pub is_running: bool,
    pub config_dirty: bool,
    /// Injected by the handler from scoped TTS_VOICE secret; absent when not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

impl AgentDetailDto {
    pub fn from_config(
        cfg: &AgentConfig,
        is_running: bool,
        config_dirty: bool,
        voice: Option<String>,
    ) -> Self {
        let a = &cfg.agent;
        Self {
            name: a.name.clone(),
            language: a.language.clone(),
            provider: a.provider.clone(),
            model: a.model.clone(),
            provider_connection: a.provider_connection.clone(),
            fallback_provider: a.fallback_provider.clone(),
            temperature: a.temperature,
            max_tokens: a.max_tokens,
            access: a.access.as_ref().map(|ac| AgentDetailAccessDto {
                mode: Some(ac.mode.clone()),
                owner_id: ac.owner_id.clone(),
            }),
            heartbeat: a.heartbeat.as_ref().map(|h| AgentDetailHeartbeatDto {
                cron: h.cron.clone(),
                timezone: h.timezone.clone(),
                announce_to: h.announce_to.clone(),
            }),
            tools: a.tools.as_ref().map(|t| AgentDetailToolsDto {
                allow: Some(t.allow.clone()),
                deny: Some(t.deny.clone()),
                allow_all: Some(t.allow_all),
                deny_all_others: Some(t.deny_all_others),
                groups: AgentDetailToolGroupsDto {
                    git: t.groups.git,
                    tool_management: t.groups.tool_management,
                    skill_editing: t.groups.skill_editing,
                    session_tools: t.groups.session_tools,
                },
            }),
            compaction: a.compaction.as_ref().map(|c| AgentDetailCompactionDto {
                enabled: c.enabled,
                threshold: c.threshold,
                preserve_tool_calls: c.preserve_tool_calls,
                preserve_last_n: c.preserve_last_n,
                max_context_tokens: c.max_context_tokens,
            }),
            session: a.session.as_ref().map(|s| AgentDetailSessionDto {
                dm_scope: s.dm_scope.clone(),
                ttl_days: s.ttl_days,
                max_messages: s.max_messages,
                prune_tool_output_after_turns: s.prune_tool_output_after_turns,
            }),
            icon: a.icon.clone(),
            max_tools_in_context: a.max_tools_in_context,
            tool_loop: a.tool_loop.as_ref().map(|tl| AgentDetailToolLoopDto {
                max_iterations: tl.max_iterations,
                compact_on_overflow: tl.compact_on_overflow,
                detect_loops: tl.detect_loops,
                warn_threshold: tl.warn_threshold,
                break_threshold: tl.break_threshold,
                max_consecutive_failures: tl.max_consecutive_failures,
                max_auto_continues: tl.max_auto_continues,
                max_loop_nudges: tl.max_loop_nudges,
                ngram_cycle_length: tl.ngram_cycle_length,
            }),
            approval: a.approval.as_ref().map(|ap| AgentDetailApprovalDto {
                enabled: Some(ap.enabled),
                require_for: Some(ap.require_for.clone()),
                require_for_categories: Some(ap.require_for_categories.clone()),
                timeout_seconds: Some(ap.timeout_seconds),
            }),
            routing: a.routing.iter().map(|r| AgentDetailRoutingDto {
                condition: r.condition.clone(),
                connection: r.connection.clone(),
                model: r.model.clone(),
                temperature: r.temperature,
                cooldown_secs: r.cooldown_secs,
            }).collect(),
            watchdog: a.watchdog.as_ref().map(|w| AgentDetailWatchdogDto {
                inactivity_secs: Some(w.inactivity_secs),
            }),
            hooks: a.hooks.as_ref().map(|h| AgentDetailHooksDto {
                log_all_tool_calls: Some(h.log_all_tool_calls),
                block_tools: Some(h.block_tools.clone()),
            }),
            max_history_messages: a.max_history_messages,
            daily_budget_tokens: a.daily_budget_tokens,
            max_agent_turns: a.max_agent_turns,
            max_failover_attempts: a.max_failover_attempts,
            is_running,
            config_dirty,
            voice,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentConfig;

    fn load_fixture(name: &str) -> AgentConfig {
        let path = format!("{}/tests/fixtures/agents/{name}.toml", env!("CARGO_MANIFEST_DIR"));
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        toml::from_str(&content).unwrap_or_else(|e| panic!("parse {path}: {e}"))
    }

    #[test]
    fn agent_detail_dto_snapshot_min() {
        let cfg = load_fixture("SnapshotMin");
        let dto = AgentDetailDto::from_config(&cfg, false, false, None);
        insta::assert_json_snapshot!("agent_detail_snapshot_min", dto);
    }

    #[test]
    fn agent_detail_dto_snapshot_full() {
        let cfg = load_fixture("SnapshotFull");
        let dto = AgentDetailDto::from_config(&cfg, false, false, None);
        insta::assert_json_snapshot!("agent_detail_snapshot_full", dto);
    }
}
