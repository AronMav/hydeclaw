---
name: multi-agent-coordination
description: Task coordination between agents — delegation, tracking, result synthesis
triggers:
  - delegate
  - assign to agent
  - ask agent
  - coordination
  - spawn agent
  - делегируй
  - поручи агенту
  - спроси агента
  - координация
priority: 5
tools_required:
  - agent
---

## Agent Tool — Reference

The `agent` tool delegates tasks to other agents and manages their lifecycle.

### Actions

| Action | Parameters | Behavior |
|--------|-----------|----------|
| `run` | `target`, `task` | **Blocks** until agent completes (1-5 min). Returns result directly. |
| `run` | `target`, `task`, `mode="async"` | Starts agent and returns immediately. Use `collect` to get result. |
| `collect` | `target` | **Blocks** until async agent completes. Returns result. |
| `message` | `target`, `text` | Sends follow-up to a running agent. Returns immediately. |
| `status` | `target` (optional) | Without target: list all agents. With target: single agent details. |
| `kill` | `target` | Terminates agent, frees resources. |

### Patterns

#### Single agent (most common — 1 tool call)

```
agent(action="run", target="Alma", task="Analyze portfolio risk and return summary table")
→ blocks 1-3 min → returns Alma's analysis directly
```

Use this for simple delegation. One call, one result.

#### Parallel agents (4 tool calls)

```
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

#### Follow-up question (2 tool calls)

```
agent(action="message", target="Alma", text="Now compare your analysis with this data: ...")
→ "Message sent"

agent(action="collect", target="Alma")
→ blocks → updated analysis
```

Use when an agent is already running and you want to give it more context.

#### Chain delegation (agent calls agent)

Agents can call other agents. Example:

```
You → Arty: "Ask Alma to get weather from Hyde"
Arty → agent(run, target=Alma, task="Use agent tool to ask Hyde for weather in Moscow")
Alma → agent(run, target=Hyde, task="What's the weather in Moscow?")
Hyde → searches web → returns weather
Alma → returns weather to Arty
Arty → returns weather to user
```

Each agent in the chain uses the same `agent` tool.

### Task Format

```
Task: [specific description]
Context: [minimum needed — agent has NO access to your conversation]
Response format: [what to return]
```

Agents work in isolated contexts. They don't see your conversation history. Include all necessary data in the task.

### Rules

- Default `run` **blocks** — no polling needed, result comes back directly
- Agents take 1-5 minutes on this hardware — this is normal
- Kill agents when done if using async mode
- Do NOT poll `status` in a loop — use blocking `run` or `collect`
- Do NOT send entire conversation — only what's needed
- Do NOT delegate trivial tasks — faster to do yourself
