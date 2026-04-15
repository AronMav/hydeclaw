use std::sync::Arc;

use crate::gateway::state::ConnectedChannelsRegistry;
use crate::gateway::stream_registry::StreamRegistry;

// ── ChannelBus ─────────────────────────────────────────────────────────────
// Groups all real-time communication primitives: the connected-channel
// registry, log/UI broadcast senders, and the SSE stream registry.

#[derive(Clone)]
pub struct ChannelBus {
    pub connected_channels: ConnectedChannelsRegistry,
    pub log_tx:             tokio::sync::broadcast::Sender<String>,
    pub ui_event_tx:        tokio::sync::broadcast::Sender<String>,
    pub stream_registry:    Arc<StreamRegistry>,
}

impl ChannelBus {
    pub fn new(
        connected_channels: ConnectedChannelsRegistry,
        log_tx: tokio::sync::broadcast::Sender<String>,
        ui_event_tx: tokio::sync::broadcast::Sender<String>,
        stream_registry: Arc<StreamRegistry>,
    ) -> Self {
        Self { connected_channels, log_tx, ui_event_tx, stream_registry }
    }

    /// Construct a minimal `ChannelBus` for unit tests.
    /// Uses a lazy (non-connecting) pool — no live DB is required.
    #[cfg(test)]
    pub fn test_new() -> Self {
        use sqlx::PgPool;

        let (log_tx, _) = tokio::sync::broadcast::channel(16);
        let (ui_event_tx, _) = tokio::sync::broadcast::channel(16);
        let db = PgPool::connect_lazy("postgres://invalid").unwrap();
        Self {
            connected_channels: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            log_tx,
            ui_event_tx,
            stream_registry: Arc::new(StreamRegistry::new(db)),
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn channel_bus_broadcast_senders_work() {
        let bus = ChannelBus::test_new();
        let _rx = bus.log_tx.subscribe();
        let _rx2 = bus.ui_event_tx.subscribe();
        // No panic = success
    }
}
