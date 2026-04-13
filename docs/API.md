# HydeClaw API Reference

**Base URL:** `http://<host>:18789`

**Authentication:** All routes require `Authorization: Bearer <HYDECLAW_AUTH_TOKEN>` unless explicitly marked **Public**. The token is configured via `gateway.auth_token_env` in `hydeclaw.toml`.

**Rate limiting:** 10 failed auth attempts within 5 minutes triggers a 5-minute lockout (per IP). Loopback (127.0.0.1) and private LAN IPs are exempt from auth lockout. General request rate limiting is configurable via `limits.max_requests_per_minute`.

---

## Table of Contents

1. [Authentication](#1-authentication)
2. [Monitoring](#2-monitoring)
3. [Agents](#3-agents)
4. [Chat — OpenAI-Compatible](#4-chat--openai-compatible)
5. [Chat SSE (Native Streaming)](#5-chat-sse-native-streaming)
6. [Sessions and Messages](#6-sessions-and-messages)
7. [Memory](#7-memory)
8. [Tools and MCP](#8-tools-and-mcp)
9. [YAML Tools](#9-yaml-tools)
10. [Skills](#10-skills)
11. [Channels](#11-channels)
12. [Cron Jobs](#12-cron-jobs)
13. [Tasks](#13-tasks)
14. [Approvals](#14-approvals)
15. [Webhooks](#15-webhooks)
16. [Secrets](#16-secrets)
17. [Config](#17-config)
18. [Backup and Restore](#18-backup-and-restore)
19. [Services](#19-services)
20. [Watchdog](#20-watchdog)
21. [Providers](#21-providers)
22. [TTS](#22-tts)
23. [Media Upload](#23-media-upload)
24. [Workspace](#24-workspace)
25. [Canvas](#25-canvas)
26. [OAuth](#26-oauth)
27. [Access / Pairing](#27-access--pairing)
28. [WebSocket (UI Events)](#28-websocket-ui-events)
29. [Email Triggers (Gmail)](#29-email-triggers-gmail)
30. [GitHub Integration](#30-github-integration)
31. [Audit Log](#31-audit-log)
32. [Setup](#32-setup)
33. [Network](#33-network)
34. [Notifications](#34-notifications)
35. [Config Schema](#35-config-schema)

---

## 1. Authentication

### WS Ticket

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/auth/ws-ticket` | Issue a one-time WebSocket ticket |

The WebSocket endpoint (`/ws`) requires authentication. Because exposing the static Bearer token in a WebSocket URL is unsafe, use this endpoint to obtain a short-lived ticket and pass it as a query parameter: `/ws?ticket=<uuid>`.

**Response:**
```json
{ "ticket": "uuid-v4-string" }
```

- Tickets are valid for **30 seconds** and consumed on first use.
- Requires standard Bearer auth (the ticket itself is for WS upgrade only).

---

## 2. Monitoring

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/health` | Public | Liveness check |
| `GET` | `/api/setup/status` | Required | Whether initial setup is needed |
| `GET` | `/api/status` | Required | Full gateway status |
| `GET` | `/api/stats` | Required | Message/session statistics |
| `GET` | `/api/usage` | Required | Token usage summary |
| `GET` | `/api/usage/daily` | Required | Daily token usage breakdown |
| `GET` | `/api/usage/sessions` | Required | Per-session token usage |
| `GET` | `/api/doctor` | Required | Health check for all subsystems |
| `GET` | `/api/audit` | Required | Audit event log |
| `GET` | `/api/audit/tools` | Required | Tool invocation audit log |

### GET /health

Returns `200 OK` with no body. Public — use for load balancer / uptime checks.

### GET /api/status

Returns gateway status including running agents, memory chunk count, uptime, and registered tools.

**Response:**
```json
{
  "status": "ok",
  "version": "0.x.x",
  "uptime_seconds": 12345,
  "db": true,
  "listen": "0.0.0.0:18789",
  "agents": ["main", "analyst"],
  "memory_chunks": 2901,
  "scheduled_jobs": 3,
  "active_sessions": 5,
  "tools_registered": 12
}
```

### GET /api/stats

**Response:**
```json
{
  "messages_today": 42,
  "sessions_today": 5,
  "total_messages": 18000,
  "total_sessions": 592,
  "recent_sessions": [
    {
      "id": "uuid",
      "agent_id": "main",
      "channel": "ui",
      "last_message_at": "2026-03-27T10:00:00Z",
      "title": "Session title"
    }
  ]
}
```

### GET /api/usage

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `days` | integer | 30 | Lookback window |
| `agent` | string | — | Filter by agent name |

### GET /api/doctor

Returns health status of all subsystems with latency measurements.

**Response:**
```json
{
  "ok": true,
  "checks": {
    "database": { "ok": true, "latency_ms": 2 },
    "toolgate": { "ok": true, "latency_ms": 15, "providers": [...] },
    "browser_renderer": { "ok": false, "latency_ms": 5001 },
    "searxng": { "ok": true, "latency_ms": 8 },
    "secrets": {
      "ok": true,
      "count": 7,
      "missing_critical": []
    },
    "channels": { "ok": true, "latency_ms": 3 },
    "agents": { "main": { "ok": true } },
    "polling": {
      "messages_in": 1234,
      "messages_out": 1230,
      "last_inbound_at": "2026-03-27T10:00:00Z",
      "last_outbound_at": "2026-03-27T10:00:01Z"
    }
  }
}
```

### GET /api/audit

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `agent` | string | — | Filter by agent name |
| `event_type` | string | — | Filter by event type |
| `limit` | integer | 100 | Max results (max 500) |
| `offset` | integer | 0 | Pagination offset |

### GET /api/audit/tools

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `agent` | string | — | Filter by agent |
| `tool` | string | — | Filter by tool name |
| `days` | integer | 7 | Lookback window |
| `limit` | integer | 100 | Max results (max 500) |

---

## 3. Agents

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/agents` | List all agents |
| `POST` | `/api/agents` | Create a new agent |
| `GET` | `/api/agents/{name}` | Get agent details |
| `PUT` | `/api/agents/{name}` | Update agent config |
| `DELETE` | `/api/agents/{name}` | Delete agent |
| `POST` | `/api/agents/{name}/model-override` | Temporarily override LLM model |
| `GET` | `/api/providers/{id}/models` | List available models for a provider |
| `GET` | `/api/agents/{name}/hooks` | Get agent hook configuration |

### GET /api/agents

Returns all agents (from disk config + running state). Base agents are listed first, then alphabetical.

**Response:**
```json
{
  "agents": [
    {
      "name": "main",
      "language": "ru",
      "model": "MiniMax-M2.5",
      "provider": "minimax",
      "icon": "🤖",
      "temperature": 1.0,
      "has_access": true,
      "access_mode": "allowlist",
      "has_heartbeat": false,
      "heartbeat_cron": null,
      "heartbeat_timezone": null,
      "tool_policy": {
        "allow": [],
        "deny": [],
        "allow_all": true
      },
      "routing_count": 0,
      "is_running": true,
      "config_dirty": false,
      "base": true,
      "base": false
    }
  ]
}
```

### POST /api/agents

Create a new agent. The agent config is written to `config/agents/{name}.toml` and the agent is started immediately.

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Agent name (alphanumeric, `-`, `_`, max 32 chars) |
| `provider` | string | Yes | LLM provider type (e.g. `minimax`, `openai`, `anthropic`) |
| `model` | string | Yes | Model identifier |
| `provider_connection` | string | No | Named LLM provider connection ID (overrides provider/model) |
| `language` | string | No | Response language hint |
| `temperature` | float | No | Sampling temperature |
| `max_tokens` | integer | No | Max response tokens |
| `icon` | string | No | Emoji or icon string |
| `voice` | string | No | TTS voice name (stored as scoped secret) |
| `access` | object\|null | No | Access control config (null to clear) |
| `heartbeat` | object\|null | No | Heartbeat cron config (null to clear) |
| `tools` | object\|null | No | Tool policy (null to clear) |
| `compaction` | object\|null | No | Context compaction config (null to clear) |
| `session` | object\|null | No | Session management config (null to clear) |
| `routing` | array\|null | No | LLM routing rules (null to clear) |
| `approval` | object\|null | No | Human approval config (null to clear) |
| `tool_loop` | object\|null | No | Tool loop config (null to clear) |
| `max_tools_in_context` | integer | No | Max tool definitions injected into context |
| `max_history_messages` | integer | No | Max messages loaded from session history |
| `daily_budget_tokens` | integer | No | Daily token budget cap |

**`access` object:**

| Field | Type | Description |
|-------|------|-------------|
| `mode` | string | `allowlist` or `open` |
| `owner_id` | string | Channel user ID with admin rights |

**`heartbeat` object:**

| Field | Type | Description |
|-------|------|-------------|
| `cron` | string | Cron expression |
| `timezone` | string | IANA timezone (e.g. `UTC`) |
| `announce_to` | string | Channel name to post heartbeat messages to |

**`tools` object:**

| Field | Type | Description |
|-------|------|-------------|
| `allow` | array | Explicitly allowed tool names |
| `deny` | array | Explicitly denied tool names |
| `allow_all` | bool | If true, all tools are allowed by default |
| `deny_all_others` | bool | If true, only `allow` list is permitted |
| `groups.git` | bool | Enable git tool group |
| `groups.tool_management` | bool | Enable tool management tools |
| `groups.skill_editing` | bool | Enable skill editing tools |
| `groups.session_tools` | bool | Enable session management tools |

**`compaction` object:**

| Field | Type | Description |
|-------|------|-------------|
| `enabled` | bool | Enable automatic context compaction |
| `threshold` | integer | Token threshold that triggers compaction |
| `preserve_tool_calls` | bool | Keep tool call/result pairs in summary |
| `preserve_last_n` | integer | Always preserve the last N messages |
| `max_context_tokens` | integer | Hard limit before emergency compaction |

**`approval` object:**

| Field | Type | Description |
|-------|------|-------------|
| `enabled` | bool | Enable human-in-the-loop approval |
| `require_for` | array | Tool names that require approval |
| `require_for_categories` | array | Tool categories requiring approval |
| `timeout_seconds` | integer | Seconds to wait before auto-deny |

**Response:** Full agent detail object (same as `GET /api/agents/{name}`).

### GET /api/agents/{name}

Returns detailed agent configuration. The `config_dirty` flag is `true` when the running config diverges from the on-disk config (e.g. after a file change that has not been reloaded).

### PUT /api/agents/{name}

Update agent configuration. Accepts the same fields as `POST /api/agents`. For nullable fields (`access`, `heartbeat`, `tools`, `compaction`, `session`, `routing`, `approval`, `tool_loop`):
- **Field absent**: existing value is preserved.
- **Explicit `null`**: value is cleared.
- **Value provided**: value is updated.

When the agent name changes (via `PUT` with a different `name`), scoped secrets are automatically migrated.

### DELETE /api/agents/{name}

Stops and removes an agent. The config file is deleted. Returns `{ "ok": true }`.

### POST /api/agents/{name}/model-override

Temporarily override the LLM model for a running agent (in-memory, survives until restart).

**Request body:**
```json
{ "model": "gpt-4o", "provider": "openai" }
```

---

## 4. Chat — OpenAI-Compatible

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/v1/chat/completions` | Required | OpenAI-compatible chat completions |
| `GET` | `/v1/models` | Required | List available models |
| `POST` | `/v1/embeddings` | Required | Proxy embeddings request |

### POST /v1/chat/completions

OpenAI-format chat completions. Supports streaming (`"stream": true`) and non-streaming modes.

**Request body:**

| Field | Type | Description |
|-------|------|-------------|
| `messages` | array | OpenAI-format message array |
| `model` | string | Model name (informational; agent selects actual model) |
| `temperature` | float | Sampling temperature (passed to agent) |
| `stream` | bool | Enable SSE streaming (default: false) |
| `agent` | string | HydeClaw extension: target agent name (defaults to first available) |

**Non-streaming response:**
```json
{
  "id": "chatcmpl-uuid",
  "object": "chat.completion",
  "created": 1711530000,
  "model": "MiniMax-M2.5",
  "choices": [
    {
      "index": 0,
      "message": { "role": "assistant", "content": "Hello!" },
      "finish_reason": "stop"
    }
  ],
  "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 },
  "tools_used": ["memory_search"],
  "iterations": 2
}
```

### GET /v1/models

Returns all available models from all configured LLM providers.

---

## 5. Chat SSE (Native Streaming)

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/chat` | Start a streaming chat session (SSE) |
| `GET` | `/api/chat/{id}/stream` | Resume a stream by stream ID |
| `POST` | `/api/chat/{id}/abort` | Abort an in-progress stream |

### POST /api/chat

Primary chat endpoint. Returns a Server-Sent Events stream compatible with the **Vercel AI SDK v3** (`useChat` hook).

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `agent` | string | Yes | Target agent name |
| `message` | string | Yes | User message text |
| `session_id` | string (UUID) | No | Continue an existing session |
| `channel` | string | No | Source channel identifier (default: `"ui"`) |
| `user_id` | string | No | User identifier for access control |
| `attachments` | array | No | File attachments (see below) |

**Attachment object:**

| Field | Type | Description |
|-------|------|-------------|
| `url` | string | Public URL of the attachment |
| `content_type` | string | MIME type (e.g. `image/jpeg`, `audio/ogg`) |
| `filename` | string | Original filename |

**SSE event types:**

| Event type | Description | Example data |
|------------|-------------|--------------|
| `data-session-id` | First event; contains the session ID | `{ "sessionId": "uuid" }` |
| `start` | Stream begins | `{ "session_id": "uuid", "stream_id": "uuid" }` |
| `text-start` | Text block starting | `{ "id": "block-uuid" }` |
| `text-delta` | Incremental text chunk | `{ "value": "Hello" }` |
| `text-end` | Text block complete | `{}` |
| `tool-input-start` | Tool call starting | `{ "toolCallId": "id", "toolName": "search" }` |
| `tool-input-delta` | Tool arguments streaming | `{ "argsTextDelta": "{\"q\":" }` |
| `tool-input-available` | Full tool call ready | `{ "toolCallId": "id", "toolName": "search", "args": {...} }` |
| `tool-output-available` | Tool result ready | `{ "toolCallId": "id", "result": "..." }` |
| `rich-card` | Structured display card | `{ "card_type": "...", "data": {...} }` |
| `file` | File produced by tool (audio, image) | `{ "url": "...", "mediaType": "audio/ogg" }` |
| `sync` | Message sync (content + tool state) | `{ "content": "...", "toolCalls": [...], "status": "...", "error": null }` |
| `finish` | Stream complete | `{ "usage": {...}, "tools_used": [...] }` |
| `error` | Error during processing | `{ "message": "error text" }` |

### GET /api/chat/{id}/stream

Resume a previously started stream by its `stream_id`. Useful for reconnecting after a dropped connection. Returns the same SSE format.

### POST /api/chat/{id}/abort

Abort an in-progress stream. The agent stops processing and the stream is closed.

**Response:** `{ "ok": true }` or `{ "ok": false, "error": "stream not found" }`

---

## 6. Sessions and Messages

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/sessions` | List sessions |
| `DELETE` | `/api/sessions` | Delete all sessions for an agent or channel |
| `GET` | `/api/sessions/latest` | Get latest session for an agent |
| `GET` | `/api/sessions/search` | Full-text search across messages |
| `PATCH` | `/api/sessions/{id}` | Update session title or UI state |
| `DELETE` | `/api/sessions/{id}` | Delete a session and all its messages |
| `POST` | `/api/sessions/{id}/compact` | Manually compact session history |
| `GET` | `/api/sessions/{id}/export` | Export session as JSON or Markdown |
| `POST` | `/api/sessions/{id}/invite` | Invite an agent into a multi-agent session |
| `POST` | `/api/sessions/{id}/documents` | Upload a document for session-scoped RAG |
| `GET` | `/api/sessions/{id}/messages` | List messages in a session |
| `DELETE` | `/api/messages/{id}` | Delete a single message |
| `PATCH` | `/api/messages/{id}` | Edit a user message |
| `POST` | `/api/messages/{id}/feedback` | Set message feedback (like/dislike) |

### GET /api/sessions

Query parameters:

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `agent` | string | Yes | Filter by agent name |
| `channel` | string | No | Filter by channel (comma-separated) |
| `limit` | integer | No | Max results (default 20, max 100) |

**Response:**
```json
{
  "sessions": [
    {
      "id": "uuid",
      "agent_id": "main",
      "user_id": "12345",
      "channel": "ui",
      "started_at": "2026-03-27T10:00:00Z",
      "last_message_at": "2026-03-27T10:05:00Z",
      "title": "Discussion about X",
      "metadata": {},
      "run_status": "idle"
    }
  ]
}
```

### GET /api/sessions/search

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `q` | string | Required | Search query |
| `agent` | string | `main` | Filter by agent |
| `limit` | integer | 50 | Max results (max 200) |

### PATCH /api/sessions/{id}

**Request body** (all fields optional):
```json
{
  "title": "New session title",
  "ui_state": { "key": "value" }
}
```

`ui_state` is merged into session metadata. Must be a JSON object under 1 KB.

### POST /api/sessions/{id}/compact

Triggers context compaction on a session. The agent summarizes the conversation into facts and replaces older messages with the summary.

**Response:**
```json
{
  "ok": true,
  "facts_extracted": 12,
  "new_message_count": 5
}
```

### GET /api/sessions/{id}/export

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `agent` | string | — | Ownership check |
| `format` | string | `json` | `json` or `markdown` |

Markdown export returns a `Content-Disposition: attachment` response.

### POST /api/sessions/{id}/invite

Invite an agent into a multi-agent session. The invited agent is added to the session's `participants` list.

**Request body:**
```json
{ "agent_name": "Agent2" }
```

**Response:**
```json
{ "ok": true, "participants": ["Agent1", "Agent2"] }
```

Returns `404` if the agent does not exist or the session is not found.

### POST /api/sessions/{id}/documents

Multipart file upload. Chunks the document and embeds it for retrieval-augmented generation scoped to this session. Requires embeddings to be configured.

**Response:**
```json
{
  "filename": "report.txt",
  "chunks": 8,
  "total_chars": 4200
}
```

### GET /api/sessions/{id}/messages

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `limit` | integer | 50 | Max results (max 200) |
| `agent` | string | — | Optional ownership check |

### POST /api/messages/{id}/feedback

**Request body:**
```json
{ "feedback": 1 }
```

Values: `1` = like, `-1` = dislike, `0` = clear.

### PATCH /api/messages/{id}

Edit a user message (role must be `user`).

**Request body:**
```json
{ "content": "Updated message text" }
```

---

## 7. Memory

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/memory` | List memory chunks |
| `POST` | `/api/memory` | Create a memory chunk manually |
| `GET` | `/api/memory/stats` | Memory statistics |
| `GET` | `/api/memory/graph` | Entity relationship graph |
| `GET` | `/api/memory/export` | Export all memory as JSON |
| `GET` | `/api/memory/fts-language` | Get FTS language setting |
| `PUT` | `/api/memory/fts-language` | Set FTS language |
| `DELETE` | `/api/memory/{id}` | Delete a memory chunk |
| `PATCH` | `/api/memory/{id}` | Update a memory chunk |
| `GET` | `/api/memory/tasks` | List memory indexing tasks |
| `GET` | `/api/memory/extraction-queue` | View extraction queue status |
| `GET` | `/api/memory/documents` | List source documents |
| `GET` | `/api/memory/documents/{id}` | Get document details |
| `PATCH` | `/api/memory/documents/{id}` | Update document metadata |
| `DELETE` | `/api/memory/documents/{id}` | Delete document and its chunks |

### GET /api/memory

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `agent` | string | — | Filter by agent |
| `query` | string | — | Semantic search query |
| `limit` | integer | 50 | Max results |
| `pinned` | bool | — | Filter pinned/unpinned chunks |

**Memory chunk object:**
```json
{
  "id": "uuid",
  "agent_id": "main",
  "content": "User prefers concise answers",
  "pinned": false,
  "created_at": "2026-03-27T10:00:00Z",
  "expires_at": null,
  "source": "conversation",
  "document_id": null
}
```

### POST /api/memory

**Request body:**
```json
{
  "agent": "main",
  "content": "User's birthday is March 15",
  "pinned": true
}
```

### GET /api/memory/stats

Returns counts, storage usage, and indexing state.

### GET /api/memory/graph

Returns entity-relationship graph extracted from memory for visualization.

### PUT /api/memory/fts-language

**Request body:**
```json
{ "language": "russian" }
```

Valid values: `simple`, `english`, `russian`, and other PostgreSQL text search configurations.

### PATCH /api/memory/{id}

**Request body** (all fields optional):
```json
{
  "content": "Updated fact text",
  "pinned": true
}
```

---

## 8. Tools and MCP

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/tool-definitions` | List all tool definitions visible to agents |
| `GET` | `/api/tools` | List registered tool services |
| `POST` | `/api/tools` | Register a new tool service |
| `PUT` | `/api/tools/{name}` | Update a tool service |
| `DELETE` | `/api/tools/{name}` | Delete a tool service |
| `GET` | `/api/mcp` | List MCP servers |
| `POST` | `/api/mcp` | Register an MCP server |
| `PUT` | `/api/mcp/{name}` | Update an MCP server |
| `DELETE` | `/api/mcp/{name}` | Delete an MCP server |
| `POST` | `/api/mcp/{name}/reload` | Reload an MCP server |
| `POST` | `/api/mcp/{name}/toggle` | Enable or disable an MCP server |
| `POST` | `/api/mcp/callback` | Internal: MCP OAuth callback |

### GET /api/tool-definitions

Returns the full list of tool definitions that agents can see (built-in + YAML + MCP).

### Tool Service object

A "tool service" is an HTTP-based tool endpoint registered in the tool registry.

```json
{
  "name": "weather",
  "url": "http://localhost:9011/weather",
  "description": "Get current weather",
  "enabled": true
}
```

### MCP Server object

```json
{
  "name": "filesystem",
  "command": "npx",
  "args": ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
  "env": {},
  "enabled": true,
  "status": "running"
}
```

---

## 9. YAML Tools

YAML tools are HTTP-based tool definitions stored as `.yaml` files in `workspace/tools/`. They are shared across all agents.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/yaml-tools` | List all global YAML tools |
| `POST` | `/api/yaml-tools` | Create a new YAML tool |
| `GET` | `/api/yaml-tools/{tool}` | Get YAML tool definition |
| `PUT` | `/api/yaml-tools/{tool}` | Update a YAML tool |
| `DELETE` | `/api/yaml-tools/{tool}` | Delete a YAML tool |
| `POST` | `/api/yaml-tools/{tool}/verify` | Move tool to verified status |
| `POST` | `/api/yaml-tools/{tool}/disable` | Move tool to disabled status |
| `POST` | `/api/yaml-tools/{tool}/enable` | Re-enable a disabled tool |

Per-agent compatibility aliases (same behavior):

| Method | Path |
|--------|------|
| `GET` | `/api/agents/{name}/yaml-tools` |
| `POST` | `/api/agents/{name}/yaml-tools/{tool}/verify` |
| `POST` | `/api/agents/{name}/yaml-tools/{tool}/disable` |

### POST /api/yaml-tools

**Request body:**
```json
{
  "content": "name: get_weather\ndescription: ...\nmethod: GET\nendpoint: ...\n..."
}
```

The `content` field is a YAML string. The tool is created with status `verified`.

**YAML tool format:**
```yaml
name: get_weather
description: Get current weather for a location
method: GET
endpoint: https://api.example.com/weather
parameters:
  - name: location
    type: string
    description: City name
    required: true
auth:
  type: bearer_env
  key: WEATHER_API_KEY
response_transform: "$.current"
```

**Auth types:**

| type | Description |
|------|-------------|
| `bearer_env` | Read API key from environment variable named by `key` |
| `none` | No authentication |

**Tool statuses:**

| Status | Location | Description |
|--------|----------|-------------|
| `verified` | `workspace/tools/*.yaml` | Active, available to agents |
| `draft` | `workspace/tools/draft/*.yaml` | Work-in-progress, not yet active |
| `disabled` | `workspace/tools/disabled/*.yaml` | Archived, not available |

---

## 10. Skills

Skills are Markdown files stored in `workspace/skills/`. They are shared prompt fragments injected into agent context.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/skills` | List all global skills |
| `GET` | `/api/skills/{skill}` | Get skill content |
| `PUT` | `/api/skills/{skill}` | Create or update a skill |
| `DELETE` | `/api/skills/{skill}` | Delete a skill |

Per-agent compatibility aliases:

| Method | Path |
|--------|------|
| `GET` | `/api/agents/{name}/skills` |
| `GET` | `/api/agents/{name}/skills/{skill}` |
| `PUT` | `/api/agents/{name}/skills/{skill}` |
| `DELETE` | `/api/agents/{name}/skills/{skill}` |

### PUT /api/skills/{skill}

**Request body:**
```json
{
  "content": "# Web Search Strategy\n\nUse SearXNG for general queries..."
}
```

---

## 11. Channels

Channels connect agents to messaging platforms (Telegram, Discord, etc.). Each channel is registered per-agent and managed via an external channel adapter that connects over WebSocket.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/channels` | List all channels across all agents |
| `GET` | `/api/channels/active` | List currently connected channel adapters |
| `POST` | `/api/channels/notify` | Send a direct notification via a channel |
| `GET` | `/api/agents/{name}/channels` | List channels for an agent |
| `POST` | `/api/agents/{name}/channels` | Create a channel for an agent |
| `PUT` | `/api/agents/{name}/channels/{id}` | Update a channel |
| `DELETE` | `/api/agents/{name}/channels/{id}` | Delete a channel |
| `POST` | `/api/agents/{name}/channels/{id}/restart` | Restart a channel adapter |
| `POST` | `/api/agents/{name}/channels/{id}/ack` | Acknowledge a channel error |
| `GET` | `/api/agents/{name}/channels/{id}/status` | Get channel status |
| `GET` | `/ws/channel/{agent_name}` | WebSocket endpoint for channel adapters |

### Channel object

```json
{
  "id": "uuid",
  "agent_name": "main",
  "channel_type": "telegram",
  "display_name": "My Bot",
  "config": {},
  "status": "running",
  "error_msg": null
}
```

### POST /api/agents/{name}/channels

**Supported channel types:** `telegram`, `discord`, `matrix`, `irc`, `slack`, `whatsapp`

**Request body:**
```json
{
  "channel_type": "telegram",
  "display_name": "My Bot",
  "config": {
    "bot_token": "5092435297:AAH..."
  }
}
```

Credential fields (`bot_token`, `access_token`, `password`, `app_token`, `verify_token`) are automatically extracted from `config` and stored in the secrets vault. The returned `config` object has these fields redacted.

**Response:**
```json
{ "ok": true, "id": "uuid", "status": "stopped" }
```

### Channel Credential Flow

1. `POST /api/agents/{name}/channels` with credentials in `config` JSON.
2. Core extracts credential fields, stores them as a scoped secret keyed by channel UUID.
3. When the channel adapter connects via `/ws/channel/{agent_name}`, Core retrieves credentials from vault and passes them to the adapter.
4. Channel credentials are never returned in plain text after creation.

### POST /api/channels/notify

Send a notification message through a specific channel without going through the agent's LLM.

**Request body:**
```json
{
  "channel_id": "uuid",
  "text": "Notification message",
  "parse_mode": "MarkdownV2"
}
```

### Channel Adapter WebSocket: /ws/channel/{agent_name}

External channel adapters connect here. Authentication options:
- **Bearer token** via `Authorization` header.
- **WS ticket** via `?ticket=<uuid>` query parameter (obtain from `POST /api/auth/ws-ticket`).

The adapter sends `ChannelInbound` JSON messages and receives `ChannelOutbound` JSON messages.

---

## 12. Cron Jobs

Cron jobs run agent tasks on a schedule.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/cron` | List all cron jobs |
| `POST` | `/api/cron` | Create a cron job |
| `PUT` | `/api/cron/{id}` | Update a cron job |
| `DELETE` | `/api/cron/{id}` | Delete a cron job |
| `POST` | `/api/cron/{id}/run` | Trigger a cron job immediately |
| `GET` | `/api/cron/{id}/runs` | Get run history for a job |
| `GET` | `/api/cron/runs` | Get run history for all jobs |

### GET /api/cron

**Response:**
```json
{
  "jobs": [
    {
      "id": "uuid",
      "agent": "main",
      "name": "morning-briefing",
      "cron": "0 9 * * *",
      "timezone": "UTC",
      "task": "Prepare daily briefing",
      "enabled": true,
      "silent": false,
      "announce_to": "telegram",
      "jitter_secs": 0,
      "run_once": false,
      "run_at": null,
      "created_at": "2026-01-01T00:00:00Z",
      "last_run": "2026-03-27T06:00:00Z",
      "next_run": "2026-03-28T06:00:00Z"
    }
  ]
}
```

### POST /api/cron

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique job name |
| `agent` | string | Yes | Target agent name |
| `task` | string | Yes | Task message sent to the agent |
| `cron` | string | Conditional | Cron expression (required unless `run_once`) |
| `timezone` | string | No | IANA timezone (default: `UTC`) |
| `announce_to` | string/object | No | Channel to send output to |
| `silent` | bool | No | If true, discard agent output (default: false) |
| `jitter_secs` | integer | No | Random delay added to execution time |
| `run_once` | bool | No | One-shot job (requires `run_at`) |
| `run_at` | datetime | Conditional | ISO 8601 datetime for one-shot jobs |

### PUT /api/cron/{id}

Accepts the same fields as `POST /api/cron`. All fields are optional (patch semantics).

---

## 13. Tasks

Tasks are multi-step execution pipelines tracked in the database.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/tasks` | List tasks |
| `POST` | `/api/tasks` | Create a task |
| `GET` | `/api/tasks/{id}` | Get a task |
| `DELETE` | `/api/tasks/{id}` | Delete a task |
| `GET` | `/api/tasks/{id}/steps` | Get task execution steps |
| `GET` | `/api/tasks/audit` | List tool execution audit log |

### GET /api/tasks

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `agent` | string | `main` | Filter by agent |
| `limit` | integer | 50 | Max results (max 200) |

### POST /api/tasks

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `agent` | string | Yes | Target agent name |
| `input` | string | Yes | Task description/prompt |
| `source` | string | No | Source identifier (default: `"api"`) |

**Response:**
```json
{ "ok": true, "task_id": "uuid" }
```

### GET /api/tasks/{id}/steps

Returns ordered list of execution steps with their outputs and status.

### GET /api/tasks/audit

Returns tool execution audit log entries.

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `agent` | string | — | Filter by agent name |
| `status` | string | — | Filter by status |
| `limit` | integer | 50 | Max results (max 200) |

**Response:**
```json
[
  {
    "id": "uuid",
    "agent_id": "agent-name",
    "session_id": "uuid",
    "tool_name": "workspace_write",
    "parameters": { "...": "..." },
    "status": "ok",
    "duration_ms": 42,
    "error": null,
    "created_at": "2025-01-01T00:00:00Z"
  }
]
```

---

## 14. Approvals

Human-in-the-loop approval for sensitive tool calls.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/approvals` | List pending approvals |
| `POST` | `/api/approvals/{id}/resolve` | Approve or deny a pending action |
| `GET` | `/api/approvals/allowlist` | List auto-approved tools |
| `POST` | `/api/approvals/allowlist` | Add a tool to the allowlist |
| `DELETE` | `/api/approvals/allowlist/{id}` | Remove from allowlist |

### POST /api/approvals/{id}/resolve

**Request body:**
```json
{ "decision": "approve" }
```

Values: `"approve"` or `"deny"`.

### POST /api/approvals/allowlist

**Request body:**
```json
{ "tool_name": "workspace_write", "agent": "main" }
```

---

## 15. Webhooks

Webhooks let external systems trigger agent processing via HTTP POST.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/webhooks` | List all webhooks |
| `POST` | `/api/webhooks` | Create a webhook |
| `PUT` | `/api/webhooks/{id}` | Update a webhook |
| `DELETE` | `/api/webhooks/{id}` | Delete a webhook |
| `POST` | `/api/webhooks/{id}/regenerate-secret` | Regenerate webhook secret |
| `POST` | `/webhook/{name}` | Trigger endpoint (Public, verified by secret) |

### GET /api/webhooks

**Response:**
```json
{
  "webhooks": [
    {
      "id": "uuid",
      "name": "github-push",
      "agent_id": "main",
      "secret": "****...abcd",
      "prompt_prefix": "New GitHub event:",
      "enabled": true,
      "created_at": "2026-01-01T00:00:00Z",
      "last_triggered_at": "2026-03-27T10:00:00Z",
      "trigger_count": 42,
      "webhook_type": "github",
      "event_filter": ["push", "pull_request"]
    }
  ]
}
```

Note: The `secret` field is masked (last 4 characters visible). The full secret is returned only at creation time.

### POST /api/webhooks

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique webhook name (used in trigger URL) |
| `agent` | string | Yes | Target agent name |
| `prompt_prefix` | string | No | Text prepended to the payload before sending to agent |
| `enabled` | bool | No | Default: `true` |
| `webhook_type` | string | No | `generic` (default) or `github` |
| `event_filter` | array | No | For GitHub webhooks: list of event types to process (e.g. `["push", "pull_request"]`) |

**Response:** `201 Created` with webhook object including the **full secret** (only visible at creation).

### POST /api/webhooks/{id}/regenerate-secret

Generates a new random secret for the webhook. The old secret is immediately invalidated.

**Response:**
```json
{ "ok": true, "secret": "new-64-char-hex-string" }
```

### POST /webhook/{name}

Trigger endpoint called by external systems. Not behind the standard auth middleware — instead authenticated by the webhook secret.

**Webhook types:**

| Type | Auth method | Payload handling |
|------|-------------|-----------------|
| `generic` | `Authorization: Bearer <secret>` | JSON body pretty-printed and sent to agent |
| `github` | `X-Hub-Signature-256: sha256=<hmac>` | GitHub event parsed and summarized before sending to agent |

**Query parameters:**

| Parameter | Description |
|-----------|-------------|
| `async=true` | Return immediately; process payload in background |

**Rate limiting:** 5 auth failures within 5 minutes locks the webhook for 10 minutes.

**Response (synchronous):**
```json
{ "ok": true, "response": "Agent response text" }
```

**Response (async):**
```json
{ "ok": true, "queued": true }
```

---

## 16. Secrets

The secrets vault stores credentials and API keys. Secrets have a name and an optional scope. Scoped secrets take priority over global ones during lookup.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/secrets` | List all secrets (values masked) |
| `POST` | `/api/secrets` | Create or update a secret |
| `GET` | `/api/secrets/{name}` | Get a secret |
| `DELETE` | `/api/secrets/{name}` | Delete a secret |

### GET /api/secrets

Returns metadata for all secrets. Values are never returned in the list view.

### POST /api/secrets

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Secret name (A-Z, a-z, 0-9, `_`, max 128 chars) |
| `value` | string | Conditional | Secret value (required unless updating description only) |
| `description` | string | No | Human-readable description |
| `scope` | string | No | Agent name for per-agent secrets; omit or empty for global |

**Scoped secret lookup order:** `(name, scope)` → `(name, "")` (global) → environment variable fallback.

### GET /api/secrets/{name}

Query parameters:

| Parameter | Type | Description |
|-----------|------|-------------|
| `scope` | string | Agent scope (empty for global) |
| `reveal` | bool | Return plaintext value (default: false) |

---

## 17. Config

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/config` | Get gateway configuration |
| `PUT` | `/api/config` | Update gateway configuration |
| `GET` | `/api/config/export` | Export full config as JSON |
| `POST` | `/api/config/import` | Import config from JSON |
| `POST` | `/api/restart` | Restart the gateway process |

### GET /api/config

Returns the current gateway configuration (from `config/hydeclaw.toml`). Sensitive fields are masked.

### PUT /api/config

Update gateway config. Changes are written to disk and applied live where possible. Some settings require a restart.

### POST /api/restart

Signals the process to exit (systemd or the watchdog will restart it). Returns immediately.

**Response:** `{ "ok": true, "message": "restarting..." }`

---

## 18. Backup and Restore

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/backup` | List available backups |
| `POST` | `/api/backup` | Create a new backup |
| `GET` | `/api/backup/{filename}` | Download a backup file |
| `DELETE` | `/api/backup/{filename}` | Delete a backup file |
| `POST` | `/api/restore` | Restore from a backup |

### POST /api/backup

Creates a full backup of the database (agents, sessions, messages, memory, cron jobs, secrets, webhooks, channels, LLM providers) and writes it to the `backups/` directory.

**Response:**
```json
{
  "ok": true,
  "filename": "hydeclaw-backup-2026-03-27T10-00-00Z.json",
  "path": "backups/hydeclaw-backup-2026-03-27T10-00-00Z.json"
}
```

### Backup file format

The backup is a JSON file with top-level keys:

```json
{
  "version": "1",
  "created_at": "2026-03-27T10:00:00Z",
  "agents": [...],
  "sessions": [...],
  "messages": [...],
  "memory_chunks": [...],
  "scheduled_jobs": [...],
  "secrets": [...],
  "webhooks": [...],
  "agent_channels": [...],
  "providers": [...]
}
```

### POST /api/restore

**Request body:**
```json
{ "filename": "hydeclaw-backup-2026-03-27T10-00-00Z.json" }
```

Restores data from the named backup file. Existing data in the target tables is replaced.

---

## 19. Services

Manage Docker services and native child processes controlled by HydeClaw.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/services` | List all managed services |
| `POST` | `/api/services/{name}/{action}` | Perform an action on a service |
| `POST` | `/api/containers/{name}/restart` | Restart a Docker container |

### GET /api/services

Returns all Docker containers visible on the host plus natively managed processes (toolgate, channel adapter).

### POST /api/services/{name}/{action}

**Actions:**

| Action | Description |
|--------|-------------|
| `restart` | Stop and restart the service |
| `rebuild` | Rebuild Docker image and restart |
| `start` | Start a stopped service |
| `stop` | Stop a running service |
| `status` | Get service status |
| `logs` | Retrieve recent logs |

For **natively managed processes** (non-Docker), only `restart`, `start`, `stop`, and `status` are supported.

---

## 20. Watchdog

The watchdog is a separate binary (`hydeclaw-watchdog`) that monitors services and can restart them automatically. Status is communicated via a JSON file at `/tmp/hydeclaw-watchdog.json`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/watchdog/status` | Current watchdog status |
| `GET` | `/api/watchdog/config` | Read watchdog TOML config |
| `PUT` | `/api/watchdog/config` | Update watchdog TOML config |
| `GET` | `/api/watchdog/settings` | Read alerting settings |
| `PUT` | `/api/watchdog/settings` | Update alerting settings |
| `POST` | `/api/watchdog/restart/{name}` | Execute the restart command for a check |

### GET /api/watchdog/status

Returns the content of `/tmp/hydeclaw-watchdog.json`. Returns `{ "error": "watchdog not running" }` if the watchdog process is not active.

### PUT /api/watchdog/config

**Request body:**
```json
{ "config": "# TOML content\n[global]\n..." }
```

Config is validated as valid TOML before saving.

### PUT /api/watchdog/settings

Updatable keys:

| Key | Type | Description |
|-----|------|-------------|
| `alert_channel_ids` | array | Channel UUIDs to send alerts to |
| `alert_events` | array | Event types that trigger alerts |

---

## 21. Providers

All providers (LLM and media) share the unified `/api/providers` endpoint. LLM and media providers are distinguished by their `kind` field (`"llm"` or `"media"`).

### LLM Providers

Named LLM provider connections allow configuring multiple providers with their API keys.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/provider-types` | List supported provider types |
| `GET` | `/api/providers` | List configured providers (filter by `kind` query param) |
| `POST` | `/api/providers` | Create a provider |
| `GET` | `/api/providers/{id}` | Get a provider |
| `PUT` | `/api/providers/{id}` | Update a provider |
| `DELETE` | `/api/providers/{id}` | Delete a provider |
| `GET` | `/api/providers/{id}/models` | List models from this provider |
| `GET` | `/api/providers/{id}/resolve` | Resolve connection details |

### GET /api/provider-types

**Response:**
```json
{
  "provider_types": [
    {
      "id": "openai",
      "name": "OpenAI",
      "default_base_url": "https://api.openai.com/v1",
      "chat_path": "/chat/completions",
      "default_secret_name": "OPENAI_API_KEY",
      "requires_api_key": true,
      "supports_model_listing": true
    }
  ]
}
```

### POST /api/providers (LLM)

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Human-readable connection name |
| `kind` | string | Yes | `"llm"` for LLM providers |
| `provider_type` | string | Yes | Provider type ID (e.g. `openai`, `anthropic`, `minimax`) |
| `base_url` | string | No | Override base URL |
| `api_key` | string | No | API key (stored in vault, masked in responses) |
| `default_model` | string | No | Default model for this connection |
| `notes` | string | No | Internal notes |

**Response:** Provider object with `has_api_key: true/false` and masked `api_key`.

### LLM Provider object

```json
{
  "id": "uuid",
  "name": "MiniMax Production",
  "kind": "llm",
  "provider_type": "minimax",
  "base_url": "https://api.minimax.io/v1",
  "api_key": "****...xyz9",
  "has_api_key": true,
  "default_model": "MiniMax-M2.5",
  "notes": "",
  "created_at": "2026-01-01T00:00:00Z",
  "updated_at": "2026-03-27T10:00:00Z"
}
```

### Media Providers

Media providers handle STT (speech-to-text), TTS (text-to-speech), vision, image generation, and embedding capabilities. They use the same `/api/providers` endpoint as LLM providers, with `kind` set to `"media"`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/providers` | List providers (use `?kind=media` to filter) |
| `POST` | `/api/providers` | Create a media provider |
| `GET` | `/api/providers/{id}` | Get a media provider |
| `PUT` | `/api/providers/{id}` | Update a media provider |
| `DELETE` | `/api/providers/{id}` | Delete a media provider |
| `GET` | `/api/provider-active` | Get currently active provider per capability |
| `PUT` | `/api/provider-active` | Set the active provider for a capability |
| `GET` | `/api/media-drivers` | List available driver types |
| `GET` | `/api/media-config` | Export toolgate-compatible media config |

### Capabilities

| Capability | Description |
|------------|-------------|
| `stt` | Speech-to-text transcription |
| `tts` | Text-to-speech synthesis |
| `vision` | Image description / visual understanding |
| `imagegen` | Image generation |
| `embedding` | Text embeddings for semantic search |

### Media Provider object

```json
{
  "id": "whisper-local",
  "kind": "media",
  "type": "stt",
  "driver": "openai",
  "base_url": "http://localhost:8300/v1",
  "model": "Systran/faster-whisper-large-v3",
  "api_key": null,
  "has_api_key": false,
  "notes": "Local faster-whisper"
}
```

### PUT /api/provider-active

**Request body:**
```json
{
  "stt": "whisper-local",
  "tts": "qwen3-tts-voice",
  "vision": "qwen35-local"
}
```

Omit a capability to leave its active provider unchanged.

---

## 22. TTS

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/tts/voices` | List available TTS voices |
| `POST` | `/api/tts/synthesize` | Synthesize speech |

### POST /api/tts/synthesize

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `text` | string | Yes | Text to synthesize |
| `voice` | string | No | Voice name or clone identifier (e.g. `clone:MyVoice`) |

**Response:** Audio binary with appropriate `Content-Type` header (`audio/mpeg`, `audio/ogg`, etc.), or JSON error.

---

## 23. Media Upload

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `POST` | `/api/media/upload` | Required | Upload a file (max 20 MB) |
| `GET` | `/uploads/{filename}` | Public | Serve an uploaded file |

### POST /api/media/upload

Multipart form upload. The file is saved to `workspace/uploads/{uuid}.{ext}`.

**Allowed extensions:** `jpg`, `jpeg`, `png`, `gif`, `webp`, `bmp`, `ico`, `mp4`, `webm`, `mov`, `avi`, `ogg`, `oga`, `mp3`, `wav`, `flac`, `aac`, `m4a`, `pdf`, `docx`, `xlsx`, `pptx`, `txt`, `md`, `csv`, `log`, `json`, `toml`, `yaml`, `yml`, `zip`, `tar`, `gz`, `bin`. Other extensions are saved with the `.bin` extension.

**Response:**
```json
{
  "url": "http://host:18789/uploads/uuid.jpg",
  "filename": "uuid.jpg",
  "size": 204800
}
```

### GET /uploads/{filename}

Public — no auth required. Serves the file with appropriate `Content-Type`. Returns `400 Bad Request` if the filename contains path traversal sequences (`..`, `/`, `\`).

---

## 24. Workspace

Browse, read, write, and delete files within the `workspace/` directory.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/workspace` | List workspace root |
| `GET` | `/api/workspace/{path}` | List directory or read file |
| `PUT` | `/api/workspace/{path}` | Write a file |
| `DELETE` | `/api/workspace/{path}` | Delete a file |

All paths are strictly sandboxed within the `workspace/` directory. Path traversal (e.g. `../`) is rejected with `403 Forbidden`.

### GET /api/workspace/{path}

For **directories**: returns a JSON listing.
```json
{
  "entries": [
    { "name": "tools", "is_dir": true, "display": "tools/ (4.2 KB)" },
    { "name": "notes.md", "is_dir": false, "display": "notes.md (1.2 KB)" }
  ]
}
```

For **files**: returns the raw file content with appropriate `Content-Type`.

### PUT /api/workspace/{path}

**Request body:** Raw file content (any content type). Parent directories are created automatically.

---

## 25. Canvas

The canvas stores ephemeral shared state (e.g. a document being collaboratively edited) per agent.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/canvas/{agent}` | Get current canvas state |
| `DELETE` | `/api/canvas/{agent}` | Clear canvas state |

### GET /api/canvas/{agent}

**Response:**
```json
{
  "agent": "main",
  "content": "# Current canvas content\n...",
  "updated_at": "2026-03-27T10:00:00Z"
}
```

---

## 26. OAuth

OAuth 2.0 integration for connecting agent tools to external services (Google, GitHub, etc.).

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/api/oauth/callback` | Public | OAuth callback (called by provider) |
| `GET` | `/api/oauth/providers` | Required | List supported OAuth providers |
| `GET` | `/api/oauth/accounts` | Required | List configured OAuth accounts |
| `POST` | `/api/oauth/accounts` | Required | Create an OAuth account |
| `DELETE` | `/api/oauth/accounts/{id}` | Required | Delete an OAuth account |
| `POST` | `/api/oauth/accounts/{id}/connect` | Required | Initiate OAuth authorization flow |
| `POST` | `/api/oauth/accounts/{id}/revoke` | Required | Revoke OAuth tokens |
| `GET` | `/api/agents/{name}/oauth/bindings` | Required | List agent OAuth bindings |
| `POST` | `/api/agents/{name}/oauth/bindings` | Required | Bind an OAuth account to an agent |
| `DELETE` | `/api/agents/{name}/oauth/bindings/{provider}` | Required | Remove an OAuth binding |

### POST /api/oauth/accounts

**Request body:**
```json
{
  "provider": "google",
  "display_name": "Work Google Account",
  "client_id": "xxx.apps.googleusercontent.com",
  "client_secret": "GOCSPX-..."
}
```

### POST /api/oauth/accounts/{id}/connect

Generates an authorization URL. Redirect the user to this URL to complete OAuth.

**Response:**
```json
{ "auth_url": "https://accounts.google.com/o/oauth2/auth?..." }
```

### POST /api/agents/{name}/oauth/bindings

**Request body:**
```json
{ "account_id": "uuid" }
```

---

## 27. Access / Pairing

Agents with `access.mode: allowlist` require users to be approved before they can chat. The pairing flow works via a 6-character code sent by the user in the channel.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/access/{agent}/pending` | List pending pairing requests |
| `POST` | `/api/access/{agent}/approve/{code}` | Approve a pairing request |
| `POST` | `/api/access/{agent}/reject/{code}` | Reject a pairing request |
| `GET` | `/api/access/{agent}/users` | List approved users |
| `DELETE` | `/api/access/{agent}/users/{user_id}` | Remove a user from the allowlist |

### Pairing flow

1. User sends `/start` or their pairing code to the bot.
2. Core creates a pending pairing entry with a 6-character code.
3. Admin calls `POST /api/access/{agent}/approve/{code}` (or rejects).
4. On approval, the user is added to `allowed_users` and can chat freely.

### GET /api/access/{agent}/pending

**Response:**
```json
{
  "pending": [
    {
      "code": "ABC123",
      "user_id": "123456789",
      "channel": "telegram",
      "created_at": "2026-03-27T10:00:00Z"
    }
  ]
}
```

### GET /api/access/{agent}/users

**Response:**
```json
{
  "users": [
    {
      "channel_user_id": "123456789",
      "display_name": "User",
      "approved_at": "2026-01-15T12:00:00Z"
    }
  ]
}
```

---

## 28. WebSocket (UI Events)

| Path | Auth | Description |
|------|------|-------------|
| `GET /ws` | Ticket or Bearer | UI real-time event stream |

The WebSocket connection at `/ws` delivers real-time events to the UI. Authentication is via:
- `?ticket=<uuid>` query parameter (from `POST /api/auth/ws-ticket`)
- `Authorization: Bearer <token>` header on the upgrade request

### UI event types

| Event | Description |
|-------|-------------|
| `agent_processing` | Agent started/stopped processing a request |
| `session_updated` | Session metadata or messages changed |
| `cron_completed` | A scheduled cron job finished |
| `task_updated` | Task status changed |
| `approval_pending` | New tool approval request |
| `approval_resolved` | Approval was resolved |
| `channel_status` | Channel adapter connected/disconnected |
| `memory_updated` | Memory chunk created or updated |
| `log` | Real-time log line (debug/info/warn/error) |

---

## 29. Email Triggers (Gmail)

Gmail Pub/Sub push triggers notify the agent when new emails arrive in a connected Gmail account.

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/api/triggers/email` | Required | List Gmail triggers |
| `POST` | `/api/triggers/email` | Required | Create a Gmail trigger |
| `DELETE` | `/api/triggers/email/{id}` | Required | Delete a Gmail trigger |
| `POST` | `/api/triggers/email/push` | Public | Gmail Pub/Sub push endpoint |

### POST /api/triggers/email

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `agent` | string | Yes | Target agent name |
| `oauth_account_id` | string | Yes | UUID of connected Google OAuth account |
| `label_filter` | array | No | Gmail label IDs to filter (e.g. `["INBOX"]`) |
| `prompt_prefix` | string | No | Text prepended to email content |

The Gmail watch subscription is automatically registered with Google Pub/Sub.

### POST /api/triggers/email/push

Called by Google Pub/Sub when new mail arrives. Public endpoint — Google does not include auth tokens. Validates the Pub/Sub message signature internally.

---

## 30. GitHub Integration

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/agents/{name}/github/repos` | List allowed GitHub repos for an agent |
| `POST` | `/api/agents/{name}/github/repos` | Add a GitHub repo to the allowlist |
| `DELETE` | `/api/agents/{name}/github/repos/{id}` | Remove a repo from the allowlist |

The GitHub repo allowlist controls which repositories an agent's GitHub tools can access. Used in combination with GitHub webhook integrations.

### POST /api/agents/{name}/github/repos

**Request body:**
```json
{
  "owner": "octocat",
  "repo": "hello-world"
}
```

---

## 31. Audit Log

See [Monitoring](#2-monitoring) for audit endpoints. Event types include:

| Event type | Description |
|------------|-------------|
| `secret_created` | Secret was created or updated |
| `agent_started` | Agent engine started |
| `agent_stopped` | Agent engine stopped |
| `access_approved` | User pairing approved |
| `access_rejected` | User pairing rejected |
| `webhook_triggered` | Webhook received a payload |
| `tool_called` | Agent invoked a tool |
| `approval_resolved` | Human approval decision made |
| `config_updated` | Gateway config changed |
| `backup_created` | Backup snapshot taken |
| `restore_completed` | Backup restored |

---

## Error Responses

All error responses use a consistent JSON format:

```json
{ "error": "human-readable error message" }
```

**Common HTTP status codes:**

| Status | Meaning |
|--------|---------|
| `400` | Bad request — missing or invalid parameters |
| `401` | Unauthorized — missing or invalid Bearer token |
| `403` | Forbidden — path traversal or ownership mismatch |
| `404` | Not found — resource does not exist |
| `409` | Conflict — resource already exists |
| `413` | Payload too large — file upload exceeds 20 MB |
| `429` | Too many requests — rate limit exceeded |
| `500` | Internal server error |
| `503` | Service unavailable — dependency (embeddings, etc.) not configured |

---

## Notes

### LLM Retry Policy

Failed LLM calls are retried up to 3 times with exponential backoff: 1s → 3s → 9s. Retries are triggered on HTTP status codes `429`, `500`, `502`, and `503`.

### Tool Concurrency

Tool call concurrency is controlled by an in-memory `tokio::Semaphore`. There is no database-level queue.

### Secrets Resolution Order

For any secret name and agent scope: `(name, scope)` → `(name, "")` global → environment variable.

### CORS

CORS origins are configured via `gateway.cors_origins`. If empty, the gateway allows the UI port (`:5173`) and API port on the same host.

---

## 32. Setup

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/setup/status` | Check whether initial setup has been completed |
| `GET` | `/api/setup/requirements` | List prerequisites and their current status |
| `POST` | `/api/setup/complete` | Mark setup as complete after configuration |

### GET /api/setup/status

Returns whether the instance has completed first-run setup.

**Response:**
```json
{
  "setup_complete": false,
  "missing_steps": ["provider", "agent"]
}
```

### GET /api/setup/requirements

Returns a checklist of prerequisites (database, Docker, secrets, providers) with pass/fail status for each.

**Response:**
```json
{
  "requirements": [
    { "name": "database", "ok": true, "message": "PostgreSQL 17 reachable" },
    { "name": "master_key", "ok": true, "message": "HYDECLAW_MASTER_KEY set" },
    { "name": "provider", "ok": false, "message": "No LLM provider configured" },
    { "name": "agent", "ok": false, "message": "No agents created" }
  ]
}
```

### POST /api/setup/complete

Marks the instance as fully configured. Subsequent calls to `GET /api/setup/status` will return `"setup_complete": true`. Idempotent.

**Request body:**
```json
{ "provider": "openai", "model": "gpt-4o-mini", "agent_name": "assistant" }
```

**Response:** `{ "ok": true }`

---

## 33. Network

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/network/addresses` | List detected LAN addresses for this host |

### GET /api/network/addresses

Returns all non-loopback IP addresses detected on the host, useful for displaying access URLs in the UI or during setup.

**Response:**
```json
{
  "addresses": [
    { "ip": "192.168.1.85", "interface": "eth0", "family": "ipv4" },
    { "ip": "fe80::1", "interface": "eth0", "family": "ipv6" }
  ],
  "port": 18789
}
```

---

## 34. Notifications

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/notifications` | List notifications for the current user |
| `PATCH` | `/api/notifications/{id}` | Update a notification (e.g. mark as read) |
| `POST` | `/api/notifications/read-all` | Mark all notifications as read |
| `DELETE` | `/api/notifications/clear` | Delete all read notifications |

### GET /api/notifications

Query parameters:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `unread` | boolean | — | Filter to unread only |
| `limit` | integer | 50 | Max results (max 200) |
| `offset` | integer | 0 | Pagination offset |

**Response:**
```json
{
  "notifications": [
    {
      "id": "uuid",
      "type": "agent_error",
      "title": "Agent failed",
      "body": "Provider returned 401 for agent 'analyst'",
      "read": false,
      "created_at": "2026-04-06T12:00:00Z"
    }
  ],
  "unread_count": 3
}
```

### PATCH /api/notifications/{id}

**Request body:**
```json
{ "read": true }
```

**Response:** `{ "ok": true }`

### POST /api/notifications/read-all

Marks all unread notifications as read. Returns the number of notifications updated.

**Response:** `{ "ok": true, "updated": 5 }`

### DELETE /api/notifications/clear

Deletes all notifications that have been read. Unread notifications are preserved.

**Response:** `{ "ok": true, "deleted": 12 }`

---

## 35. Config Schema

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/config/schema` | Get the JSON schema for gateway configuration |

### GET /api/config/schema

Returns the JSON schema describing all valid fields for `config/hydeclaw.toml`. Useful for UI-driven config editors and client-side validation.

**Response:**
```json
{
  "type": "object",
  "properties": {
    "gateway": {
      "type": "object",
      "properties": {
        "listen": { "type": "string", "default": "0.0.0.0:18789" },
        "cors_origins": { "type": "array", "items": { "type": "string" } }
      }
    },
    "limits": {
      "type": "object",
      "properties": {
        "max_requests_per_minute": { "type": "integer", "default": 100 }
      }
    }
  }
}
```
