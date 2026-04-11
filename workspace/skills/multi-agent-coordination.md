---
name: multi-agent-coordination
description: Task coordination between agents — delegation, parallel execution, orchestration with structured plans
triggers:
  - delegate
  - assign to agent
  - ask agent
  - coordination
  - spawn agent
  - orchestrate
  - coordinate agents
  - run in parallel
  - multi-step task
  - task plan
  - делегируй
  - поручи агенту
  - спроси агента
  - координация
  - параллельная задача
  - план задачи
priority: 5
tools_required:
  - agent
---

## Agent Tool — Reference

The `agent` tool delegates tasks to other agents and manages their lifecycle.

### Actions

| Action | Parameters | Behavior |
| ------ | ---------- | -------- |
| `run` | `target`, `task` | **Blocks** until agent completes (1-5 min). Returns result directly. |
| `run` | `target`, `task`, `mode="async"` | Starts agent and returns immediately. Use `collect` to get result. |
| `collect` | `target` | **Blocks** until async agent completes. Returns result. |
| `message` | `target`, `text` | Sends follow-up to a running agent. Returns immediately. |
| `status` | `target` (optional) | Without target: list all agents. With target: single agent details. |
| `kill` | `target` | Terminates agent, frees resources. |

### Task Format

Agents work in isolated contexts — they don't see your conversation history.

```text
Task: [specific description]
Context: [minimum needed data]
Response format: [what to return]
```

---

## Patterns

### Single agent (1 tool call)

```text
agent(action="run", target="Alma", task="Analyze portfolio risk and return summary table")
→ blocks 1-3 min → returns Alma's analysis directly
```

Use this for simple delegation.

### Parallel agents (4 tool calls)

```text
agent(action="run", target="Alma", task="Task A", mode="async")
→ "Agent Alma started"

agent(action="run", target="Hyde", task="Task B", mode="async")
→ "Agent Hyde started"

agent(action="collect", target="Alma")
→ blocks → Alma's result

agent(action="collect", target="Hyde")
→ blocks → Hyde's result
```

Use when you need multiple agents working simultaneously.

### Follow-up question (2 tool calls)

```text
agent(action="message", target="Alma", text="Compare your analysis with this: ...")
→ "Message sent"

agent(action="collect", target="Alma")
→ blocks → updated analysis
```

### Chain delegation (agent calls agent)

Agents can call other agents using the same `agent` tool:

```text
You → Arty: "Ask Alma to get weather from Hyde"
Arty → agent(run, target=Alma, task="Use agent tool to ask Hyde for weather")
  Alma → agent(run, target=Hyde, task="Weather in Moscow?")
    Hyde → searches web → returns weather
  Alma → returns weather
Arty → returns weather to user
```

---

## Orchestration with Plans

For complex multi-step tasks (3+ steps, dependencies between them), use a structured JSON plan file to track progress.

### When to use a plan

- 2+ agents with sequential dependencies
- Progress must be visible to the user or other agents
- Long-running task with checkpoints

Single-agent tasks do NOT need a plan.

### Plan format

Plan files live at `tasks/task_YYYYMMDD_XXXXXX.json` in workspace.

```json
{
  "task_id": "task_20260408_a1b2c3",
  "agent": "AgentName",
  "title": "Brief description",
  "status": "planning",
  "steps": [
    {"id": "step_1", "title": "Fetch data", "status": "pending"},
    {"id": "step_2", "title": "Analyze", "status": "pending"},
    {"id": "step_3", "title": "Write report", "status": "pending"}
  ]
}
```

Status transitions: `pending` → `in_progress` → `done` | `error`

### Plan workflow

```text
// 1. Write plan
workspace_write(path="tasks/task_20260408_x9y8z7.json", content=<plan JSON>)

// 2. Run step 1 (blocks until complete)
result = agent(action="run", target="Alma", task="Fetch Q1 revenue data")

// 3. Update plan: step_1 done, step_2 in_progress
workspace_write(path="tasks/task_20260408_x9y8z7.json", content=<updated plan>)

// 4. Run step 2... repeat
```

### Parallel steps in plans

```text
// Mark both steps in_progress in the plan first
workspace_write(path="tasks/...", content=<steps 1+2 in_progress>)

// Spawn both async
agent(action="run", target="Alma", task="Task A", mode="async")
agent(action="run", target="Hyde", task="Task B", mode="async")

// Collect results
result_a = agent(action="collect", target="Alma")
result_b = agent(action="collect", target="Hyde")

// Update plan with results
```

Always use `workspace_write` (full rewrite), NOT `workspace_edit`, for plan files.

---

## Rules

- Default `run` **blocks** — no polling needed
- Agents take 1-5 minutes on this hardware — this is normal
- Kill agents when done if using async mode
- Do NOT poll `status` in a loop — use blocking `run` or `collect`
- Do NOT send entire conversation — only what's needed
- Do NOT delegate trivial tasks — faster to do yourself
- Do NOT create plans for single-agent tasks — overhead not worth it
