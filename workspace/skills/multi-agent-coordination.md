---
name: multi-agent-coordination
description: Task coordination between agents — delegation, tracking, result synthesis
triggers:
  - delegate
  - assign to agent
  - ask agent
  - coordination
  - subagent
  - spawn
  - делегируй
  - поручи агенту
  - спроси агента
  - координация
  - субагент
priority: 5
tools_required:
  - subagent
  - send_to_agent
---

## Agent Coordination Strategy

### When to delegate

- The task requires another agent's expertise
- Parallel execution of multiple tasks is needed
- The task is long-running and does not require your attention

### Delegation Principles

#### 1. Clear assignment
Send each subagent:
- **What to do** — a specific task, not vague wishes
- **Result format** — exactly what to return (number, list, text)
- **Context** — minimum needed to complete the task
- **Constraints** — time, scope, priority

#### 2. Choosing the right agent
- **Architect** — design, configuration, system tasks (base)
- **Other agents** — by their specialization (see AGENTS.md)
- **Subagent** — for one-off tasks that don't require specialization

#### 3. Collecting and synthesizing results
- Do not forward the raw subagent response — interpret it
- If results are contradictory — note the discrepancies
- If the subagent failed — try another approach or do it yourself

### Async Subagent Workflow

Subagents run **asynchronously** — `subagent(action="spawn")` returns an ID immediately, work continues in background.

**CRITICAL: You MUST monitor every subagent until completion. NEVER respond to the user while subagents are still running.**

#### 1. Spawn
```
subagent(action="spawn", task="Research weather in Moscow and Samara, return comparison table")
→ "Subagent spawned: id=sa_a1b2c3d4"
```

#### 2. Monitor loop (MANDATORY)
```
// Repeat until status != running
subagent(action="status", subagent_id="sa_a1b2c3d4")
→ running, 3 iterations, 15s elapsed

// If running too long or you suspect loops:
subagent(action="logs", subagent_id="sa_a1b2c3d4", last_n=5)
→ [iter 2] tools=[search_web] "Found weather data..."
```

#### 3. Detect stuck / loops
If `subagent(action="logs")` shows the same tools repeated with identical arguments → kill and redo:
```
subagent(action="kill", subagent_id="sa_a1b2c3d4")
→ "Kill signal sent"
```

#### 4. Collect result
```
subagent(action="spawn", subagent_id="sa_a1b2c3d4")
→ (waits up to 60s, returns result when done)
```

#### 5. Multiple subagents
When running 2+ subagents, monitor ALL in a single loop:
```
// Spawn all
id1 = subagent(action="spawn", task="Task A")
id2 = subagent(action="spawn", task="Task B")

// Monitor loop — check all, repeat until all done
subagent(action="status")  // no ID = list all
→ - sa_xxx Running (30s, 5 iters): Task A
  - sa_yyy Completed (20s, 3 iters): Task B

// Collect completed ones, keep checking running ones
subagent(action="spawn", subagent_id="sa_yyy")  // collect B
subagent(action="status", subagent_id="sa_xxx") // keep monitoring A
```

### Subagent Request Format

```
Task: [specific description]
Context: [minimum needed — subagent has NO conversation history]
Response format: [what to return]
NOTE: Subagent cannot access workspace files via code_exec (sandbox isolated).
      Read files first with workspace_read and include content in the task.
```

### Anti-patterns

- DO NOT delegate simple tasks that are faster to do yourself
- DO NOT send the entire conversation context — only what's needed
- DO NOT chain delegation (agent → agent → agent) — coordinate yourself
- DO NOT forget to report the delegation result to the user
- DO NOT leave subagents running — monitor until completion or kill
- DO NOT respond to the user while subagents are still running
- DO NOT use code_exec inside subagent to read workspace files — pass data in task
