//! Event hook system — synchronous only (no async DB/HTTP calls inside hooks).
//!
//! Hooks intercept engine events for policy enforcement, logging, and argument modification.
//! Use hooks for automated blocking; use the approval system for human-in-the-loop.

#[derive(Debug, Clone)]
pub enum HookEvent {
    BeforeMessage,
    AfterResponse,
    BeforeToolCall { agent: String, tool_name: String },
    AfterToolResult { agent: String, tool_name: String, duration_ms: u64 },
    OnError,
}

#[derive(Debug, Clone)]
pub enum HookAction {
    /// Continue normal execution.
    Continue,
    /// Block execution with reason.
    Block(String),
}

pub type HookHandler = Box<dyn Fn(&HookEvent) -> HookAction + Send + Sync>;

pub struct HookRegistry {
    handlers: Vec<(String, HookHandler)>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }

    pub fn register(&mut self, name: String, handler: HookHandler) {
        tracing::info!(hook = %name, "hook registered");
        self.handlers.push((name, handler));
    }

    /// Fire event through all handlers. First non-Continue action wins.
    pub fn fire(&self, event: &HookEvent) -> HookAction {
        for (name, handler) in &self.handlers {
            match handler(event) {
                HookAction::Continue => continue,
                action => {
                    tracing::debug!(hook = %name, event = ?std::mem::discriminant(event), "hook intercepted");
                    return action;
                }
            }
        }
        HookAction::Continue
    }

    /// List registered hook names.
    pub fn names(&self) -> Vec<&str> {
        self.handlers.iter().map(|(n, _)| n.as_str()).collect()
    }
}

/// Built-in hook: log all tool calls via tracing.
pub fn logging_hook() -> HookHandler {
    Box::new(|event| {
        if let HookEvent::BeforeToolCall { agent, tool_name } = event {
            tracing::info!(agent = %agent, tool = %tool_name, "hook: tool call");
        }
        if let HookEvent::AfterToolResult { agent, tool_name, duration_ms } = event {
            tracing::info!(agent = %agent, tool = %tool_name, duration_ms, "hook: tool result");
        }
        HookAction::Continue
    })
}

/// Built-in hook: block specific tools by name.
pub fn block_tools_hook(blocked: Vec<String>) -> HookHandler {
    Box::new(move |event| {
        if let HookEvent::BeforeToolCall { tool_name, .. } = event
            && blocked.iter().any(|b| b == tool_name) {
                return HookAction::Block(format!("tool '{tool_name}' is blocked by policy"));
            }
        HookAction::Continue
    })
}
