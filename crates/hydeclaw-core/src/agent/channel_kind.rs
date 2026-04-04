/// Well-known channel identifiers used throughout the engine.
#[allow(dead_code)]
pub mod channel {
    pub const CRON: &str = "cron";
    pub const HEARTBEAT: &str = "heartbeat";
    pub const SYSTEM: &str = "system";
    pub const INTER_AGENT: &str = "inter-agent";
    pub const UI: &str = "ui";
    pub const GROUP: &str = "group";
    pub const TELEGRAM: &str = "telegram";

    /// Returns true for automated channels that bypass approval checks.
    pub fn is_automated(ch: &str) -> bool {
        matches!(ch, CRON | HEARTBEAT | SYSTEM | INTER_AGENT)
    }
}

/// Tool execution categories for approval system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    System,
    Destructive,
    External,
}

impl ToolCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Destructive => "destructive",
            Self::External => "external",
        }
    }

    pub fn classify(tool_name: &str) -> Self {
        match tool_name {
            "code_exec" | "process" | "browser_action" => Self::System,
            "workspace_delete" | "workspace_write" | "workspace_edit" | "workspace_rename" => Self::Destructive,
            n if n.starts_with("git_") => Self::System,
            _ => Self::External,
        }
    }
}
