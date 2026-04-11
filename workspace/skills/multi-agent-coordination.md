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

## Agent Coordination Strategy

### When to delegate

- The task requires another agent's expertise
- Parallel execution of multiple tasks is needed
- The task is long-running and does not require your attention

### The `agent` Tool

#### Single agent (most common)

```
agent(action="run", target="Alma", task="Analyze the investment portfolio and provide risk assessment")
→ blocks until Alma completes (1-3 minutes) → returns result directly
```

The tool BLOCKS until the agent finishes. You get the result in one call — no polling needed.

#### Parallel agents

```
// 1. Start both without waiting
agent(action="run", target="Alma", task="Task A", mode="async")
→ "Agent Alma started"

agent(action="run", target="Hyde", task="Task B", mode="async")
→ "Agent Hyde started"

// 2. Do other useful work while they process
bcs_portfolio()  // fetch data, run tools, etc.

// 3. Collect results (blocks until each completes)
agent(action="collect", target="Alma")
→ blocks → returns Alma's result

agent(action="collect", target="Hyde")
→ blocks → returns Hyde's result

// 4. Synthesize both results for the user
```

#### Follow-up messages

```
agent(action="message", target="Alma", text="Now compare with Hyde's analysis: ...")
→ "Message queued"

agent(action="collect", target="Alma")
→ blocks → returns updated analysis
```

#### Other actions

```
agent(action="status")                    // list all agents (diagnostic)
agent(action="status", target="Alma")     // check specific agent
agent(action="kill", target="Alma")       // terminate agent
```

### Agent Request Format

```
Task: [specific description]
Context: [minimum needed — agent has NO access to your conversation]
Response format: [what to return]
```

### Lifecycle

- Agents are bound to the current session
- They stay alive until killed or the session ends
- Each agent maintains its own conversation context across messages
- Agents can take 1-3 minutes to complete — this is normal

### Anti-patterns

- DO NOT use `status` polling in a loop — use blocking `run` or `collect` instead
- DO NOT send the entire conversation context — only what's needed
- DO NOT delegate simple tasks that are faster to do yourself
- DO NOT leave agents running after their task is done — kill them
