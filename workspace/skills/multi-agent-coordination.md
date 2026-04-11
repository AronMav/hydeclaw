---
name: multi-agent-coordination
description: Task coordination between agents — spawning, messaging, tracking, result synthesis
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

All inter-agent communication uses a single tool with 4 actions:

#### 1. Start an agent
```
agent(action="run", target="Alma", task="Analyze the investment portfolio and provide risk assessment")
→ "Agent Alma started in session."
```

The agent starts processing immediately. It runs in an isolated context — it does NOT see your conversation history. Include all necessary context in the task.

#### 2. Check status
```
// Check a specific agent
agent(action="status", target="Alma")
→ {"name": "Alma", "status": "idle", "last_result": "...", "iterations": 3, "elapsed_secs": 45}

// List all agents in session
agent(action="status")
→ [{"name": "Alma", "status": "idle", ...}, {"name": "Hyde", "status": "processing", ...}]
```

#### 3. Send a message to a running agent
```
agent(action="message", target="Alma", text="Now compare your analysis with Hyde's risk report: ...")
→ "Message queued for Alma."
```

The agent continues its dialog — it remembers previous context. Poll status to get the new result.

#### 4. Stop an agent
```
agent(action="kill", target="Alma")
→ "Agent Alma terminated."
```

### Coordination Patterns

#### Simple delegation
```
1. agent(action="run", target="Alma", task="Research topic X, return summary")
2. Do other useful work while agent processes (fetch data, etc.)
3. agent(action="status", target="Alma")  // check once after ~60s
4. If still processing, do more work, then check again (max 2-3 checks total)
5. Read last_result from status response when idle
6. Present to user
```

IMPORTANT: Agents need 1-3 minutes on this hardware. Do NOT poll in a tight loop — each check costs an iteration.

#### Parallel agents
```
1. agent(action="run", target="Alma", task="Task A")
2. agent(action="run", target="Hyde", task="Task B")
3. Poll both: agent(action="status")  // list all, repeat until all idle
4. Synthesize results
```

#### Multi-round discussion
```
1. agent(action="run", target="Alma", task="Initial analysis of X")
2. Poll until idle, get Alma's result
3. agent(action="run", target="Hyde", task="Risk assessment of X")
4. Poll until idle, get Hyde's result
5. agent(action="message", target="Alma", text="Hyde found risks: ... What do you think?")
6. Poll until idle, get Alma's updated opinion
7. Synthesize both perspectives for user
```

### Agent Request Format

```
Task: [specific description]
Context: [minimum needed — the agent has NO access to your conversation]
Response format: [what to return]
```

### Lifecycle

- Agents are bound to the current session
- They stay alive until killed or the session ends
- Each agent maintains its own conversation context across messages
- All agents in a session can see each other via `agent(action="status")`
- Any agent can message any other agent in the session (peer-to-peer)

### Anti-patterns

- DO NOT delegate simple tasks that are faster to do yourself
- DO NOT send the entire conversation context — only what's needed
- DO NOT forget to poll for results before responding to the user
- DO NOT leave agents running after their task is done — kill them
- DO NOT respond to the user while agents are still processing
