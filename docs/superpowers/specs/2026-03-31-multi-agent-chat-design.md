# Multi-Agent Chat — Design Spec

**Date:** 2026-03-31
**Status:** Draft
**Author:** Claude + Author

## Problem

Current architecture requires agents to communicate through `send_to_agent` — a synchronous, opaque tool call where agent B processes in an isolated session invisible to the user. Agent A acts as intermediary, often misinterpreting requests. The user has no visibility into what agent B thinks or does.

## Solution

Session-centric multi-agent chat. Any session can have multiple agent participants. Users and agents can invite other agents into the conversation. All messages are visible to everyone in the session. Canvas is per-session, not per-agent.

## Core Concepts

### Session as the central object

- A session starts with one agent (owner) — same as today
- Additional agents join via `invite_agent` tool (by agent) or `@AgentName` (by user)
- Session stores `participants: [agent_name, ...]` — ordered, first = owner
- All participants share the same message history
- Canvas is bound to session, not to agent

### Message routing

| Input | Routed to |
|-------|-----------|
| `@Arty check portfolio` | Arty (explicit mention) |
| `check portfolio` (no @) | Owner agent |
| `@Architect @Arty both of you check` | First mentioned agent (Architect) |
| Agent calls `invite_agent("Arty")` | System message "Arty joined", Arty added to participants |

### Agent context when responding

Each agent, when addressed:
- Uses its own SOUL.md / IDENTITY.md / system prompt
- Uses its own tool set (not the owner's)
- Receives full session message history as context
- Streams response to the same SSE connection
- Messages tagged with `agent_id` in DB (already exists in schema)

### Agent-to-agent communication

Agents CAN address each other within the chat:
- Agent A responds and includes `@Arty can you verify this?`
- Core detects the mention, queues Arty to respond next
- Arty receives the full history including A's latest message
- This creates a natural conversation flow visible to the user

To prevent infinite loops:
- Max agent-to-agent turns per user message: 5 (configurable)
- Agent cannot @-mention itself
- If turn limit hit, Core sends system message "Turn limit reached" and waits for user input

## Database Changes

### sessions table

Add column:
```sql
ALTER TABLE sessions ADD COLUMN participants TEXT[] DEFAULT '{}';
```

`participants[0]` = owner agent. Empty array = legacy single-agent session (backward compatible — treat `agent_id` as sole participant).

### messages table

`agent_id` column already exists. Currently nullable. For multi-agent sessions, every assistant message MUST have `agent_id` set.

No other schema changes needed.

## Backend Changes

### New tool: `invite_agent`

Available to all agents (not just base). Adds target agent to session participants.

```rust
// Tool definition
name: "invite_agent"
description: "Invite another agent into this chat session"
parameters: { agent_name: string (required) }
```

Behavior:
1. Validate agent exists
2. Add to session participants (if not already present)
3. Emit system message: "{agent_name} joined the conversation"
4. Return confirmation

### Message routing in `handle_sse`

After receiving user message:
1. Parse `@AgentName` mentions from message text
2. If mentioned agent is in participants → route to that agent
3. If mentioned agent is NOT in participants → auto-invite, then route
4. If no mention → route to owner agent
5. After agent responds, check response for `@AgentName` mentions → queue next agent

### Agent response with mentions

After agent A finishes responding:
1. Parse A's response for `@AgentName` patterns
2. If found and within turn limit → automatically process next agent
3. Next agent receives full updated history (including A's response)
4. Repeat until no more mentions or turn limit reached

### SSE events

Existing events work — each already supports distinguishing agent:

```
event: start
data: {"agentName": "Arty", "messageId": "..."}

event: text-delta
data: {"agentName": "Arty", "delta": "Checking..."}
```

Add `agentName` field to all SSE events. UI uses this to render correct avatar/name.

New events:
```
event: agent-joined
data: {"agentName": "Arty", "sessionId": "..."}

event: agent-turn
data: {"agentName": "Arty", "reason": "mentioned by Architect"}
```

### Deprecate `send_to_agent`

Phase out gradually:
1. Add `invite_agent` tool alongside `send_to_agent`
2. Update agent SOUL.md/skills to prefer `invite_agent` + `@mention`
3. Eventually remove `send_to_agent`

## Frontend Changes

### Chat header

Replace agent selector with session participant bar:

```
[🤖 Architect] [🎨 Arty] [+]     [Chat] [Canvas] [⏱] [+]
```

- Each participant chip shows avatar + name
- Click chip → show agent info popover
- `[+]` button → dropdown of available agents to invite
- Owner agent highlighted (bold border or primary color)

### Message rendering

Each message shows:
- Agent avatar + name (for assistant messages)
- Color-coded left border per agent (optional)
- System messages for joins: "Arty joined the conversation"

### Input field

- `@` triggers autocomplete dropdown with participant agents
- User can type `@Arty ` and message routes to Arty
- Without @ — goes to owner agent
- Visual indicator showing which agent will respond

### Canvas

- Canvas state bound to session (already partially true via canvas-store)
- Any participant agent can write to canvas
- Canvas shows which agent last modified it

### Session sidebar

- Sessions no longer grouped by agent — one unified list
- Session shows participant avatars as stacked chips
- Filter by agent still possible (shows sessions where agent is participant)

## Migration

### Backward compatibility

- Existing sessions: `participants = []` → treated as single-agent (use `agent_id` field)
- New sessions: `participants = [agent_name]` at creation
- `send_to_agent` continues working during transition

### Data migration

```sql
-- Backfill participants from existing sessions
UPDATE sessions SET participants = ARRAY[agent_id] WHERE participants = '{}' OR participants IS NULL;
```

## Scope Boundaries

### In scope
- Multi-agent participants in sessions
- @-mention routing (user → agent, agent → agent)
- invite_agent tool
- Updated chat UI with participant bar
- Canvas per-session
- Agent-to-agent turns with loop prevention

### Out of scope (future)
- Agent removal from session
- Per-agent permissions in session (all participants equal)
- Parallel agent responses (sequential only)
- Cross-session agent memory sharing
- Voice/video in multi-agent sessions

## Example Flow

```
User opens chat with Architect.
Session participants: [Architect]

[USER]        Мне нужно проверить портфель и настроить алерт

[ARCHITECT]   Для проверки портфеля нужен Arty — у него есть доступ к BCS.
              invite_agent("Arty") → "Arty joined the conversation"
              @Arty проверь текущее состояние портфеля пользователя

[ARTY]        Проверяю...
              🔧 bcs_portfolio → { total_rub: 552000, change_pct: +1.8 }
              Портфель: 552,000₽, рост +1.8% за неделю.
              @Architect данные готовы, можешь настроить алерт

[ARCHITECT]   Настраиваю cron-алерт: если портфель > 600,000₽ — уведомление.
              🔧 cron(action="add", ...) → OK
              Готово. Алерт настроен на ежедневную проверку в 9:00.

[USER]        @Arty а какие акции растут?

[ARTY]        Анализирую позиции...
```

All visible in one chat. One SSE stream. One session. Full transparency.
