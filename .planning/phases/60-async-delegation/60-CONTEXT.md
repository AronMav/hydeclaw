# Phase 60: Async Delegation Model - Context

**Gathered:** 2026-04-11
**Status:** Ready for planning

<domain>
## Phase Boundary

Replace blocking handoff (turn loop switching) with async delegation model inspired by Claude Code. Parent agent stays in control, target agents run as isolated async subagents, results injected as user messages.

</domain>

<decisions>
## Implementation Decisions

### Architecture (approved by user)
- `handoff` tool returns IMMEDIATELY with "Agent X is working on the task"
- Target agent spawned as async subagent (reuse existing subagent infrastructure)
- Target runs in isolated context (own system prompt, own messages, only task+context)
- When target completes → result stored in subagent registry
- After Arty's turn ends, turn loop checks registry for completed subagents
- Completed results injected as user messages: `[Response from X]\n{result}`
- Arty continues processing with the injected response
- SSE stream NEVER blocked — UI updates in real-time

### Files to Change
1. `engine_handoff.rs` — spawn async subagent on TARGET engine, return immediately
2. `chat.rs` turn loop — after handle_sse, poll subagent registry for completed handoffs, inject as user messages, continue loop
3. `engine_subagent.rs` — reuse existing registry/completion_rx infrastructure
4. `streaming-renderer.ts` — remove agent-turn/handoff-specific turn loop logic
5. `ChatThread.tsx` — simplify handoff UI (no more handoff stack/dividers needed)

### Key Insight from Claude Code Analysis
- Claude Code NEVER switches active agent. Parent always controls.
- Sub-agents are tool calls that block (sync) or run background (async).
- Results come back as `<task-notification>` XML in user messages.
- Coordinator mode: specialized system prompt teaching LLM to orchestrate workers.

### Context Enrichment
- `get_recent_tool_results()` already implemented — auto-includes last 5 tool results in handoff context
- Target gets: task + context from initiator + recent tool results (truncated to 1000 chars each)

### Identity Isolation
- Each target agent uses own system prompt via `run_subagent()` on target engine
- Target does NOT see initiator's conversation history
- No identity confusion — each agent is fully isolated

</decisions>

<code_context>
## Existing Infrastructure to Reuse

- `engine_subagent.rs`: `subagent_registry().register()` → returns (id, handle, cancel, completion_rx)
- `run_subagent()` on any AgentEngine — isolated execution with own context
- `subagent_semaphore()` — concurrency control
- `SubagentHandle` with status tracking (Running, Completed, Failed)
- `completion_rx` — oneshot channel for waiting on completion

## Current Handoff Issues
- Turn loop switching: SSE blocks during target's handle_sse
- Shared session: all agents see all messages → identity confusion  
- handoff_target Arc<Mutex<Option>> pattern → stale state issues
- handoff_stack for return routing → complex, error-prone

</code_context>

<specifics>
## Implementation Steps

### Step 1: Backend - Async Handoff
In `engine_handoff.rs`:
- Resolve target engine from agent_map
- Call `target_engine.subagent_registry().register(task)`
- Spawn tokio task: `target_engine.run_subagent(full_task, ...)`
- Return immediately: `"Handoff to {target} accepted. Agent is working on the task. Result will be provided when complete."`
- Store (subagent_id, target_name) in a new `pending_handoffs` map on the engine

### Step 2: Backend - Result Injection in Turn Loop
In `chat.rs` turn loop, after `handle_sse` returns:
- Check `current_engine.pending_handoffs()` for completed subagents
- For each completed: inject `[Response from {target}]\n{result}` as user message
- Set `current_msg` to the injected message
- Continue loop (Arty processes the response)

### Step 3: Frontend Cleanup
- `streaming-renderer.ts`: remove agent-turn rich card handling that switches currentAgent
- `ChatThread.tsx`: remove handoff stack, simplify showThinking
- `MessageList.tsx`: HandoffDivider can stay (shows agent name) but no longer switches context
- `page.tsx`: remove handoff-related session restore logic

</specifics>

<deferred>
## Deferred Ideas

- Coordinator mode (specialized system prompt for orchestration)
- Fork with cache sharing (child inherits parent's prompt cache)
- Parallel async handoffs (multiple agents working simultaneously)
- Inter-agent messaging (SendMessage pattern from Claude Code)

</deferred>
