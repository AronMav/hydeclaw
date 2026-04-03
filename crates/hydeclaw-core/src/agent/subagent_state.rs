use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, serde::Serialize)]
pub struct SubagentLogEntry {
    pub iteration: usize,
    pub timestamp: DateTime<Utc>,
    pub tool_calls: Vec<String>,
    pub content_preview: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SubagentHandle {
    pub id: String,
    pub task: String,
    pub status: SubagentStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub log: Vec<SubagentLogEntry>,
    #[serde(skip)]
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
    Killed,
}

/// Lightweight summary for list display (no logs/result cloning).
#[derive(Debug, Clone)]
pub struct SubagentSummary {
    pub id: String,
    pub task: String,
    pub status: SubagentStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub iterations: usize,
}

/// Per-agent registry of subagent handles. Clone-safe (inner Arc).
#[derive(Debug, Clone, Default)]
pub struct SubagentRegistry {
    inner: Arc<RwLock<HashMap<String, Arc<RwLock<SubagentHandle>>>>>,
}

impl SubagentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new subagent. Returns (id, handle, cancel_token) — cancel token
    /// is returned separately to avoid an extra read lock on the freshly created handle.
    pub async fn register(&self, task: &str) -> (String, Arc<RwLock<SubagentHandle>>, Arc<std::sync::atomic::AtomicBool>) {
        let id = format!("sa_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = Arc::new(RwLock::new(SubagentHandle {
            id: id.clone(),
            task: task.chars().take(200).collect(),
            status: SubagentStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            result: None,
            error: None,
            log: Vec::new(),
            cancel: cancel.clone(),
        }));
        self.inner.write().await.insert(id.clone(), handle.clone());
        (id, handle, cancel)
    }

    pub async fn get(&self, id: &str) -> Option<Arc<RwLock<SubagentHandle>>> {
        self.inner.read().await.get(id).cloned()
    }

    /// List lightweight summaries — avoids cloning large result/log fields.
    pub async fn list_summary(&self) -> Vec<SubagentSummary> {
        let arcs: Vec<Arc<RwLock<SubagentHandle>>> = {
            self.inner.read().await.values().cloned().collect()
        };
        let mut result = Vec::with_capacity(arcs.len());
        for h in &arcs {
            let h = h.read().await;
            result.push(SubagentSummary {
                id: h.id.clone(),
                task: h.task.clone(),
                status: h.status,
                started_at: h.started_at,
                finished_at: h.finished_at,
                iterations: h.log.len(),
            });
        }
        result
    }

    /// Remove completed/failed/killed entries older than max_age.
    pub async fn cleanup(&self, max_age: chrono::Duration) {
        let cutoff = Utc::now() - max_age;
        let arcs: Vec<(String, Arc<RwLock<SubagentHandle>>)> = {
            self.inner.read().await.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let mut to_remove = Vec::new();
        for (id, h) in &arcs {
            let handle = h.read().await;
            if handle.status != SubagentStatus::Running && handle.started_at < cutoff {
                to_remove.push(id.clone());
            }
        }
        if !to_remove.is_empty() {
            let mut map = self.inner.write().await;
            for id in &to_remove {
                map.remove(id);
            }
            tracing::debug!(removed = to_remove.len(), "subagent registry cleanup");
        }
    }
}
