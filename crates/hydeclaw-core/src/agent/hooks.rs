//! Async event hook system for engine lifecycle events.
//!
//! Hooks intercept engine events for policy enforcement, logging, and telemetry.
//! Handlers are async (return Future) and ordered by priority (lower = first).
//! Use hooks for automated blocking; use the approval system for human-in-the-loop.

use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum HookEvent {
    BeforeMessage,
    AfterResponse,
    BeforeToolCall { agent: String, tool_name: String },
    AfterToolResult { agent: String, tool_name: String, duration_ms: u64 },
    OnError,
    // ── Extended events (v7.0) ────────────────────────────────────────────
    OnSessionStart { session_id: Uuid, agent: String, channel: String },
    OnSessionEnd { session_id: Uuid, agent: String },
    OnAgentSwitch { session_id: Uuid, from_agent: String, to_agent: String },
    OnApprovalRequired { approval_id: Uuid, agent: String, tool_name: String },
    OnCompaction { agent: String, messages_before: usize, messages_after: usize },
    OnProviderFallback { agent: String, from_provider: String, to_provider: String, reason: String },
}

#[derive(Debug, Clone)]
pub enum HookAction {
    /// Continue normal execution.
    Continue,
    /// Block execution with reason.
    Block(String),
}

/// Async hook handler: receives event reference, returns action.
pub type AsyncHookHandler = Box<
    dyn Fn(&HookEvent) -> Pin<Box<dyn Future<Output = HookAction> + Send>> + Send + Sync,
>;

/// Sync hook handler (backward-compatible convenience wrapper).
pub type HookHandler = Box<dyn Fn(&HookEvent) -> HookAction + Send + Sync>;

struct HookEntry {
    name: String,
    priority: i32,
    handler: AsyncHookHandler,
}

pub struct HookRegistry {
    handlers: Vec<HookEntry>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }

    /// Register an async hook handler with priority (lower runs first).
    pub fn register_async(&mut self, name: String, priority: i32, handler: AsyncHookHandler) {
        tracing::info!(hook = %name, priority, "hook registered");
        self.handlers.push(HookEntry { name, priority, handler });
        self.handlers.sort_by_key(|h| h.priority);
    }

    /// Register a sync hook handler (convenience — wraps in async).
    pub fn register(&mut self, name: String, handler: HookHandler) {
        let async_handler: AsyncHookHandler = Box::new(move |event: &HookEvent| {
            let result = handler(event);
            Box::pin(async move { result })
        });
        self.register_async(name, 100, async_handler);
    }

    /// Fire event through all handlers (async). First non-Continue action wins.
    pub async fn fire(&self, event: &HookEvent) -> HookAction {
        for entry in &self.handlers {
            match (entry.handler)(event).await {
                HookAction::Continue => continue,
                action => {
                    tracing::debug!(hook = %entry.name, event = ?std::mem::discriminant(event), "hook intercepted");
                    return action;
                }
            }
        }
        HookAction::Continue
    }

    /// Fire event synchronously (for backward compatibility in sync contexts).
    /// Only runs sync-wrapped handlers correctly; true async handlers will panic.
    pub fn fire_sync(&self, event: &HookEvent) -> HookAction {
        for entry in &self.handlers {
            // Use block_in_place for sync contexts — safe because hooks are fast
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on((entry.handler)(event))
            });
            match result {
                HookAction::Continue => continue,
                action => {
                    tracing::debug!(hook = %entry.name, event = ?std::mem::discriminant(event), "hook intercepted");
                    return action;
                }
            }
        }
        HookAction::Continue
    }

    /// List registered hook names.
    pub fn names(&self) -> Vec<&str> {
        self.handlers.iter().map(|h| h.name.as_str()).collect()
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
                return HookAction::Block(format!("tool '{}' is blocked by policy", tool_name));
            }
        HookAction::Continue
    })
}
