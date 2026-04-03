# Multi-Agent Chat Implementation Plan (Revised)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transform HydeClaw from single-agent-per-chat to session-centric multi-agent conversations where users and agents can @-mention other agents into the same chat.

**Architecture:** Add `participants TEXT[]` column to sessions. New `invite_agent` tool lets agents invite others. Message routing parses `@AgentName` from user and agent messages. SSE events gain `agentName` field. UI shows participant chips in header and per-message agent avatars. Canvas binds to session not agent.

**Tech Stack:** Rust (backend), sqlx (migrations), TypeScript/React (UI), SSE streaming, Zustand stores.

**Spec:** `docs/superpowers/specs/2026-03-31-multi-agent-chat-design.md`

---

## Key Codebase Facts (from research)

These facts were verified against the actual source code and MUST be honored:

- **`ToolDefinition`** is defined in `crates/hydeclaw-types/src/lib.rs:102` with fields `{name, description, input_schema}`. It has NO `parameters` or `required_base` field.
- **`AgentEngine`** (engine.rs:130-193) has NO `sse_tx` field and NO `current_session_id` field. SSE events flow through `mpsc::UnboundedSender<StreamEvent>` passed as a parameter to `handle_sse()`.
- **`broadcast_ui_event()`** (engine.rs:453) sends JSON to WebSocket broadcast channel via `ui_event_tx`.
- **`execute_tool_call(name, arguments)`** (engine.rs:2143) takes only `name` and `arguments` — NO session_id parameter.
- **`execute_tool_calls_partitioned()`** (engine_parallel.rs:34) takes `session_id: Uuid` as a parameter but doesn't pass it to individual tool calls.
- **`StreamEvent`** (engine.rs:102-118) has these variants: `SessionId`, `MessageStart`, `StepStart`, `TextDelta`, `ToolCallStart`, `ToolCallArgs`, `ToolResult`, `StepFinish`, `RichCard`, `File`, `Finish`, `Error`.
- **`Session`** struct (db/sessions.rs:7-20) has NO `participants` column yet.
- **`ChatMessage`** (chat-store.ts:82-89) already has `agentId?: string`.
- **`MessageRow`** (api.ts:110-121) already has `agent_id?: string | null`.
- **`SessionRow`** (api.ts:98-108) has NO `participants` field yet.
- **Canvas store** is keyed by agent name: `canvases: Record<string, AgentCanvas>`.
- **Chat store** is per-agent: `agents: Record<string, AgentState>`.
- **Agent module** (agent/mod.rs) has NO `mention_parser` module.
- **Chat handler** (chat.rs:303-407) creates `event_tx`/`event_rx` channels, spawns engine task and converter task.
- **Session list API** (sessions.rs:81-150) manually builds JSON with `json!({...})` — NO `participants` field yet.

---

## Phase 1: Database & Backend Foundation

### Task 1: Add `participants` column to sessions

**Files:**
- Create: `migrations/002_multi_agent_sessions.sql`
- Modify: `crates/hydeclaw-core/src/db/sessions.rs` (Session struct + helpers)

- [ ] **Step 1: Write migration**

```sql
-- migrations/002_multi_agent_sessions.sql
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS participants TEXT[] NOT NULL DEFAULT '{}';

-- Backfill existing sessions: set participants = [agent_id]
UPDATE sessions SET participants = ARRAY[agent_id] WHERE participants = '{}';
```

- [ ] **Step 2: Add `participants` field to Session struct**

In `crates/hydeclaw-core/src/db/sessions.rs`, add to the `Session` struct (after `activity_at` field, line ~20):

```rust
#[sqlx(default)]
pub participants: Vec<String>,
```

- [ ] **Step 3: Update ALL session queries to include `participants`**

Every `sqlx::query_as::<_, Session>()` call must include `participants` in its SELECT list. Search for all occurrences:

```bash
grep -rn "query_as.*Session" crates/hydeclaw-core/src/ | grep -i "select"
```

For each query, add `, participants` to the SELECT column list. Key locations:
- `db/sessions.rs` — `get_or_create_session()`, `create_isolated_session_with_user()`
- `gateway/handlers/sessions.rs` — `api_list_sessions()` (lines 101, 113), `api_latest_session()`, `api_search_sessions()`

- [ ] **Step 4: Update session creation to initialize participants**

In `get_or_create_session` (db/sessions.rs), modify the INSERT to set `participants = ARRAY[$1]`:

```rust
// In the INSERT query, add participants column:
// INSERT INTO sessions (agent_id, user_id, channel, participants)
// VALUES ($1, $2, $3, ARRAY[$1])
```

Same for `create_isolated_session_with_user`.

- [ ] **Step 5: Add helper functions for participants**

In `sessions.rs`, add:

```rust
pub async fn add_participant(db: &PgPool, session_id: Uuid, agent_name: &str) -> Result<Vec<String>> {
    let row = sqlx::query(
        "UPDATE sessions SET participants = array_append(participants, $2) \
         WHERE id = $1 AND NOT ($2 = ANY(participants)) \
         RETURNING participants"
    )
    .bind(session_id)
    .bind(agent_name)
    .fetch_optional(db)
    .await?;
    match row {
        Some(r) => Ok(r.get("participants")),
        None => {
            // Agent was already a participant — return current list
            let r = sqlx::query("SELECT participants FROM sessions WHERE id = $1")
                .bind(session_id)
                .fetch_one(db)
                .await?;
            Ok(r.get("participants"))
        }
    }
}

pub async fn get_participants(db: &PgPool, session_id: Uuid) -> Result<Vec<String>> {
    let row = sqlx::query("SELECT participants FROM sessions WHERE id = $1")
        .bind(session_id)
        .fetch_one(db)
        .await?;
    Ok(row.get("participants"))
}
```

- [ ] **Step 6: Verify build**

Run: `cargo check --all-targets`
Expected: compiles with zero errors

- [ ] **Step 7: Commit**

```bash
git add migrations/002_multi_agent_sessions.sql crates/hydeclaw-core/src/db/sessions.rs
git commit -m "feat: add participants column to sessions for multi-agent chat"
```

---

### Task 2: Add `invite_agent` tool with session_id access

**Problem:** `execute_tool_call(name, arguments)` has no access to the current session_id. The invite_agent handler needs it to call `add_participant()`.

**Solution:** Add a `processing_session_id: Arc<tokio::sync::Mutex<Option<Uuid>>>` field to `AgentEngine`. Set it at the start of `handle_sse()` after `build_context()` resolves session_id, clear on finish. This is safe because each engine processes one message at a time per agent (the processing_tracker already enforces this).

**Files:**
- Modify: `crates/hydeclaw-core/src/agent/engine.rs` (add field + set/clear + dispatch)
- Modify: `crates/hydeclaw-core/src/agent/engine_tool_defs.rs` (tool definition)
- Modify: `crates/hydeclaw-core/src/agent/engine_subagent.rs` (handler)

- [ ] **Step 1: Add `processing_session_id` field to AgentEngine**

In `engine.rs`, add to the `AgentEngine` struct (after `processing_tracker`, line ~166):

```rust
/// Current session ID being processed (set during handle_sse/handle_with_status, cleared on finish).
/// Used by tools that need session context (e.g., invite_agent).
pub processing_session_id: Arc<tokio::sync::Mutex<Option<Uuid>>>,
```

Update the `AgentEngine` constructor (wherever `AgentEngine { ... }` is built) to initialize:

```rust
processing_session_id: Arc::new(tokio::sync::Mutex::new(None)),
```

- [ ] **Step 2: Set/clear `processing_session_id` in handle_sse**

In `handle_sse()` (engine.rs:1407), after `build_context()` returns `session_id` (line ~1440):

```rust
let (session_id, mut messages, available_tools) =
    self.build_context(msg, true, resume_session_id, force_new_session).await?;

// Store session_id for tool handlers that need session context (e.g., invite_agent)
*self.processing_session_id.lock().await = Some(session_id);
```

Before the function returns (both success and error paths), clear it. Best approach: create a guard struct or use `defer!`. Simplest: add clearing in the existing RAII status guard, or just clear before return:

```rust
// At end of handle_sse, before Ok(final_response):
*self.processing_session_id.lock().await = None;
```

Also do the same in `handle_with_status()` if it exists as another entry point.

- [ ] **Step 3: Add `invite_agent` to `all_system_tool_names()`**

In `engine_tool_defs.rs`, add `"invite_agent"` to the `all_system_tool_names()` array, in the Communication group:

```rust
// Communication
"send_to_agent", "invite_agent", "web_fetch", "message",
```

- [ ] **Step 4: Add tool definition in `internal_tool_definitions()`**

In `engine_tool_defs.rs`, in the `internal_tool_definitions()` method, after the `send_to_agent` ToolDefinition, add:

```rust
ToolDefinition {
    name: "invite_agent".to_string(),
    description: "Invite another agent into this chat session. The invited agent will see the full conversation history and can respond to @mentions. Use this when a task needs ongoing collaboration, not just a one-off question (use send_to_agent for that).".to_string(),
    input_schema: serde_json::json!({
        "type": "object",
        "properties": {
            "agent_name": {
                "type": "string",
                "description": "Name of the agent to invite into the current session"
            }
        },
        "required": ["agent_name"]
    }),
},
```

- [ ] **Step 5: Add dispatch in `execute_tool_call_inner()`**

In `engine.rs`, in `execute_tool_call_inner()` (line ~2223), add case near `handle_send_to_agent` (line ~2422):

```rust
if name == "invite_agent" {
    return self.handle_invite_agent(arguments).await;
}
```

- [ ] **Step 6: Implement handler in engine_subagent.rs**

In `engine_subagent.rs`, add:

```rust
pub(super) async fn handle_invite_agent(&self, args: &serde_json::Value) -> String {
    let agent_name = match args.get("agent_name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n,
        _ => return "Error: 'agent_name' is required".to_string(),
    };

    if agent_name == self.agent.name {
        return "Error: cannot invite yourself into your own session".to_string();
    }

    // Verify target agent exists in the agent_map
    let agent_map = match &self.agent_map {
        Some(m) => m,
        None => return "Error: agent registry not available".to_string(),
    };
    {
        let map = agent_map.read().await;
        if !map.contains_key(agent_name) {
            return format!("Error: agent '{}' not found. Use agents_list to see available agents.", agent_name);
        }
    }

    // Get session_id from the processing context
    let session_id = match *self.processing_session_id.lock().await {
        Some(id) => id,
        None => return "Error: no active session (invite_agent only works during chat processing)".to_string(),
    };

    // Add to session participants
    match crate::db::sessions::add_participant(&self.db, session_id, agent_name).await {
        Ok(participants) => {
            // Broadcast join event to WebSocket (UI sidebar refresh + live notification)
            self.broadcast_ui_event(serde_json::json!({
                "type": "agent_joined",
                "agent_name": agent_name,
                "session_id": session_id.to_string(),
                "invited_by": self.agent.name,
                "participants": participants,
            }));

            format!("{} has joined the conversation. You can now @-mention them to direct messages.", agent_name)
        }
        Err(e) => format!("Error adding participant: {}", e),
    }
}
```

**Note on SSE notification:** The tool handler does NOT have access to the `event_tx` channel (it's passed only to `handle_sse`, not stored on the struct). Instead:
- The tool result string ("X has joined the conversation") will be returned as a normal ToolResult SSE event.
- The `broadcast_ui_event` sends a WebSocket event that the UI can use to refresh the participant list.
- If we want a visual "agent joined" separator in the chat, the frontend should detect tool_name "invite_agent" in ToolResult events and render a system message accordingly.

- [ ] **Step 7: Verify build**

Run: `cargo check --all-targets`
Expected: compiles

- [ ] **Step 8: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine.rs \
        crates/hydeclaw-core/src/agent/engine_tool_defs.rs \
        crates/hydeclaw-core/src/agent/engine_subagent.rs
git commit -m "feat: add invite_agent tool with processing_session_id for session context"
```

---

### Task 3: @-mention parser with word boundary checks

**Problem from review:** Naive `text.contains("@Arty")` matches inside email addresses like `email@Arty.com`. Must check that `@` is preceded by whitespace or start-of-string.

**Files:**
- Create: `crates/hydeclaw-core/src/agent/mention_parser.rs`
- Modify: `crates/hydeclaw-core/src/agent/mod.rs` (module declaration)

- [ ] **Step 1: Create mention parser with word boundary logic**

```rust
// crates/hydeclaw-core/src/agent/mention_parser.rs

/// Parse @AgentName mentions from message text.
/// Returns all mentioned agent names (in order of appearance).
///
/// Word boundary rule: `@` must be preceded by whitespace, start of string,
/// or a punctuation character (not alphanumeric or `.`). This prevents
/// matching inside email addresses like `user@Arty.com`.
pub fn parse_mentions(text: &str, known_agents: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    let lower_text = text.to_lowercase();

    for agent in known_agents {
        let lower_pattern = format!("@{}", agent.to_lowercase());

        // Find all occurrences with word boundary check
        let mut search_from = 0;
        while let Some(pos) = lower_text[search_from..].find(&lower_pattern) {
            let abs_pos = search_from + pos;

            // Check word boundary BEFORE the @
            let valid_start = if abs_pos == 0 {
                true
            } else {
                let prev_char = text.as_bytes()[abs_pos - 1];
                // Previous char must be whitespace or certain punctuation, NOT alphanumeric or dot
                prev_char.is_ascii_whitespace() || matches!(prev_char, b'(' | b'[' | b'{' | b',' | b';' | b':' | b'"' | b'\'' | b'\n')
            };

            // Check word boundary AFTER the mention
            let end_pos = abs_pos + lower_pattern.len();
            let valid_end = if end_pos >= text.len() {
                true
            } else {
                let next_char = text.as_bytes()[end_pos];
                !next_char.is_ascii_alphanumeric() && next_char != b'_'
            };

            if valid_start && valid_end && !found.contains(&agent.clone()) {
                found.push(agent.clone());
            }

            search_from = abs_pos + 1;
        }
    }
    found
}

/// Return the first mentioned agent, or None.
pub fn parse_first_mention(text: &str, known_agents: &[String]) -> Option<String> {
    parse_mentions(text, known_agents).into_iter().next()
}

/// Strip @AgentName mention from text, returning cleaned text.
/// Case-insensitive replacement.
pub fn strip_mention(text: &str, agent_name: &str) -> String {
    let pattern = format!("@{}", agent_name);
    // Case-insensitive replace: find position, replace exact span
    let lower_text = text.to_lowercase();
    let lower_pattern = pattern.to_lowercase();
    match lower_text.find(&lower_pattern) {
        Some(pos) => {
            let mut result = String::with_capacity(text.len());
            result.push_str(&text[..pos]);
            result.push_str(&text[pos + pattern.len()..]);
            result.trim().to_string()
        }
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agents() -> Vec<String> {
        vec!["Arty".to_string(), "Architect".to_string(), "Alma".to_string()]
    }

    #[test]
    fn parse_mention_found() {
        assert_eq!(parse_first_mention("@Arty check portfolio", &agents()), Some("Arty".to_string()));
    }

    #[test]
    fn parse_mention_not_found() {
        assert_eq!(parse_first_mention("hello world", &agents()), None);
    }

    #[test]
    fn parse_mention_case_insensitive() {
        assert_eq!(parse_first_mention("@arty do something", &agents()), Some("Arty".to_string()));
    }

    #[test]
    fn no_match_in_email() {
        // Must NOT match @Arty inside an email address
        assert_eq!(parse_first_mention("email@Arty.com", &agents()), None);
    }

    #[test]
    fn no_match_in_email_with_dot() {
        assert_eq!(parse_first_mention("user.name@Architect.org", &agents()), None);
    }

    #[test]
    fn match_after_newline() {
        assert_eq!(parse_first_mention("hello\n@Arty check this", &agents()), Some("Arty".to_string()));
    }

    #[test]
    fn match_at_start() {
        assert_eq!(parse_first_mention("@Architect review this", &agents()), Some("Architect".to_string()));
    }

    #[test]
    fn no_match_partial_name() {
        // @Arty should not match if followed by more alphanumeric chars
        let agents = vec!["Art".to_string()];
        assert_eq!(parse_first_mention("@Arty check", &agents), None);
    }

    #[test]
    fn strip_mention_cleans_text() {
        assert_eq!(strip_mention("@Arty check portfolio", "Arty"), "check portfolio");
    }

    #[test]
    fn strip_mention_case_insensitive() {
        assert_eq!(strip_mention("@arty check portfolio", "Arty"), "check portfolio");
    }

    #[test]
    fn multiple_mentions() {
        let result = parse_mentions("@Arty and @Architect review this", &agents());
        assert_eq!(result, vec!["Arty".to_string(), "Architect".to_string()]);
    }
}
```

- [ ] **Step 2: Add module declaration**

In `crates/hydeclaw-core/src/agent/mod.rs`, add:

```rust
pub mod mention_parser;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p hydeclaw-core mention_parser -- --nocapture`
Expected: all tests pass, including email address exclusion

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/agent/mention_parser.rs \
        crates/hydeclaw-core/src/agent/mod.rs
git commit -m "feat: @-mention parser with word boundary checks (blocks email false positives)"
```

---

### Task 3.5: POST `/api/sessions/{id}/invite` endpoint

**Problem from review:** The frontend InviteAgentButton needs a REST API endpoint to add a participant to a session. The original plan's `inviteAgent()` function was undefined.

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/sessions.rs` (new handler)
- Modify: `crates/hydeclaw-core/src/gateway/mod.rs` (route registration)

- [ ] **Step 1: Add invite handler in sessions.rs**

```rust
#[derive(Debug, Deserialize)]
pub(crate) struct InviteRequest {
    pub agent_name: String,
}

pub(crate) async fn api_invite_to_session(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(req): Json<InviteRequest>,
) -> impl IntoResponse {
    // Validate agent exists
    let agent_exists = {
        let map = state.agent_map.read().await;
        map.contains_key(&req.agent_name)
    };
    if !agent_exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("agent '{}' not found", req.agent_name)})),
        ).into_response();
    }

    match crate::db::sessions::add_participant(&state.db, id, &req.agent_name).await {
        Ok(participants) => {
            // Broadcast to WebSocket for live UI updates
            let event = serde_json::json!({
                "type": "agent_joined",
                "agent_name": req.agent_name,
                "session_id": id.to_string(),
                "invited_by": "user",
                "participants": participants,
            });
            state.ui_event_tx.send(event.to_string()).ok();

            Json(json!({ "participants": participants })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ).into_response(),
    }
}
```

- [ ] **Step 2: Register route in mod.rs**

In `gateway/mod.rs`, add route after the existing `/api/sessions/{id}/documents` line (~124):

```rust
.route("/api/sessions/{id}/invite", post(handlers::sessions::api_invite_to_session))
```

- [ ] **Step 3: Verify build**

Run: `cargo check --all-targets`

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/sessions.rs \
        crates/hydeclaw-core/src/gateway/mod.rs
git commit -m "feat: POST /api/sessions/{id}/invite endpoint for multi-agent chat"
```

---

### Task 4: @-mention routing in chat handler

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/chat.rs` (mention-based routing)

- [ ] **Step 1: Add mention routing before engine dispatch**

In `api_chat_sse()` (chat.rs:303), after the `engine` is resolved and `user_text` is extracted (line ~349), but BEFORE the `IncomingMessage` is built (line ~374), add mention routing:

```rust
// ── @-mention routing ──────────────────────────────────────────
// If user message contains @AgentName, route to that agent instead.
let all_agent_names: Vec<String> = {
    let map = state.agent_map.read().await;
    map.keys().cloned().collect()
};

let (target_engine, target_agent_name, cleaned_text) = if let Some(mentioned) =
    crate::agent::mention_parser::parse_first_mention(&user_text, &all_agent_names)
{
    // Resolve the mentioned agent's engine
    let mentioned_engine = {
        let map = state.agent_map.read().await;
        map.get(&mentioned).map(|h| h.engine.clone())
    };
    match mentioned_engine {
        Some(eng) => {
            let cleaned = crate::agent::mention_parser::strip_mention(&user_text, &mentioned);
            (eng, mentioned.clone(), cleaned)
        }
        None => (engine.clone(), engine.agent.name.clone(), user_text.clone()),
    }
} else {
    (engine.clone(), engine.agent.name.clone(), user_text.clone())
};

// Auto-invite mentioned agent if not already a participant
// (session_id might not be resolved yet — the engine's handle_sse will create it.
//  We'll do the invite after session_id is known, in the converter task.)
let engine = target_engine;
let agent_name = target_agent_name.clone();
```

Then update the `IncomingMessage` construction (line ~374) to use `cleaned_text`:

```rust
let msg = hydeclaw_types::IncomingMessage {
    user_id: crate::agent::channel_kind::channel::UI.to_string(),
    text: Some(cleaned_text),  // <-- was user_text
    attachments: vec![],
    agent_id: engine.agent.name.clone(),  // target agent
    channel: crate::agent::channel_kind::channel::UI.to_string(),
    context: serde_json::Value::Null,
    timestamp: chrono::Utc::now(),
    formatting_prompt: None,
};
```

- [ ] **Step 2: Auto-invite in converter task after session_id is known**

In the converter task (chat.rs, inside the `tokio::spawn` that processes `event_rx`), in the `StreamEvent::SessionId` handler (line ~493), add auto-invite logic:

```rust
StreamEvent::SessionId(sid) => {
    // ... existing code ...
    session_uuid = parsed_uuid;

    // Auto-invite the target agent if it differs from the session owner
    if let Some(sid_uuid) = session_uuid {
        if agent_name != original_session_agent {
            // target agent was chosen via @-mention, ensure they're a participant
            let db = chat_db.clone();
            let agent = agent_name.clone();
            tokio::spawn(async move {
                let _ = crate::db::sessions::add_participant(&db, sid_uuid, &agent).await;
            });
        }
    }

    // ... rest of existing code ...
}
```

This requires passing `original_session_agent` (the agent_name from the request before mention routing) into the converter task scope.

- [ ] **Step 3: Verify build**

Run: `cargo check --all-targets`

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/chat.rs
git commit -m "feat: @-mention routing in chat handler with auto-invite"
```

---

### Task 4.5: Add `agentName` to SSE events

**Problem from review:** SSE events don't carry `agentName`, so the frontend can't tell which agent produced each text delta or tool call.

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/chat.rs` (converter task)

- [ ] **Step 1: Add `agentName` to key SSE event types**

In the converter task (chat.rs, the `tokio::spawn` starting ~line 428), the variable `agent_name` already exists (cloned from `msg.agent_id`). Add `"agentName"` field to the following SSE events:

For `MessageStart` (line ~517):
```rust
StreamEvent::MessageStart { message_id } => {
    json!({"type": sse_types::START, "messageId": message_id, "agentName": agent_name})
}
```

For `TextDelta` (line ~530-534), add agentName to the `text-start` event:
```rust
let start_data = json!({"type": sse_types::TEXT_START, "id": text_id.clone(), "agentName": agent_name.clone()}).to_string();
```

For `ToolCallStart` (line ~547):
```rust
StreamEvent::ToolCallStart { id, name } => {
    tool_name_map.insert(id.clone(), name.clone());
    json!({
        "type": sse_types::TOOL_INPUT_START,
        "toolCallId": id,
        "toolName": name,
        "agentName": agent_name,
    })
}
```

For `Finish` (line ~600), add agentName:
```rust
let finish_data = json!({"type": sse_types::FINISH, "agentName": agent_name}).to_string();
```

- [ ] **Step 2: Verify build**

Run: `cargo check --all-targets`

- [ ] **Step 3: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/chat.rs
git commit -m "feat: add agentName field to SSE events for multi-agent identity"
```

---

### Task 5: Agent-to-agent turn loop in chat handler

**Problem from review:** The original plan was too abstract about how agent-to-agent turns work. Concrete implementation needed.

**Architecture decision:** The turn loop happens OUTSIDE the engine — in the chat handler. After an agent finishes responding, the chat handler checks the accumulated response text for @-mentions. If found, it creates a NEW `event_tx`/`event_rx` pair, calls the target engine's `handle_sse()` with the response as a new message, and forwards those events through the SAME `sse_tx` to the client. The client sees a continuous stream.

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/chat.rs` (turn loop)

- [ ] **Step 1: Refactor engine dispatch into a reusable function**

Extract the engine spawning and event collection into a helper. Before the `api_chat_sse` function, add:

```rust
/// Spawn an engine task and return a handle + the accumulated response text.
/// The converter_tx receives all StreamEvents for SSE forwarding.
async fn run_agent_turn(
    engine: Arc<crate::agent::engine::AgentEngine>,
    msg: hydeclaw_types::IncomingMessage,
    session_id: Option<uuid::Uuid>,
    force_new: bool,
    converter_tx: mpsc::UnboundedSender<(StreamEvent, String)>,  // (event, agent_name)
) -> String {
    let agent_name = engine.agent.name.clone();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();

    // Run engine
    let engine_clone = engine.clone();
    let event_tx_clone = event_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = engine_clone.handle_sse(&msg, event_tx_clone.clone(), session_id, force_new).await {
            tracing::error!(error = %e, "SSE chat error");
            event_tx_clone.send(StreamEvent::Error(e.to_string())).ok();
        }
    });

    // Collect response text while forwarding events
    let mut accumulated = String::new();
    while let Some(event) = event_rx.recv().await {
        if let StreamEvent::TextDelta(ref text) = event {
            accumulated.push_str(text);
        }
        let is_finish = matches!(event, StreamEvent::Finish { .. } | StreamEvent::Error(_));
        converter_tx.send((event, agent_name.clone())).ok();
        if is_finish {
            break;
        }
    }
    accumulated
}
```

**Note:** This is a conceptual design. The actual implementation will need to integrate with the existing converter task pattern (which handles streaming message DB persistence, registry, etc.). A simpler approach may be preferred — see Step 2.

- [ ] **Step 2: Simpler approach — sequential turn loop in engine task**

Instead of the complex refactor above, modify the engine task spawn (chat.rs:393) to include a turn loop:

```rust
let engine_handle = tokio::spawn(async move {
    let mut current_engine = engine;
    let mut current_msg = msg;
    let mut current_session_id = session_id;
    let mut force_new = force_new_session;
    let mut turn_count = 0;
    const MAX_AGENT_TURNS: usize = 5;

    loop {
        let current_agent_name = current_engine.agent.name.clone();

        // Run engine turn
        if let Err(e) = current_engine.handle_sse(&current_msg, event_tx.clone(), current_session_id, force_new).await {
            tracing::error!(error = %e, "SSE chat error (agent: {})", current_agent_name);
            event_tx.send(StreamEvent::Error(e.to_string())).ok();
            break;
        }

        turn_count += 1;
        if turn_count >= MAX_AGENT_TURNS {
            tracing::info!("Agent turn limit ({}) reached, stopping turn loop", MAX_AGENT_TURNS);
            break;
        }

        // Check accumulated text for @-mention of another agent
        // We need to read the response text. The engine stores it in the session messages.
        // Read the last assistant message from the session.
        let sid = match *current_engine.processing_session_id.lock().await {
            Some(id) => id,
            None => break,
        };

        let last_response = match crate::db::sessions::get_last_assistant_message(&current_engine.db, sid).await {
            Ok(Some(text)) => text,
            _ => break,
        };

        let next_agent = crate::agent::mention_parser::parse_first_mention(&last_response, &all_agent_names_clone);
        let next_agent = match next_agent {
            Some(name) if name != current_agent_name => name,
            _ => break,  // No mention or self-mention — done
        };

        // Resolve next agent's engine
        let next_engine = match agent_map_clone.read().await.get(&next_agent) {
            Some(h) => h.engine.clone(),
            None => break,
        };

        // Auto-invite next agent
        let _ = crate::db::sessions::add_participant(&current_engine.db, sid, &next_agent).await;

        // Emit agent-turn SSE event so the UI shows a visual separator
        event_tx.send(StreamEvent::RichCard {
            card_type: "agent-turn".to_string(),
            data: serde_json::json!({
                "agentName": next_agent,
                "reason": format!("mentioned by {}", current_agent_name),
            }),
        }).ok();

        // Build message for next agent: include the @-mention text as context
        let cleaned = crate::agent::mention_parser::strip_mention(&last_response, &next_agent);
        current_msg = hydeclaw_types::IncomingMessage {
            user_id: current_agent_name.clone(),  // From the agent, not the user
            text: Some(cleaned),
            attachments: vec![],
            agent_id: next_agent.clone(),
            channel: crate::agent::channel_kind::channel::UI.to_string(),
            context: serde_json::Value::Null,
            timestamp: chrono::Utc::now(),
            formatting_prompt: None,
        };
        current_engine = next_engine;
        current_session_id = Some(sid);  // Continue in same session
        force_new = false;
    }

    // Notify UI about session update
    let event = serde_json::json!({
        "type": "session_updated",
        "agent": agent_for_broadcast,
        "channel": crate::agent::channel_kind::channel::UI,
    });
    ui_tx.send(event.to_string()).ok();
});
```

This requires cloning `all_agent_names` and `agent_map` into the spawned task:

```rust
let all_agent_names_clone = all_agent_names.clone();
let agent_map_clone = state.agent_map.clone();
```

- [ ] **Step 3: Add `get_last_assistant_message` helper**

In `db/sessions.rs`, add:

```rust
/// Get the text content of the last assistant message in a session.
pub async fn get_last_assistant_message(db: &PgPool, session_id: Uuid) -> Result<Option<String>> {
    let row = sqlx::query(
        "SELECT content FROM messages WHERE session_id = $1 AND role = 'assistant' \
         ORDER BY created_at DESC LIMIT 1"
    )
    .bind(session_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| r.get("content")))
}
```

- [ ] **Step 4: Update converter task to handle agent identity switching**

The converter task needs to know the current agent for each event. Since events now come from multiple agents in sequence, add an `agent_name` tracking variable that updates when a `MessageStart` event with a different agent arrives:

In the converter task, change `agent_name` from `let agent_name = ...` to `let mut current_responding_agent = agent_name.clone()`. Then in the `MessageStart` handler, check if the `message_id` prefix indicates a different agent, OR simpler: rely on the engine's `handle_sse` which sets `msg.agent_id` — the agent_id is already used in `upsert_streaming_message`. For SSE agentName, the agent_name variable already holds the correct value from the initial setup. For multi-turn, we need to update it.

**Simplest approach:** Add a new `StreamEvent` variant `AgentSwitch { agent_name: String }` that the turn loop emits before each new agent turn. The converter task updates `current_responding_agent` when it sees this event:

```rust
// In StreamEvent enum (engine.rs):
AgentSwitch { agent_name: String },

// In converter task:
StreamEvent::AgentSwitch { agent_name: new_name } => {
    current_responding_agent = new_name;
    continue;  // Don't emit SSE for this internal event
}
```

Then in the turn loop (Step 2), emit `AgentSwitch` before each new turn:

```rust
event_tx.send(StreamEvent::AgentSwitch { agent_name: next_agent.clone() }).ok();
```

- [ ] **Step 5: Verify build and test**

Run: `cargo check --all-targets && cargo test -p hydeclaw-core`

- [ ] **Step 6: Commit**

```bash
git add crates/hydeclaw-core/src/agent/engine.rs \
        crates/hydeclaw-core/src/db/sessions.rs \
        crates/hydeclaw-core/src/gateway/handlers/chat.rs
git commit -m "feat: agent-to-agent turn loop with @-mention detection and response collection"
```

---

### Task 6: Include `participants` in session API responses

**Files:**
- Modify: `crates/hydeclaw-core/src/gateway/handlers/sessions.rs` (all session JSON builders)

- [ ] **Step 1: Add participants to session JSON in all list/detail handlers**

In `api_list_sessions` (sessions.rs:128-140), add `"participants"` to the json! macro:

```rust
json!({
    "id": s.id,
    "agent_id": s.agent_id,
    "user_id": s.user_id,
    "channel": s.channel,
    "started_at": s.started_at.to_rfc3339(),
    "last_message_at": s.last_message_at.to_rfc3339(),
    "title": s.title,
    "metadata": s.metadata,
    "run_status": s.run_status,
    "participants": s.participants,  // <-- NEW
})
```

Find all other places where Session rows are serialized to JSON (search for `s.agent_id` or `s.run_status` in sessions.rs) and add `"participants": s.participants` to each.

Also update `api_latest_session`, `api_search_sessions`, and any other handler returning session data.

- [ ] **Step 2: Update the SELECT queries**

All `SELECT` queries that feed into `Session` struct must include `participants`. Search:

```bash
grep -n "SELECT.*FROM sessions" crates/hydeclaw-core/src/gateway/handlers/sessions.rs
```

Add `, participants` to each SELECT column list.

- [ ] **Step 3: Verify build**

Run: `cargo check --all-targets`

- [ ] **Step 4: Commit**

```bash
git add crates/hydeclaw-core/src/gateway/handlers/sessions.rs
git commit -m "feat: include participants in all session API responses"
```

---

## Phase 2: Frontend

### Task 7: Update TypeScript types

**Files:**
- Modify: `ui/src/types/api.ts` (SessionRow, add participants)
- Modify: `ui/src/stores/sse-events.ts` (new event types)

- [ ] **Step 1: Add `participants` to SessionRow**

In `api.ts`, add to `SessionRow` interface (after `metadata`, line ~107):

```typescript
participants?: string[];  // optional for backward compat with old sessions
```

- [ ] **Step 2: Add new SSE event type fields**

In `sse-events.ts`, find the relevant event type definitions and add `agentName` to `StartEvent` (or the type for `"start"` SSE events):

```typescript
// Add to the start event type:
agentName?: string;
```

Add a new `RichCard` subtype for agent-turn:

```typescript
// In the RichCard event handling, add a case for cardType "agent-turn":
export interface AgentTurnCard {
  agentName: string;
  reason: string;
}
```

- [ ] **Step 3: Commit**

```bash
git add ui/src/types/api.ts ui/src/stores/sse-events.ts
git commit -m "feat: add multi-agent types to frontend (participants, agentName SSE)"
```

---

### Task 8: Chat-store SSE processor — set agentId on messages

**Problem from review:** The frontend's `ChatMessage` already has `agentId?: string` but the SSE processor doesn't set it. Need to extract `agentName` from SSE events and set `agentId` on `ChatMessage`.

**Files:**
- Modify: `ui/src/stores/chat-store.ts` (SSE processor)

- [ ] **Step 1: Track current responding agent in chat store**

In the SSE processing logic (wherever `StreamEvent` objects are parsed and `ChatMessage` objects are created/updated), add:

```typescript
// In the SSE processor state (probably inside the streaming connection handler):
let currentRespondingAgent: string | null = null;

// When processing a "start" event with agentName:
if (event.type === "start" && event.agentName) {
    currentRespondingAgent = event.agentName;
}

// When processing "text-start" with agentName:
if (event.type === "text-start" && event.agentName) {
    currentRespondingAgent = event.agentName;
}
```

- [ ] **Step 2: Set agentId when creating live messages**

Wherever a new `ChatMessage` with `role: "assistant"` is created from SSE data, set `agentId`:

```typescript
const newMessage: ChatMessage = {
    id: messageId,
    role: "assistant",
    parts: [],
    agentId: currentRespondingAgent ?? undefined,
};
```

- [ ] **Step 3: Handle agent-turn RichCard**

When a `rich-card` event with `cardType: "agent-turn"` arrives, update `currentRespondingAgent`:

```typescript
if (event.type === "rich-card" && event.cardType === "agent-turn") {
    currentRespondingAgent = event.data.agentName;
    // Optionally insert a visual separator message in liveMessages
}
```

- [ ] **Step 4: Set agentId when loading messages from DB**

In the message loading logic (where `MessageRow` from the API is converted to `ChatMessage`), map `agent_id`:

```typescript
// When converting MessageRow to ChatMessage:
agentId: row.agent_id ?? undefined,
```

- [ ] **Step 5: Build and verify**

Run: `cd ui && npm run build`

- [ ] **Step 6: Commit**

```bash
git add ui/src/stores/chat-store.ts
git commit -m "feat: set agentId on ChatMessages from SSE agentName field"
```

---

### Task 9: Participant chips in chat header (keep agent selector)

**Key change from review:** Do NOT replace the agent selector. Keep it for new chat creation. Add participant bar ALONGSIDE it for existing sessions.

**Files:**
- Modify: `ui/src/app/(authenticated)/chat/page.tsx` (header area)
- Modify: `ui/src/lib/api.ts` (add inviteAgent function)

- [ ] **Step 1: Add `inviteAgent` API function**

In `ui/src/lib/api.ts`, add:

```typescript
export async function inviteAgent(sessionId: string, agentName: string): Promise<{ participants: string[] }> {
    const res = await fetch(`/api/sessions/${sessionId}/invite`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
            Authorization: `Bearer ${getToken()}`,
        },
        body: JSON.stringify({ agent_name: agentName }),
    });
    if (!res.ok) throw new Error(`Failed to invite agent: ${res.statusText}`);
    return res.json();
}
```

- [ ] **Step 2: Add ParticipantBar component**

In `page.tsx`, add a `ParticipantBar` component that shows ONLY when there's an active session with participants:

```tsx
function ParticipantBar({ sessionId, currentAgent }: { sessionId: string | null; currentAgent: string }) {
    const { data: sessionsData } = useSessions(currentAgent);
    const session = sessionsData?.sessions.find((s: SessionRow) => s.id === sessionId);
    const participants = session?.participants ?? [currentAgent];
    const agentIcons = useAuthStore(s => s.agentIcons);
    const { data: allAgents } = useAgents();
    const queryClient = useQueryClient();

    // Only show when there are multiple participants
    if (participants.length <= 1) return null;

    const available = allAgents?.filter((a: AgentInfo) => !participants.includes(a.name)) ?? [];

    return (
        <div className="flex items-center gap-1.5">
            {participants.map((name: string) => (
                <div key={name} className="flex items-center gap-1.5 h-8 px-2.5 rounded-lg border border-border/40 bg-muted/30 text-xs font-semibold">
                    {agentIcons[name] ? (
                        <img src={`/uploads/${agentIcons[name]}`} className="h-5 w-5 rounded-md object-cover" alt={name} />
                    ) : (
                        <div className="h-5 w-5 rounded-md bg-primary/20 flex items-center justify-center text-[10px] font-bold text-primary">
                            {name[0]}
                        </div>
                    )}
                    <span className="uppercase tracking-wide">{name}</span>
                </div>
            ))}
            {sessionId && available.length > 0 && (
                <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                        <Button variant="outline" size="icon" className="h-8 w-8 border-border/40 bg-muted/30">
                            <Plus className="h-3.5 w-3.5" />
                        </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent>
                        {available.map((a: AgentInfo) => (
                            <DropdownMenuItem key={a.name} onClick={async () => {
                                try {
                                    await inviteAgent(sessionId, a.name);
                                    queryClient.invalidateQueries({ queryKey: ["sessions", currentAgent] });
                                } catch (err) {
                                    console.error("Failed to invite:", err);
                                }
                            }}>
                                {a.name}
                            </DropdownMenuItem>
                        ))}
                    </DropdownMenuContent>
                </DropdownMenu>
            )}
        </div>
    );
}
```

- [ ] **Step 3: Add ParticipantBar to header layout**

In the header section of the chat page (around line ~591), KEEP the existing `{agentSelector}` and ADD the participant bar next to it:

```tsx
<div className="flex items-center gap-3">
    {agentSelector}
    <ParticipantBar sessionId={activeSessionId} currentAgent={currentAgent} />
</div>
```

- [ ] **Step 4: Build and verify**

Run: `cd ui && npm run build`

- [ ] **Step 5: Commit**

```bash
git add ui/src/app/\(authenticated\)/chat/page.tsx ui/src/lib/api.ts
git commit -m "feat: participant chips + invite button in chat header (keeps agent selector)"
```

---

### Task 10: Per-message agent avatars in ChatThread

**Files:**
- Modify: `ui/src/app/(authenticated)/chat/ChatThread.tsx` (message rendering)

- [ ] **Step 1: Use agentId for per-message agent identity**

In the `AssistantMessageView` component (or wherever assistant messages are rendered), the existing `useAgentIdLookup(msgId)` context already exists. Enhance it to also check `message.agentId`:

```tsx
// In the assistant message component:
const agentIdFromContext = useAgentIdLookup(msg.id);
const agentName = msg.agentId ?? agentIdFromContext ?? currentAgent;
const agentIcons = useAuthStore(s => s.agentIcons);
const iconUrl = agentIcons[agentName] ? `/uploads/${agentIcons[agentName]}` : undefined;
```

Display agent name above the message when in a multi-agent session:

```tsx
{isMultiAgent && (
    <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground/70 mb-1">
        {agentName}
    </span>
)}
```

Where `isMultiAgent` is derived from the session's participants count (pass as prop or context).

- [ ] **Step 2: Handle agent-turn RichCard as visual separator**

Add rendering for `rich-card` with `cardType: "agent-turn"`:

```tsx
function AgentTurnSeparator({ data }: { data: { agentName: string; reason: string } }) {
    return (
        <div className="flex items-center justify-center gap-2 py-3 text-xs text-muted-foreground/50">
            <div className="h-px flex-1 bg-border/30" />
            <span>{data.agentName} is responding</span>
            <div className="h-px flex-1 bg-border/30" />
        </div>
    );
}
```

Also handle invite_agent tool results — when a `tool-output-available` event has `toolName: "invite_agent"`, render a "joined" system message:

```tsx
function AgentJoinedMessage({ agentName }: { agentName: string }) {
    return (
        <div className="flex items-center justify-center gap-2 py-2 text-xs text-muted-foreground/50">
            <div className="h-px flex-1 bg-border/30" />
            <span>{agentName} joined the conversation</span>
            <div className="h-px flex-1 bg-border/30" />
        </div>
    );
}
```

- [ ] **Step 3: Build and verify**

Run: `cd ui && npm run build`

- [ ] **Step 4: Commit**

```bash
git add ui/src/app/\(authenticated\)/chat/ChatThread.tsx
git commit -m "feat: per-message agent avatars, turn separators, and join notifications"
```

---

### Task 11: @-mention autocomplete in input

**Files:**
- Modify: `ui/src/app/(authenticated)/chat/ChatThread.tsx` or the input component

- [ ] **Step 1: Add @-mention detection in input**

In the chat input component, detect when the user types `@` and show a dropdown:

```tsx
function MentionAutocomplete({ text, agents, onSelect }: {
    text: string;
    agents: string[];
    onSelect: (name: string) => void;
}) {
    const match = text.match(/@(\w*)$/);
    if (!match) return null;

    const query = match[1].toLowerCase();
    const filtered = agents.filter(p => p.toLowerCase().startsWith(query));

    if (filtered.length === 0) return null;

    return (
        <div className="absolute bottom-full mb-1 left-0 bg-popover border border-border rounded-lg shadow-lg p-1 z-50">
            {filtered.map(name => (
                <button
                    key={name}
                    className="flex items-center gap-2 px-3 py-1.5 text-sm rounded-md hover:bg-muted w-full text-left"
                    onMouseDown={(e) => { e.preventDefault(); onSelect(name); }}
                >
                    <span className="font-semibold">@{name}</span>
                </button>
            ))}
        </div>
    );
}
```

The `onSelect` callback should replace the `@partial` text with `@FullName `:

```typescript
const handleMentionSelect = (name: string) => {
    const match = inputText.match(/@(\w*)$/);
    if (match) {
        const before = inputText.slice(0, match.index);
        setInputText(`${before}@${name} `);
    }
};
```

- [ ] **Step 2: Show target agent indicator below input**

```tsx
{targetAgent && targetAgent !== ownerAgent && (
    <span className="text-[10px] text-muted-foreground/50 ml-2">
        will respond: {targetAgent}
    </span>
)}
```

Where `targetAgent` is computed by parsing the current input text for @-mentions.

- [ ] **Step 3: Build and verify**

Run: `cd ui && npm run build`

- [ ] **Step 4: Commit**

```bash
git add ui/src/app/\(authenticated\)/chat/ChatThread.tsx
git commit -m "feat: @-mention autocomplete and target agent indicator in input"
```

---

### Task 12: Canvas per-session binding

**Problem from review:** Canvas store is keyed by agent name (`canvases: Record<string, AgentCanvas>` where key = agent name). For multi-agent sessions, canvas should be keyed by session ID so all agents in a session share one canvas.

**Files:**
- Modify: `ui/src/stores/canvas-store.ts`
- Modify: components that use `useCanvasStore`

- [ ] **Step 1: Change canvas key from agent name to session ID**

In `canvas-store.ts`, the `handleEvent` method receives an event with an `agent` field. We need to change the keying. The canvas event must include `sessionId`. Two approaches:

**Approach A (minimal):** The canvas tool already sends events via WebSocket. Add `sessionId` to the canvas event payload on the backend (in the canvas tool handler). On the frontend, key by sessionId when available, fall back to agent name.

**Approach B (frontend-only):** Since the chat page knows the active session ID, pass it to the canvas store. Change the store to accept explicit keys:

```typescript
// In canvas-store.ts, change handleEvent to accept an explicit key:
handleEvent(event: CanvasEvent, key: string) {  // key = sessionId or agentName
    set((s) => {
        switch (event.action) {
            case "present":
                s.canvases[key] = { ... };
                break;
            // etc.
        }
    });
}
```

Then in the chat page, when calling `handleEvent`, pass `activeSessionId ?? agentName` as the key.

- [ ] **Step 2: Update CanvasPanel to use session-based key**

```typescript
// In CanvasPanel:
const sessionId = useActiveSessionId();  // or passed as prop
const canvasKey = sessionId ?? agentName;
const canvas = useCanvasStore(s => s.canvases[canvasKey]);
```

- [ ] **Step 3: Build and verify**

Run: `cd ui && npm run build && npm test`

- [ ] **Step 4: Commit**

```bash
git add ui/src/stores/canvas-store.ts \
        ui/src/app/\(authenticated\)/chat/CanvasPanel.tsx
git commit -m "feat: bind canvas to session ID instead of agent name"
```

---

### Task 12.5: Chat-store session-level routing for multi-agent sessions

**Problem:** `chat-store.ts` keys all state per-agent: `agents: Record<string, AgentState>`. Each `AgentState` holds `activeSessionId`, `liveMessages`, `viewMode`, `streamStatus`, etc. In a multi-agent session (e.g. Architect + Arty in one session), if the user switches the agent selector from Architect to Arty, the store creates a **fresh** `AgentState` for Arty with `activeSessionId: null` — the user loses the active session context. They need to see the **same** session regardless of which participant agent is selected.

**Solution:** Add a session-routing layer. When switching agents, if the **current active session** includes the target agent as a participant, keep that session active instead of resetting. This is a targeted fix that preserves the existing per-agent architecture.

**Files:**
- Modify: `ui/src/stores/chat-store.ts`
- Modify: `ui/src/app/(authenticated)/chat/page.tsx` (session restore logic)

- [ ] **Step 1: Add `sessionParticipantsCache` to store state**

In `chat-store.ts`, add a lightweight cache that tracks which sessions have which participants. This avoids re-fetching on every agent switch:

```typescript
// Add to ChatStore interface:
/** Cache: sessionId → participant list (updated from API responses and WS events). */
sessionParticipants: Record<string, string[]>;
updateSessionParticipants: (sessionId: string, participants: string[]) => void;
```

Implementation:

```typescript
sessionParticipants: {},

updateSessionParticipants: (sessionId, participants) => {
    set((draft) => {
        draft.sessionParticipants[sessionId] = participants;
    });
},
```

Populate this cache:
- When `data-session-id` SSE event arrives, fetch participants from the sessions query cache
- When `agent_joined` WS event arrives, update from the event payload
- When session list data loads, bulk-populate from `SessionRow.participants`

- [ ] **Step 2: Update `setCurrentAgent` to carry over multi-agent sessions**

In the `setCurrentAgent` action, check if the current session includes the target agent as a participant. If so, carry the session over:

```typescript
setCurrentAgent: (name: string) => {
    const prev = get().currentAgent;
    if (prev === name) return;

    // Check if current session is multi-agent and includes the new agent
    const prevState = get().agents[prev];
    const activeSessionId = prevState?.activeSessionId;

    set((draft) => { draft.currentAgent = name; });

    if (activeSessionId) {
        const participants = get().sessionParticipants[activeSessionId];
        if (participants && participants.includes(name)) {
            // Carry over the session — the new agent is already a participant
            ensure(name);
            update(name, {
                activeSessionId,
                liveMessages: prevState?.liveMessages ?? [],
                viewMode: prevState?.viewMode ?? "live",
                streamStatus: prevState?.streamStatus ?? "idle",
            });
            return;
        }
    }

    // Otherwise, normal behavior: restore last session for the new agent
    // (existing restore logic in page.tsx handles this)
},
```

- [ ] **Step 3: Update session restore logic in page.tsx**

In the session restore effect (page.tsx, lines ~108-151), when restoring a session for an agent, also check `sessionParticipants` to see if any recently active multi-agent session includes this agent:

```typescript
// In the restore effect, after checking URL and server active sessions:
// Check if any other agent's active session includes this agent as participant
const allAgentStates = chatStore.agents;
for (const [otherAgent, otherState] of Object.entries(allAgentStates)) {
    if (otherAgent === currentAgent) continue;
    const sid = otherState.activeSessionId;
    if (sid && chatStore.sessionParticipants[sid]?.includes(currentAgent)) {
        // This agent is a participant in another agent's active session — use it
        chatStore.selectSessionById(currentAgent, sid);
        return;
    }
}
```

- [ ] **Step 4: Populate cache from SSE and WS events**

In the SSE `data-session-id` handler (chat-store.ts, ~line 634), after setting `activeSessionId`:

```typescript
// After update(agent, { activeSessionId: sid }):
// Try to populate participants from cached session data
const sessionsData = queryClient.getQueryData<{ sessions: SessionRow[] }>(
    qk.sessions(agent)
);
const session = sessionsData?.sessions.find(s => s.id === sid);
if (session?.participants) {
    get().updateSessionParticipants(sid, session.participants);
}
```

In the WS `agent_joined` handler (wherever WS events are processed):

```typescript
if (data.type === "agent_joined") {
    get().updateSessionParticipants(data.session_id, data.participants);
    // Invalidate session queries to refresh sidebar
    queryClient.invalidateQueries({ queryKey: ["sessions"] });
}
```

- [ ] **Step 5: Build and verify**

Run: `cd ui && npm run build`

Test scenarios:
1. Open chat with Architect → invite Arty → switch agent selector to Arty → should see SAME session
2. Open chat with Arty (no multi-agent) → switch to Architect → should see Architect's own sessions (normal behavior)
3. Stream active with Architect+Arty → switch to Arty mid-stream → stream continues in same session

- [ ] **Step 6: Commit**

```bash
git add ui/src/stores/chat-store.ts \
        ui/src/app/\(authenticated\)/chat/page.tsx
git commit -m "feat: chat-store session-level routing for multi-agent session continuity"
```

---

### Task 13: Session sidebar with participant avatars

**Files:**
- Modify: `ui/src/app/(authenticated)/chat/page.tsx` (session list items)

- [ ] **Step 1: Show stacked participant avatars in session list**

In the session list rendering (wherever `SessionRow` items are mapped to list items), add participant avatar display:

```tsx
function SessionParticipantAvatars({ session }: { session: SessionRow }) {
    const agentIcons = useAuthStore(s => s.agentIcons);
    const participants = session.participants ?? [session.agent_id];

    if (participants.length <= 1) return null;

    return (
        <div className="flex -space-x-1.5 ml-auto">
            {participants.slice(0, 3).map(name => (
                <div key={name} className="h-5 w-5 rounded-full border-2 border-background bg-muted flex items-center justify-center overflow-hidden">
                    {agentIcons[name] ? (
                        <img src={`/uploads/${agentIcons[name]}`} className="h-full w-full object-cover" alt={name} />
                    ) : (
                        <span className="text-[8px] font-bold">{name[0]}</span>
                    )}
                </div>
            ))}
            {participants.length > 3 && (
                <div className="h-5 w-5 rounded-full border-2 border-background bg-muted/60 flex items-center justify-center">
                    <span className="text-[8px]">+{participants.length - 3}</span>
                </div>
            )}
        </div>
    );
}
```

- [ ] **Step 2: Build and verify**

Run: `cd ui && npm run build`

- [ ] **Step 3: Commit**

```bash
git add ui/src/app/\(authenticated\)/chat/page.tsx
git commit -m "feat: show participant avatars in session sidebar for multi-agent sessions"
```

---

## Phase 3: Integration & Cleanup

### Task 14: Update agent SOUL.md files

**Files:**
- Modify: `workspace/agents/Architect/SOUL.md`
- Modify: `workspace/skills/architect/agent-management.md` (if exists)

- [ ] **Step 1: Document invite_agent in SOUL.md**

Add to Tools section:
```markdown
- `invite_agent` — invite another agent into current chat session for ongoing collaboration
```

Add usage guidance:
```markdown
### Multi-Agent Chat
For ongoing collaboration, use invite_agent to bring another agent into the conversation.
Then @-mention them to direct messages. Example:
1. invite_agent(agent_name="Arty")
2. "@Arty check the portfolio returns"
Arty responds in the same chat with full conversation context.

For one-off questions, send_to_agent is still preferred (creates an isolated session).
```

- [ ] **Step 2: Commit**

```bash
git add workspace/agents/Architect/SOUL.md
git commit -m "docs: document invite_agent and multi-agent chat in SOUL.md"
```

---

### Task 15: End-to-end verification

- [ ] **Step 1: Build everything**

```bash
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test
cd ui && npm run build && npm test
```

- [ ] **Step 2: Deploy to Pi and test**

```bash
make deploy
make doctor
```

- [ ] **Step 3: Manual test checklist**

- [ ] Start chat with Architect — agent selector works as before
- [ ] Type `@Arty hello` — Arty becomes the responding agent (check SSE agentName)
- [ ] Arty auto-joins session — participant list updates
- [ ] Click [+] button in header — invite Alma via REST API
- [ ] Alma appears in participant chips
- [ ] Architect uses `invite_agent("Arty")` — tool result confirms join
- [ ] Agent @-mentions another agent in response — turn auto-passes (check turn loop)
- [ ] 5+ consecutive agent turns — loop limit kicks in, chain stops
- [ ] Canvas shows correctly when multiple agents are in session
- [ ] Session sidebar shows stacked avatars for multi-agent sessions
- [ ] @-mention autocomplete works in input field
- [ ] Email addresses like `user@Arty.com` do NOT trigger mention routing
- [ ] Existing single-agent chats work exactly as before (no regression)

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "feat: multi-agent chat — complete implementation"
```

---

## Summary of Issues Fixed

| # | Issue | Fix |
|---|-------|-----|
| 1 | `self.sse_tx` doesn't exist | Use `broadcast_ui_event()` for WebSocket notification + tool result string. No direct SSE emission from tool handler. |
| 2 | `self.current_session_id` doesn't exist | Added `processing_session_id: Arc<Mutex<Option<Uuid>>>` field to AgentEngine, set in handle_sse after build_context. |
| 3 | Mention parser matches emails | Word boundary check: `@` must be preceded by whitespace/SOL/punctuation, followed by non-alphanumeric. |
| 4 | Turn loop too abstract | Concrete implementation in chat handler engine task: read last assistant message from DB, parse mentions, resolve engine, loop with MAX_AGENT_TURNS=5. |
| 5 | `collect_response_text` undefined | Read last assistant message from DB via `get_last_assistant_message()` instead of accumulating from event channel (which would require complex refactoring of the converter task). |
| 6 | `inviteAgent` function undefined | New `POST /api/sessions/{id}/invite` endpoint (Task 3.5) + `inviteAgent()` API function in `ui/src/lib/api.ts`. |
| 7 | Agent selector replaced | Kept agent selector for new chat. ParticipantBar added alongside for existing multi-agent sessions. |
| 8 | agentName not in SSE | Added `"agentName"` field to MessageStart, TextStart, ToolCallStart, Finish SSE events. Chat-store extracts and sets `agentId` on ChatMessage. |
| 9 | session_id access in tool | `processing_session_id` field on AgentEngine, set/cleared around handle_sse. Simple `Arc<Mutex<Option<Uuid>>>`. |
| 10 | Canvas keyed by agent | Changed to key by sessionId (with agent name fallback). Components pass `activeSessionId ?? agentName` as key. |
| 11 | Chat-store per-agent keying breaks multi-agent sessions | Added `sessionParticipantsCache` + session carry-over in `setCurrentAgent`. Switching agents within a multi-agent session preserves the active session. |
| + | ToolDef vs ToolDefinition | All code uses `ToolDefinition` from `hydeclaw-types` (3 fields: name, description, input_schema). No `required_base` field. |
| + | AgentSwitch SSE event | New `StreamEvent::AgentSwitch` variant for multi-turn agent identity tracking in converter task. |
