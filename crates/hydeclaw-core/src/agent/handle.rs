use std::sync::Arc;
use uuid::Uuid;

use super::channel_actions::ChannelActionRouter;
use super::engine::AgentEngine;
use crate::scheduler::Scheduler;

/// Runtime handle for a running agent — holds everything needed to stop it gracefully.
pub struct AgentHandle {
    pub engine: Arc<AgentEngine>,
    /// Scheduler job UUIDs registered for this agent (heartbeat).
    pub scheduler_job_ids: Vec<Uuid>,
    /// Multi-channel router — WS handlers subscribe via `router.subscribe()`.
    pub channel_router: Option<ChannelActionRouter>,
}

impl AgentHandle {
    /// Gracefully stop all agent tasks: cancel subagents, remove scheduler jobs.
    pub async fn shutdown(mut self, scheduler: &Scheduler) {
        let agent_name = &self.engine.agent.name;

        // Cancel all running subagents (REL-05)
        let all = self.engine.subagent_registry().list_summary().await;
        let mut cancelled_count = 0u32;
        for sa in &all {
            if sa.status == crate::agent::subagent_state::SubagentStatus::Running
                && let Some(handle) = self.engine.subagent_registry().get(&sa.id).await
            {
                let h = handle.read().await;
                h.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                cancelled_count += 1;
                tracing::info!(agent = %agent_name, subagent = %sa.id, "cancelled subagent on shutdown");
            }
        }
        if cancelled_count > 0 {
            tracing::info!(agent = %agent_name, count = cancelled_count, "cancelled running subagents");
        }

        // Remove scheduler jobs (heartbeat)
        for uuid in self.scheduler_job_ids.drain(..) {
            if let Err(e) = scheduler.remove_job(uuid).await {
                tracing::warn!(agent = %agent_name, job = %uuid, error = %e, "failed to remove scheduler job");
            }
        }

        tracing::info!(agent = %agent_name, "agent stopped");
    }
}
