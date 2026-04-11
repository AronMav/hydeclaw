use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use super::engine::AgentEngine;

// ── Status constants ──────────────────────────────────────────────────────────

pub const STATUS_IDLE: u8 = 0;
pub const STATUS_PROCESSING: u8 = 1;

// ── AgentMessage ─────────────────────────────────────────────────────────────

/// Message sent to a LiveAgent's processing loop.
pub struct AgentMessage {
    pub text: String,
}

// ── LiveAgent ─────────────────────────────────────────────────────────────────

/// An always-alive agent instance bound to a session.
pub struct LiveAgent {
    pub engine: Arc<AgentEngine>,
    pub name: String,
    pub message_tx: mpsc::Sender<AgentMessage>,
    pub status: Arc<AtomicU8>,
    pub last_result: Arc<RwLock<Option<String>>>,
    pub cancel: Arc<AtomicBool>,
    pub created_at: Instant,
    pub iteration_count: Arc<AtomicUsize>,
    pub task_handle: tokio::task::JoinHandle<()>,
}

impl LiveAgent {
    /// Returns true if the agent is currently processing a message.
    pub fn is_processing(&self) -> bool {
        self.status.load(Ordering::Acquire) == STATUS_PROCESSING
    }

    /// Returns true if the agent is idle (not processing).
    pub fn is_idle(&self) -> bool {
        self.status.load(Ordering::Acquire) == STATUS_IDLE
    }

    /// Returns the number of iterations (messages processed) by this agent.
    pub fn iterations(&self) -> usize {
        self.iteration_count.load(Ordering::Relaxed)
    }

    /// Returns the elapsed time since this agent was created.
    pub fn elapsed(&self) -> std::time::Duration {
        self.created_at.elapsed()
    }
}

impl Drop for LiveAgent {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.task_handle.abort();
    }
}

// ── SessionAgentPool ─────────────────────────────────────────────────────────

/// Pool of always-alive agents for a single session.
pub struct SessionAgentPool {
    agents: HashMap<String, LiveAgent>,
    session_id: Uuid,
}

impl SessionAgentPool {
    /// Creates a new empty pool for the given session.
    pub fn new(session_id: Uuid) -> Self {
        Self {
            agents: HashMap::new(),
            session_id,
        }
    }

    /// Returns the session ID this pool is associated with.
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    /// Returns a reference to the named agent, if present.
    pub fn get(&self, name: &str) -> Option<&LiveAgent> {
        self.agents.get(name)
    }

    /// Returns true if the pool contains an agent with the given name.
    pub fn contains(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    /// Inserts a live agent into the pool.
    pub fn insert(&mut self, agent: LiveAgent) {
        self.agents.insert(agent.name.clone(), agent);
    }

    /// Removes and returns the named agent from the pool, if present.
    pub fn remove(&mut self, name: &str) -> Option<LiveAgent> {
        self.agents.remove(name)
    }

    /// Returns a list of lightweight status summaries for all agents in the pool.
    pub fn list(&self) -> Vec<AgentPoolEntry> {
        self.agents
            .values()
            .map(|a| AgentPoolEntry {
                name: a.name.clone(),
                status: if a.is_processing() {
                    "processing".to_string()
                } else {
                    "idle".to_string()
                },
                iterations: a.iterations(),
                elapsed_secs: a.elapsed().as_secs_f64(),
            })
            .collect()
    }

    /// Cancels and drops all agents in the pool.
    pub fn kill_all(&mut self) {
        self.agents.clear();
    }

    /// Returns the number of agents in the pool.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Returns true if the pool contains no agents.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }
}

// ── AgentPoolEntry ────────────────────────────────────────────────────────────

/// Lightweight status summary for a live agent in the pool.
#[derive(Debug, Clone, Serialize)]
pub struct AgentPoolEntry {
    pub name: String,
    pub status: String,
    pub iterations: usize,
    pub elapsed_secs: f64,
}
