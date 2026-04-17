//! REF-01 Task 6 target — populated in Task 6 of plan 66-02.
//!
//! Will own: trait impls that wire `AgentEngine` into the tool pipeline —
//! `ToolExecutorDeps`, `pipeline::parallel::ToolExecutor`,
//! `pipeline::llm_call::Compactor` — plus `execute_tool_calls_partitioned`,
//! `tool_groups`, `internal_tool_definitions[_for_subagent]`, and the
//! top-level `all_system_tool_names()` accessor.
