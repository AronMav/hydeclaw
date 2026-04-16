//! Pipeline — free functions for each step of the agent execution loop.
//!
//! Each function takes explicit `(&AgentConfig, &AgentState, &mut RequestContext)`
//! dependencies instead of `&self` on `AgentEngine`.

pub mod entry;
pub mod execution;
pub mod context;
pub mod llm_call;
pub mod parallel;
pub mod dispatch;
pub mod tool_defs;
pub mod memory;
pub mod commands;
pub mod handlers;
pub mod sandbox;
pub mod subagent;
pub mod agent_tool;
pub mod sessions;
pub mod canvas;
