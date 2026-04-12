use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::PgPool;

use crate::agent::engine::AgentEngine;
use crate::agent::handle::AgentHandle;
use crate::channels::access::AccessGuard;
use crate::config::{AppConfig, SharedConfig};
use crate::memory::MemoryStore;
use crate::scheduler::Scheduler;
use crate::secrets::SecretsManager;
use crate::tools::ToolRegistry;

use super::stream_registry;

/// Tracks which agents are currently processing a request.
/// Used to replay `agent_processing` state to newly connected WS clients.
pub type ProcessingTracker = Arc<std::sync::RwLock<HashMap<String, serde_json::Value>>>;

pub type AgentMap = Arc<tokio::sync::RwLock<HashMap<String, AgentHandle>>>;
pub type AccessGuardMap = Arc<tokio::sync::RwLock<HashMap<String, Arc<AccessGuard>>>>;

/// A channel adapter currently connected via WebSocket.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectedChannel {
    pub agent_name: String,
    pub channel_id: Option<uuid::Uuid>,
    pub channel_type: String,
    pub display_name: String,
    pub adapter_version: String,
    pub connected_at: chrono::DateTime<chrono::Utc>,
    /// Updated on every inbound message; used by stale-channel detector.
    pub last_activity: chrono::DateTime<chrono::Utc>,
}

pub type ConnectedChannelsRegistry = Arc<tokio::sync::RwLock<Vec<ConnectedChannel>>>;

/// Atomic counters for channel polling diagnostics.
/// Exposed via GET /api/doctor for "bot not responding" troubleshooting.
pub struct PollingDiagnostics {
    pub messages_in: AtomicU64,
    pub messages_out: AtomicU64,
    pub last_inbound_at: AtomicU64,
    pub last_outbound_at: AtomicU64,
}

impl PollingDiagnostics {
    pub fn new() -> Self {
        Self {
            messages_in: AtomicU64::new(0),
            messages_out: AtomicU64::new(0),
            last_inbound_at: AtomicU64::new(0),
            last_outbound_at: AtomicU64::new(0),
        }
    }

    pub fn record_inbound(&self) {
        self.messages_in.fetch_add(1, Ordering::Relaxed);
        self.last_inbound_at.store(
            chrono::Utc::now().timestamp() as u64,
            Ordering::Relaxed,
        );
    }

    pub fn record_outbound(&self) {
        self.messages_out.fetch_add(1, Ordering::Relaxed);
        self.last_outbound_at.store(
            chrono::Utc::now().timestamp() as u64,
            Ordering::Relaxed,
        );
    }

    #[allow(dead_code)]
    pub fn snapshot(&self) -> serde_json::Value {
        let last_in = self.last_inbound_at.load(Ordering::Relaxed);
        let last_out = self.last_outbound_at.load(Ordering::Relaxed);
        serde_json::json!({
            "messages_in": self.messages_in.load(Ordering::Relaxed),
            "messages_out": self.messages_out.load(Ordering::Relaxed),
            "last_inbound_at": if last_in > 0 {
                chrono::DateTime::from_timestamp(last_in as i64, 0)
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_default()
            } else {
                "never".to_string()
            },
            "last_outbound_at": if last_out > 0 {
                chrono::DateTime::from_timestamp(last_out as i64, 0)
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_default()
            } else {
                "never".to_string()
            },
        })
    }
}

/// Cached WAN (public) IP address with CGNAT classification and a fetch timestamp.
#[derive(Clone)]
pub struct WanIpCache {
    pub ip: String,
    pub is_cgnat: bool,
    pub fetched_at: std::time::Instant,
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: AppConfig,
    /// Dynamic config that updates on file changes (via config watcher).
    pub shared_config: SharedConfig,
    pub tools: ToolRegistry,
    /// Running agent handles (mutable: agents can be added/removed at runtime).
    pub agents: AgentMap,
    pub secrets: Arc<SecretsManager>,
    pub scheduler: Arc<Scheduler>,
    /// Access guards per agent (mutable: created/removed with agents).
    pub access_guards: AccessGuardMap,
    /// Process start time for uptime calculation.
    pub started_at: std::time::Instant,
    /// Broadcast channel for real-time log streaming to UI.
    pub log_tx: tokio::sync::broadcast::Sender<String>,
    /// Shared deps for starting new agents at runtime (`RwLock` for hot-update via PUT /api/config).
    pub agent_deps: Arc<tokio::sync::RwLock<AgentDeps>>,
    /// Native memory store (pgvector + external embedding endpoint).
    pub memory_store: Arc<MemoryStore>,
    /// Docker container manager (for MCP lifecycle + runtime config updates).
    pub container_manager: Option<Arc<crate::containers::ContainerManager>>,
    /// Docker code execution sandbox (managed per-agent containers).
    pub sandbox: Option<Arc<crate::containers::sandbox::CodeSandbox>>,
    /// Broadcast channel for UI events (`session_updated`, `cron_completed`, etc.).
    pub ui_event_tx: tokio::sync::broadcast::Sender<String>,
    /// In-memory registry of active SSE streams for resume support.
    pub stream_registry: Arc<stream_registry::StreamRegistry>,
    /// Tracks which agents are currently processing (for WS reconnection).
    pub processing_tracker: ProcessingTracker,
    /// Runtime registry of connected channel adapters (Telegram, Discord, etc.).
    pub connected_channels: ConnectedChannelsRegistry,
    /// Native child process manager (channels, toolgate — spawned by Core, not Docker).
    pub process_manager: Option<Arc<crate::process_manager::ProcessManager>>,
    /// One-time WS tickets: ticket UUID → creation time. Consumed on first use, 30s TTL.
    pub ws_tickets: Arc<tokio::sync::Mutex<HashMap<String, std::time::Instant>>>,
    /// OAuth 2.0 connection manager (Google, GitHub, etc.)
    pub oauth: Arc<crate::oauth::OAuthManager>,
    /// Atomic counters for channel message diagnostics.
    pub polling_diagnostics: Arc<PollingDiagnostics>,
    /// Cached WAN IP address (refreshed every 5 minutes to avoid hammering STUN/TURN services).
    pub wan_ip_cache: Arc<tokio::sync::RwLock<Option<WanIpCache>>>,
    /// Session-scoped agent pools: maps session UUID → pool of alive agents.
    pub session_pools: crate::agent::session_agent_pool::SessionPoolsMap,
}

/// Shared dependencies needed to start new agents at runtime (from CRUD endpoints).
pub struct AgentDeps {
    pub mcp: Option<Arc<crate::mcp::McpRegistry>>,
    pub workspace_dir: String,
    pub toolgate_url: Option<String>,
    pub sandbox: Option<Arc<crate::containers::sandbox::CodeSandbox>>,
    pub tool_embed_cache: Arc<crate::tools::embedding::ToolEmbeddingCache>,
    pub penalty_cache: Arc<crate::db::tool_quality::PenaltyCache>,
}

impl AppState {
    /// Get an engine by agent name (read-locks the agents map briefly).
    pub async fn get_engine(&self, name: &str) -> Option<Arc<AgentEngine>> {
        self.agents.read().await.get(name).map(|h| h.engine.clone())
    }

    /// Get the first available engine.
    pub async fn first_engine(&self) -> Option<Arc<AgentEngine>> {
        self.agents.read().await.values().next().map(|h| h.engine.clone())
    }

    /// Get list of running agent names (base agents first, then alphabetical).
    pub async fn agent_names(&self) -> Vec<String> {
        let mut names: Vec<(bool, String)> = self.agents.read().await.values()
            .map(|h| (h.engine.agent.base, h.engine.agent.name.clone()))
            .collect();
        names.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        names.into_iter().map(|(_, n)| n).collect()
    }

    /// Get list of running agents with name and icon (base agents first, then alphabetical).
    #[allow(dead_code)]
    pub async fn agent_summaries(&self) -> Vec<serde_json::Value> {
        let mut summaries: Vec<(bool, String, serde_json::Value)> = self.agents.read().await.values()
            .map(|h| (h.engine.agent.base, h.engine.agent.name.clone(), serde_json::json!({
                "name": h.engine.agent.name,
                "icon": h.engine.agent.icon,
            })))
            .collect();
        summaries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        summaries.into_iter().map(|(_, _, v)| v).collect()
    }
}
