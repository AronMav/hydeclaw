//! Memory-related internal tools — thin delegations to `pipeline::memory`.

use super::*;
use crate::agent::pipeline::memory as pipeline_memory;

// Re-export MemoryContext from pipeline for backward compat within `engine` submodules.
pub(super) use crate::agent::pipeline::memory::MemoryContext;

impl AgentEngine {
    /// Build L0 memory context: load pinned chunks for this agent.
    /// Called from build_context() in engine.rs before the system prompt size log.
    pub(super) async fn build_memory_context(&self, budget_tokens: u32) -> MemoryContext {
        pipeline_memory::build_memory_context(
            self.memory_store.as_ref(),
            &self.agent.name,
            budget_tokens,
        ).await
    }

    /// Index extracted facts into memory (called after session compaction via /compact).
    /// Uses batch embedding for efficiency when multiple facts are available.
    pub(super) async fn index_facts_to_memory(&self, facts: &[String]) {
        pipeline_memory::index_facts_to_memory(
            self.memory_store.as_ref(),
            &self.agent.name,
            facts,
        ).await
    }

    /// Internal tool: search long-term memory.
    pub(super) async fn handle_memory_search(&self, args: &serde_json::Value) -> String {
        let pinned_ids = self.tex().pinned_chunk_ids.lock().await.clone();
        pipeline_memory::handle_memory_search(
            self.memory_store.as_ref(),
            &self.agent.name,
            &pinned_ids,
            args,
        ).await
    }

    /// Internal tool: index content into long-term memory.
    pub(super) async fn handle_memory_index(&self, args: &serde_json::Value) -> String {
        pipeline_memory::handle_memory_index(
            self.memory_store.as_ref(),
            &self.agent.name,
            args,
        ).await
    }

    /// Internal tool: bulk re-index all .md/.txt files from the entire workspace into memory.
    pub(super) async fn handle_memory_reindex(&self, args: &serde_json::Value) -> String {
        pipeline_memory::handle_memory_reindex(
            self.memory_store.as_ref(),
            &self.agent.name,
            &self.workspace_dir,
            args,
        ).await
    }

    /// Internal tool: get memory chunks by ID or source.
    pub(super) async fn handle_memory_get(&self, args: &serde_json::Value) -> String {
        pipeline_memory::handle_memory_get(
            self.memory_store.as_ref(),
            args,
        ).await
    }

    /// Internal tool: delete a memory chunk by UUID.
    pub(super) async fn handle_memory_delete(&self, args: &serde_json::Value) -> String {
        pipeline_memory::handle_memory_delete(
            self.memory_store.as_ref(),
            args,
        ).await
    }

    /// Internal tool: add/update/remove an entry in the agent's MEMORY.md file.
    pub(super) async fn handle_memory_update(&self, args: &serde_json::Value) -> String {
        pipeline_memory::handle_memory_update(
            &self.tex().memory_md_lock,
            &self.workspace_dir,
            &self.agent.name,
            args,
        ).await
    }
}
