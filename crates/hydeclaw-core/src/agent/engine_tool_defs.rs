//! Tool definitions (internal + sandbox) — thin delegation to `pipeline::tool_defs`.

use super::*;

use crate::agent::pipeline::tool_defs as ptd;

/// All system (internal) tool names — single source of truth.
/// Delegates to `pipeline::tool_defs::all_system_tool_names()`.
pub fn all_system_tool_names() -> &'static [&'static str] {
    ptd::all_system_tool_names()
}

impl AgentEngine {
    /// Resolve tool group settings (from agent config or defaults).
    pub(super) fn tool_groups(&self) -> &crate::config::ToolGroups {
        ptd::resolve_tool_groups(self.agent.tools.as_ref())
    }

    /// Return tool definitions for internal tools available to the LLM.
    pub(super) fn internal_tool_definitions(&self) -> Vec<ToolDefinition> {
        let browser_url = Self::browser_renderer_url();
        let ctx = ptd::ToolDefsContext {
            is_base: self.agent.base,
            groups: self.tool_groups(),
            default_timezone: &self.default_timezone,
            has_sandbox: self.sandbox().is_some(),
            browser_renderer_url: &browser_url,
        };
        ptd::build_internal_tool_definitions(&ctx)
    }

    /// Internal tool definitions filtered for subagent use.
    /// If `allowed_tools` is Some, only those tools are included.
    /// If None, all tools except `SUBAGENT_DENIED_TOOLS` are included.
    pub(super) fn internal_tool_definitions_for_subagent(
        &self,
        allowed_tools: Option<&[String]>,
    ) -> Vec<hydeclaw_types::ToolDefinition> {
        ptd::filter_for_subagent(
            self.internal_tool_definitions(),
            super::subagent_impl::SUBAGENT_DENIED_TOOLS,
            allowed_tools,
        )
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_in_system_tool_names() {
        let names = all_system_tool_names();
        assert!(names.contains(&"agent"), "agent must be in all_system_tool_names()");
        assert!(!names.contains(&"handoff"), "handoff should be removed");
        assert!(!names.contains(&"subagent"), "subagent should be removed");
    }
}
