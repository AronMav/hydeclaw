---
name: agent-management
description: Create, update, delete agents via Core API with GET→modify→PUT pattern
triggers:
  - создай агента
  - новый агент
  - настрой агента
  - измени агента
  - agent
tools_required:
  - code_exec
priority: 10
---

# Agent Management

## Create agent

`provider` and `model` are required. If using a named provider connection, also set `provider_connection`.

```bash
curl -sf -X POST http://localhost:18789/api/agents \
  -H "Authorization: Bearer $HYDECLAW_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "NewAgent",
    "language": "ru",
    "provider": "openai",
    "model": "gpt-4o",
    "provider_connection": "my-openai"
  }'
```

New agents are always non-base (sandboxed Docker container).

## Update agent (GET → modify → PUT)

PUT requires `name`, `provider`, `model` — they are NOT optional. Always GET first, modify, then PUT back.

```python
import json, urllib.request, os
tok = os.environ["HYDECLAW_AUTH_TOKEN"]
hdrs = {"Authorization": f"Bearer {tok}", "Content-Type": "application/json"}

# GET current config
req = urllib.request.Request("http://localhost:18789/api/agents/AgentName", headers=hdrs)
d = json.loads(urllib.request.urlopen(req).read())

# Modify fields
d["temperature"] = 0.7
d["provider_connection"] = "my-openai"

# PUT back
req = urllib.request.Request(
    "http://localhost:18789/api/agents/AgentName",
    data=json.dumps(d).encode(), method="PUT", headers=hdrs
)
print(urllib.request.urlopen(req).read().decode())
```

The agent is hot-restarted after update.

## Available PUT fields

Required: `name`, `provider`, `model`

Optional: `provider_connection`, `temperature`, `max_tokens`, `language`, `heartbeat`, `tools`, `compaction`, `session`, `access`, `routing`, `tool_loop`, `hooks`, `max_history_messages`, `daily_budget_tokens`

## Delete agent (non-base only)

```bash
curl -sf -X DELETE http://localhost:18789/api/agents/AgentName \
  -H "Authorization: Bearer $HYDECLAW_AUTH_TOKEN"
```

## List agents

```bash
curl -sf http://localhost:18789/api/agents \
  -H "Authorization: Bearer $HYDECLAW_AUTH_TOKEN"
```

## Get agent detail

```bash
curl -sf http://localhost:18789/api/agents/AgentName \
  -H "Authorization: Bearer $HYDECLAW_AUTH_TOKEN"
```

## Checklist

1. Check if agent already exists: `GET /api/agents`
2. If creating: `POST /api/agents` with name, provider, model
3. If updating: GET → modify → PUT (never PUT without GET first)
4. Verify: `GET /api/agents/Name` shows correct config
