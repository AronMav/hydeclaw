use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse, Json,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::str::FromStr;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use super::super::{AppState, OpenAiMessage, sse_types};
use crate::agent::engine::StreamEvent;
use crate::tasks;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/api/mcp/callback", post(mcp_callback))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/v1/embeddings", post(embeddings_proxy))
        .route("/api/chat", post(api_chat_sse))
        .route("/api/chat/{id}/stream", get(api_chat_resume_stream))
        .route("/api/chat/{id}/abort", post(api_chat_abort))
}

// ── Streaming message RAII guard ──
// Ensures streaming messages are finalized in DB even if the converter task
// panics or exits unexpectedly (e.g. engine panic, tokio cancellation).

struct StreamingMessageGuard {
    db: sqlx::PgPool,
    msg_id: uuid::Uuid,
    session_id: Option<uuid::Uuid>,
    finalized: bool,
}

impl StreamingMessageGuard {
    fn new(db: sqlx::PgPool, msg_id: uuid::Uuid) -> Self {
        Self { db, msg_id, session_id: None, finalized: false }
    }
    fn set_session_id(&mut self, sid: uuid::Uuid) {
        self.session_id = Some(sid);
    }
    fn mark_finalized(&mut self) {
        self.finalized = true;
    }
}

impl Drop for StreamingMessageGuard {
    fn drop(&mut self) {
        if !self.finalized
            && let Some(_sid) = self.session_id {
                let db = self.db.clone();
                let mid = self.msg_id;
                tokio::spawn(async move {
                    if let Err(e) = crate::db::sessions::finalize_streaming_message(&db, mid).await {
                        tracing::warn!(error = %e, msg_id = %mid, "failed to finalize streaming message in guard Drop");
                    }
                });
            }
    }
}

// ── SSE flush helpers (bounded text accumulation + delta tools) ──

/// Build tools JSON from accumulated tools, reusing cached value when no new tools arrived.
/// Only calls `.to_vec()` when `accumulated_tools` actually grew since the last build.
fn build_tools_json(
    tools: &[serde_json::Value],
    flushed_count: &mut usize,
    cache: &mut Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    if tools.is_empty() {
        return None;
    }
    if cache.is_none() || tools.len() != *flushed_count {
        *cache = Some(serde_json::Value::Array(tools.to_vec()));
        *flushed_count = tools.len();
    }
    cache.clone()
}

/// Append-mode streaming message upsert. Text is APPENDED to existing content (not replaced).
/// Used for bounded text accumulation -- caller clears accumulated_text after success.
/// Also touches session activity for watchdog heartbeat, mirroring upsert_streaming_message behavior.
async fn upsert_streaming_append(
    db: &sqlx::PgPool,
    message_id: uuid::Uuid,
    session_id: uuid::Uuid,
    agent_id: &str,
    text_delta: &str,
    tool_calls: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, tool_calls, agent_id, status) \
         VALUES ($1, $2, 'assistant', $3, $4, $5, 'streaming') \
         ON CONFLICT (id) DO UPDATE SET content = messages.content || $3, tool_calls = $4",
    )
    .bind(message_id)
    .bind(session_id)
    .bind(text_delta)
    .bind(tool_calls)
    .bind(agent_id)
    .execute(db)
    .await?;
    // Maintain watchdog heartbeat -- mirrors what upsert_streaming_message does today.
    crate::db::sessions::touch_session_activity(db, session_id)
        .await
        .ok();
    Ok(())
}

/// Read the accumulated content from a streaming message row.
/// Used at Finish/Error/unexpected-exit to get full text for stream_jobs set_content,
/// since accumulated_text is cleared after each periodic flush.
async fn read_streaming_content(db: &sqlx::PgPool, message_id: uuid::Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT COALESCE(content, '') FROM messages WHERE id = $1")
        .bind(message_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

// ── OpenAI-compatible /v1/chat/completions ──

#[allow(dead_code)] // Deserialized from JSON; model/temperature reserved for future use
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionRequest {
    model: Option<String>,
    messages: Vec<OpenAiMessage>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    stream: bool,
    /// Agent to route to (HydeClaw extension). Defaults to first available.
    agent: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<ChatResponseChoice>,
    usage: Option<ChatResponseUsage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools_used: Vec<String>,
    iterations: u32,
}

#[derive(Debug, Serialize)]
struct ChatResponseChoice {
    index: u32,
    message: ChatResponseMessage,
    finish_reason: String,
}

#[derive(Debug, Serialize)]
struct ChatResponseMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatResponseUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

pub(crate) async fn chat_completions(
    State(state): State<AppState>,
    Json(req): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    // Route to agent: req.agent extension first, then req.model as agent name, then first available
    let engine = {
        let by_ext = req.agent.as_deref().filter(|s| !s.is_empty());
        let by_model = req.model.as_deref().filter(|s| !s.is_empty());
        match (by_ext, by_model) {
            (Some(name), _) => state.get_engine(name).await,
            (None, Some(name)) => {
                let e = state.get_engine(name).await;
                if e.is_some() { e } else { state.first_engine().await }
            }
            _ => state.first_engine().await,
        }
    };

    let engine = match engine {
        Some(e) => e,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": "no agent available", "type": "invalid_request_error"}})),
            )
                .into_response();
        }
    };

    let completion_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let model_name = engine.model_name();
    let created = chrono::Utc::now().timestamp();

    if req.stream {
        let (sse_tx, sse_rx) =
            tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(256);

        let messages = req.messages.clone();
        tokio::spawn(async move {
            let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

            let engine_clone = engine.clone();
            let handle = tokio::spawn(async move {
                engine_clone.handle_openai(&messages, Some(chunk_tx)).await
            });

            while let Some(chunk) = chunk_rx.recv().await {
                let data = json!({
                    "id": completion_id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": model_name,
                    "choices": [{"index": 0, "delta": {"content": chunk}, "finish_reason": null}]
                });
                if sse_tx.send(Ok(Event::default().data(data.to_string()))).await.is_err() { break; }
            }

            // Final stop chunk
            let data = json!({
                "id": completion_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model_name,
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            });
            let _ = sse_tx.send(Ok(Event::default().data(data.to_string()))).await;
            let _ = sse_tx.send(Ok(Event::default().data("[DONE]"))).await;

            if let Ok(Err(e)) = handle.await {
                tracing::error!(error = %e, "streaming chat completion error");
            }
        });

        return Sse::new(ReceiverStream::new(sse_rx))
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    // Non-streaming: pass full message history to handle_openai
    match engine.handle_openai(&req.messages, None).await {
        Ok(llm_resp) => {
            let usage = llm_resp.usage.map(|u| ChatResponseUsage {
                prompt_tokens: u.input_tokens,
                completion_tokens: u.output_tokens,
                total_tokens: u.input_tokens + u.output_tokens,
            });
            let resp = ChatCompletionResponse {
                id: completion_id,
                object: "chat.completion".to_string(),
                created,
                model: model_name,
                choices: vec![ChatResponseChoice {
                    index: 0,
                    message: ChatResponseMessage {
                        role: "assistant".to_string(),
                        content: llm_resp.content,
                    },
                    finish_reason: "stop".to_string(),
                }],
                usage,
                tools_used: llm_resp.tools_used,
                iterations: llm_resp.iterations,
            };
            Json(resp).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "chat completion error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": {"message": e.to_string(), "type": "server_error"}})),
            )
                .into_response()
        }
    }
}

// ── GET /v1/models ──

pub(crate) async fn list_models(State(state): State<AppState>) -> Json<Value> {
    let agents_map = state.agents.read().await;
    let data: Vec<Value> = agents_map
        .keys()
        .map(|name| {
            json!({
                "id": name,
                "object": "model",
                "created": 0,
                "owned_by": "hydeclaw"
            })
        })
        .collect();
    Json(json!({ "object": "list", "data": data }))
}

/// POST /v1/embeddings — proxy to configured embedding endpoint (OpenAI-compatible).
pub(crate) async fn embeddings_proxy(
    State(state): State<AppState>,
    Json(req): Json<Value>,
) -> impl IntoResponse {
    if !state.memory_store.is_available() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({
            "error": {"message": "embeddings not configured", "type": "server_error"}
        }))).into_response();
    }

    let input = req.get("input").cloned().unwrap_or(json!(""));
    let texts: Vec<String> = if let Some(arr) = input.as_array() {
        arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
    } else if let Some(s) = input.as_str() {
        vec![s.to_string()]
    } else {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": {"message": "input must be a string or array of strings", "type": "invalid_request_error"}
        }))).into_response();
    };

    if texts.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": {"message": "input must not be empty", "type": "invalid_request_error"}
        }))).into_response();
    }

    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    match state.memory_store.embed_batch(&refs).await {
        Ok(embeddings) => {
            let data: Vec<Value> = embeddings.iter().enumerate().map(|(i, emb)| {
                json!({"object": "embedding", "index": i, "embedding": emb})
            }).collect();
            Json(json!({
                "object": "list",
                "data": data,
                "model": state.memory_store.embed_model_name(),
                "usage": {"prompt_tokens": 0, "total_tokens": 0}
            })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
            "error": {"message": e.to_string(), "type": "server_error"}
        }))).into_response(),
    }
}

// ── AI SDK SSE Chat endpoint ──

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ChatSseRequest {
    messages: Vec<OpenAiMessage>,
    agent: Option<String>,
    session_id: Option<String>,
    /// Chat ID from AI SDK frontend (used as session_id alias for resume).
    #[serde(default)]
    id: Option<String>,
    /// Force creation of a new session (UI "New Chat" button).
    #[serde(default)]
    force_new_session: bool,
    /// When set, engine builds LLM context from the branch chain ending at this message.
    #[serde(default)]
    leaf_message_id: Option<String>,
    /// Optional file attachments from the UI.
    #[serde(default)]
    attachments: Vec<hydeclaw_types::MediaAttachment>,
}

#[allow(unused_assignments)]
pub(crate) async fn api_chat_sse(
    State(state): State<AppState>,
    Json(req): Json<ChatSseRequest>,
) -> impl IntoResponse {
    let agent_name = req.agent.clone().unwrap_or_default();
    let engine = if !agent_name.is_empty() {
        state.get_engine(&agent_name).await
    } else {
        state.first_engine().await
    };

    let engine = match engine {
        Some(e) => e,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "no agent available"})),
            )
                .into_response();
        }
    };

    // Find the LAST user message - support both content (v1) and parts (v3) formats
    let user_text = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| {
            // Try content field first (v1 format)
            if let Some(content) = &m.content
                && !content.is_empty() {
                    return Some(content.clone());
                }
            // Try parts field (AI SDK v3 format)
            if let Some(parts) = &m.parts {
                for part in parts.iter().rev() {
                    if part.part_type == "text"
                        && let Some(text) = &part.text
                            && !text.is_empty() {
                                return Some(text.clone());
                            }
                }
            }
            None
        })
        .unwrap_or_default();

    tracing::info!(
        messages_count = req.messages.len(),
        user_text_len = user_text.len(),
        "Processing chat request"
    );

    if req.messages.is_empty() {
        tracing::error!("Request messages array is empty");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "no messages provided"})),
        )
            .into_response();
    }

    let session_id = req
        .session_id
        .as_deref()
        .or(req.id.as_deref())
        .and_then(|s| uuid::Uuid::from_str(s).ok());
    let force_new_session = req.force_new_session && session_id.is_none();
    let user_text_for_title = user_text.clone();

    // ── @-mention routing ──────────────────────────────────────────
    // If user message contains @AgentName, route to that agent instead.
    let all_agent_names: Vec<String> = {
        let map = state.agents.read().await;
        map.keys().cloned().collect()
    };

    tracing::debug!(user_text = %user_text, agents = ?all_agent_names, "mention routing: checking");
    let (engine, cleaned_text, mentioned_agent) = if let Some(mentioned) =
        crate::agent::mention_parser::parse_first_mention(&user_text, &all_agent_names)
    {
        tracing::info!(mentioned = %mentioned, "mention routing: found @-mention");
        // Resolve the mentioned agent's engine
        let mentioned_engine = state.get_engine(&mentioned).await;
        match mentioned_engine {
            Some(eng) => {
                let cleaned = crate::agent::mention_parser::strip_mention(&user_text, &mentioned);
                (eng, cleaned, Some(mentioned))
            }
            None => (engine, user_text.clone(), None),
        }
    } else {
        tracing::debug!("mention routing: no @-mention found");
        (engine, user_text.clone(), None)
    };

    let _original_session_agent = agent_name.clone();
    // Update agent_name to the target agent (may differ from request if @-mention routed)
    let agent_name = engine.name().to_string();
    tracing::info!(agent_name = %agent_name, mentioned = ?mentioned_agent, "mention routing: final target agent");

    // Parse leaf_message_id for branch-aware context building
    let leaf_message_id = req.leaf_message_id.as_deref()
        .and_then(|s| uuid::Uuid::parse_str(s).ok());

    // Send cleaned text to LLM (without @mention prefix — prevents LLM from echoing it)
    let msg = hydeclaw_types::IncomingMessage {
        user_id: crate::agent::channel_kind::channel::UI.to_string(),
        text: Some(cleaned_text),
        attachments: vec![],
        agent_id: engine.name().to_string(),
        channel: crate::agent::channel_kind::channel::UI.to_string(),
        context: serde_json::Value::Null,
        timestamp: chrono::Utc::now(),
        formatting_prompt: None,
        tool_policy_override: None,
        leaf_message_id,
    };

    let (event_tx, mut event_rx) =
        tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    let (sse_tx, sse_rx) =
        tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(512);

    // Engine task: process message and emit StreamEvents.
    // Includes agent-to-agent turn loop: after each agent responds, check for @-mentions
    // and route to the next agent. Turn limit is configurable (global + per-agent override).
    let global_max_agent_turns = state.config.limits.max_agent_turns;
    let ui_tx = state.ui_event_tx.clone();
    let agent_for_broadcast = msg.agent_id.clone();
    let invite_db = state.db.clone();
    let mentioned_for_invite = mentioned_agent.clone();
    let agent_map = state.agents.clone();
    let all_agent_names_for_loop = all_agent_names.clone();
    let engine_handle = tokio::spawn(async move {
        let mut current_engine = engine;
        let initial_leaf_id = msg.leaf_message_id;
        let mut current_msg = msg;
        let mut current_session_id = session_id;
        let mut current_force_new = force_new_session;
        let mut turn_count = 0;
        let mut turn_chain: Vec<String> = Vec::new();
        let mut handoff_stack: Vec<(String, std::sync::Arc<crate::agent::engine::AgentEngine>)> = Vec::new();
        let mut current_leaf_id = initial_leaf_id;

        loop {
            let current_agent_name = current_engine.name().to_string();
            turn_chain.push(current_agent_name.clone());

            let assistant_msg_id = match current_engine.handle_sse(&current_msg, event_tx.clone(), current_session_id, current_force_new).await {
                Ok(id) => {
                    current_leaf_id = Some(id);
                    id
                }
                Err(e) => {
                    tracing::error!(error = %e, "SSE chat error (agent: {})", current_agent_name);
                    event_tx.send(StreamEvent::Error(e.to_string())).ok();
                    break;
                }
            };

            turn_count += 1;

            // Turn limit and cycle detection moved AFTER handoff routing checks (below)
            // to allow a final return-to-initiator before stopping.

            // Get session_id for turn loop. handle_sse clears processing_session_id
            // on return, so we read it BEFORE it's cleared by capturing it from the
            // session that was just used. For the first turn, current_session_id may be
            // None (new session), so we look up the latest session for this agent.
            let sid = if let Some(id) = current_session_id {
                id
            } else {
                // First turn created a new session — find it
                match crate::db::sessions::get_latest_session_id(current_engine.db_pool(), &current_agent_name).await {
                    Ok(Some(id)) => {
                        current_session_id = Some(id);
                        id
                    }
                    _ => break,
                }
            };

            let last_response = match crate::db::sessions::get_last_assistant_message(current_engine.db_pool(), sid).await {
                Ok(Some(text)) => {
                    tracing::debug!(session_id = %sid, response_len = text.len(), "turn loop: got last assistant message");
                    text
                }
                Ok(None) => {
                    tracing::info!(session_id = %sid, "turn loop: no assistant message found");
                    break;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "turn loop: failed to get last message");
                    break;
                }
            };

            // ── Routing: check handoff tool first, then @-mention fallback ──
            tracing::info!(
                agent = %current_agent_name,
                has_initiator = !handoff_stack.is_empty(),
                turn = turn_count,
                "turn loop: routing check"
            );

            // Priority 1: Structured handoff (D-06, D-07)
            let handoff = current_engine.take_handoff().await;
            if let Some(req) = handoff {
                let next_agent_name = req.target_agent.clone();

                // Resolve next agent's engine
                let next_engine = match agent_map.read().await.get(&next_agent_name) {
                    Some(h) => h.engine.clone(),
                    None => {
                        tracing::warn!(target = %next_agent_name, "handoff target agent not found, stopping turn loop");
                        break;
                    }
                };

                // Add participant (D-03: handoff adds participant AND transfers turn)
                let _ = crate::db::sessions::add_participant(current_engine.db_pool(), sid, &next_agent_name).await;

                // Signal agent switch to converter task (UI separator)
                event_tx.send(StreamEvent::AgentSwitch { agent_name: next_agent_name.clone() }).ok();
                event_tx.send(StreamEvent::RichCard {
                    card_type: "agent-turn".to_string(),
                    data: serde_json::json!({
                        "agentName": next_agent_name,
                        "reason": format!("handoff from {}", current_agent_name),
                    }),
                }).ok();

                // Truncate handoff context to prevent context window bloat (CTXA-04)
                let max_ctx_chars = state.config.limits.max_handoff_context_chars;
                let truncated_context = if req.context.len() > max_ctx_chars {
                    let mut end = max_ctx_chars;
                    // Avoid splitting UTF-8 characters
                    while !req.context.is_char_boundary(end) && end > 0 {
                        end -= 1;
                    }
                    format!("{}... [truncated]", &req.context[..end])
                } else {
                    req.context.clone()
                };

                // Build structured inter-agent message (D-14)
                let context_text = format!(
                    "[Handoff from {}]\nTask: {}\nContext: {}",
                    current_agent_name, req.task, truncated_context
                );
                current_msg = hydeclaw_types::IncomingMessage {
                    user_id: format!("agent:{}", current_agent_name),
                    text: Some(context_text),
                    attachments: vec![],
                    agent_id: next_agent_name.clone(),
                    channel: crate::agent::channel_kind::channel::INTER_AGENT.to_string(),
                    context: serde_json::json!({
                        "from": current_agent_name,
                        "task": req.task,
                        "context": truncated_context,
                        "handoff": true,
                    }),
                    timestamp: chrono::Utc::now(),
                    formatting_prompt: None,
                    tool_policy_override: None,
                    leaf_message_id: current_leaf_id,
                };
                // Push initiator onto stack so we can return control after target responds
                handoff_stack.push((current_agent_name.clone(), current_engine.clone()));
                current_engine = next_engine;
                current_session_id = Some(sid);
                current_force_new = false;
                continue;
            }

            // Priority 1b: Return control to handoff initiator after target agent responded.
            // Pop from stack — A→B→C returns to B, then B returns to A.
            if let Some((initiator_name, initiator_engine)) = handoff_stack.last().cloned() {
                if current_agent_name != initiator_name {
                    handoff_stack.pop(); // consume this level
                    tracing::info!(
                        from = %current_agent_name,
                        to = %initiator_name,
                        stack_depth = handoff_stack.len(),
                        "turn loop: returning control to handoff initiator"
                    );

                    event_tx.send(StreamEvent::AgentSwitch { agent_name: initiator_name.clone() }).ok();

                    let max_ctx = state.config.limits.max_handoff_context_chars;
                    let truncated_response = if last_response.len() > max_ctx {
                        let mut end = max_ctx;
                        while end > 0 && !last_response.is_char_boundary(end) { end -= 1; }
                        format!("{}... [truncated]", &last_response[..end])
                    } else {
                        last_response.clone()
                    };

                    current_msg = hydeclaw_types::IncomingMessage {
                        user_id: format!("agent:{}", current_agent_name),
                        text: Some(format!("[Response from {}]\n{}", current_agent_name, truncated_response)),
                        attachments: vec![],
                        agent_id: initiator_name.clone(),
                        channel: crate::agent::channel_kind::channel::INTER_AGENT.to_string(),
                        context: serde_json::json!({
                            "from": current_agent_name,
                            "response_from": current_agent_name,
                            "handoff_return": true,
                        }),
                        timestamp: chrono::Utc::now(),
                        formatting_prompt: None,
                        tool_policy_override: None,
                        leaf_message_id: current_leaf_id,
                    };
                    current_engine = initiator_engine;
                    current_session_id = Some(sid);
                    current_force_new = false;
                    continue;
                }
            }

            // Check turn limit and cycle detection AFTER routing checks to allow one last return
            let effective_limit = current_engine.max_agent_turns()
                .unwrap_or(global_max_agent_turns);
            if turn_count >= effective_limit {
                tracing::info!(
                    limit = effective_limit,
                    "Agent turn limit reached, stopping turn loop"
                );
                break;
            }

            // Cycle detection: stop if any agent appears 5+ times (slower but safer for complex tasks)
            {
                let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
                let mut cycle_detected = false;
                for name in &turn_chain {
                    let count = counts.entry(name.as_str()).or_insert(0);
                    *count += 1;
                    if *count >= 5 {
                        cycle_detected = true;
                        break;
                    }
                }
                if cycle_detected {
                    tracing::warn!(
                        chain = ?turn_chain,
                        "Cycle detected in agent turn loop, stopping"
                    );
                    event_tx.send(StreamEvent::Error(
                        format!("Agent turn loop stopped: cycle detected (agents: {})",
                            turn_chain.join(" -> "))
                    )).ok();
                    break;
                }
            }

            // Priority 2: @-mention fallback with self-mention filtering (D-09, D-10, D-11)
            let non_self_mentions: Vec<String> = crate::agent::mention_parser::parse_mentions(
                &last_response,
                &all_agent_names_for_loop,
            )
            .into_iter()
            .filter(|name| name != &current_agent_name)
            .collect();

            let next_agent = match non_self_mentions.into_iter().next() {
                Some(name) => {
                    tracing::info!(next = %name, "turn loop: found @-mention, routing to next agent");
                    name
                }
                None => {
                    tracing::info!("turn loop: no @-mention in response (self-mentions filtered), stopping");
                    break;
                }
            };

            // Resolve next agent's engine
            let next_engine = match agent_map.read().await.get(&next_agent) {
                Some(h) => h.engine.clone(),
                None => break,
            };

            // Auto-invite next agent into the session
            let _ = crate::db::sessions::add_participant(current_engine.db_pool(), sid, &next_agent).await;

            // Signal agent switch to the converter task (updates current_responding_agent)
            event_tx.send(StreamEvent::AgentSwitch { agent_name: next_agent.clone() }).ok();

            // Emit rich card so the UI shows a visual separator for the new agent turn
            event_tx.send(StreamEvent::RichCard {
                card_type: "agent-turn".to_string(),
                data: serde_json::json!({
                    "agentName": next_agent,
                    "reason": format!("mentioned by {}", current_agent_name),
                }),
            }).ok();

            // Build structured inter-agent message (D-14: consistent format for both paths)
            // Truncate context to max_handoff_context_chars (same as handoff path)
            let max_ctx = state.config.limits.max_handoff_context_chars;
            let truncated = if last_response.len() > max_ctx {
                format!("{}...(truncated)", &last_response[..last_response.floor_char_boundary(max_ctx)])
            } else {
                last_response.clone()
            };
            let context_text = format!(
                "[Handoff from {}]\nTask: (mentioned in response)\nContext: {}",
                current_agent_name, truncated
            );
            current_msg = hydeclaw_types::IncomingMessage {
                user_id: format!("agent:{}", current_agent_name),
                text: Some(context_text),
                attachments: vec![],
                agent_id: next_agent.clone(),
                channel: crate::agent::channel_kind::channel::INTER_AGENT.to_string(),
                context: serde_json::json!({
                    "from": current_agent_name,
                    "mentioned": true,
                }),
                timestamp: chrono::Utc::now(),
                formatting_prompt: None,
                tool_policy_override: None,
                leaf_message_id: None,
            };
            current_engine = next_engine;
            current_session_id = Some(sid);
            current_force_new = false;
        }

        // Notify UI about session update so sidebar refreshes
        let event = serde_json::json!({
            "type": "session_updated",
            "agent": agent_for_broadcast,
            "channel": crate::agent::channel_kind::channel::UI,
        });
        ui_tx.send(event.to_string()).ok();
    });

    // Converter task: StreamEvent → SSE JSON events (Vercel AI SDK v3 UI format)
    // Based on @ai-sdk/react UIMessageChunk types
    // Also buffers events in StreamRegistry for resume support.
    //
    // AUDIT:SSE-01 (verified 2026-03-30): Event ordering is guaranteed by single-task
    // sequential processing. The `while let Some(event) = event_rx.recv().await` loop
    // is the sole consumer of engine events. `pending_text_end` ensures text-end is
    // flushed before any non-text event (Finish, Error, ToolCallStart) -- see top of
    // loop (line ~469) and explicit flush in Finish handler. No concurrent emission
    // possible because event_rx.recv() processes one event at a time in this task.
    //
    // AUDIT:SSE-02 (verified 2026-03-30): Error delivery has two paths:
    // 1. LLM errors mid-stream: engine sends error as TextDelta via format_user_error()
    //    (intentional -- user sees error inline in chat history), then Finish event.
    // 2. handle_sse() top-level errors: chat.rs sends StreamEvent::Error via event_tx,
    //    converter sends error SSE event, marks stream as error in registry, finalizes
    //    the streaming message, then sends [DONE]. Client always receives error before
    //    connection close in both paths.
    let registry = state.stream_registry.clone();
    tokio::spawn(async move {
        let mut text_id_counter: usize = 0;
        let mut pending_text_end: Option<String> = None;
        let mut tool_name_map: HashMap<String, String> = HashMap::new();
        let mut session_id_str: Option<String> = None;
        // Tracks which agent is currently responding (updated on AgentSwitch)
        let mut current_responding_agent = agent_name.clone();
        tracing::debug!(current_responding_agent = %current_responding_agent, "converter: initial agent for SSE");
        #[allow(unused_assignments)]
        let mut client_gone_since: Option<std::time::Instant> = None;

        // Helper: send SSE event to client (if connected) and always buffer in registry
        macro_rules! send_and_buffer {
            ($json_str:expr) => {{
                if let Some(ref sid) = session_id_str {
                    registry.push_event(sid, &$json_str).await;
                }
                if !sse_tx.is_closed() {
                    client_gone_since = None;
                    sse_tx.send(Ok(Event::default().data($json_str))).await.is_ok()
                } else {
                    // Client disconnected — keep buffering for DB save + resume.
                    // Do NOT abort the engine: let it finish naturally so the result
                    // is saved to DB and the frontend picks it up via polling on reload.
                    // Engine has its own limits (max_iterations, subagent timeout).
                    if client_gone_since.is_none() {
                        client_gone_since = Some(std::time::Instant::now());
                        tracing::info!("SSE client disconnected, continuing engine for DB save");
                    }
                    true // always keep going — abort only via cancel API
                }
            }};
        }

        let mut finished_sent = false;
        let mut cancel_token: Option<CancellationToken> = None;
        let mut job_id: Option<uuid::Uuid> = None;
        let chat_db = state.db.clone();
        let mut accumulated_text = String::new();
        let mut accumulated_tools: Vec<serde_json::Value> = Vec::new();
        let mut tools_flushed_count: usize = 0;
        let mut cached_tools_json: Option<serde_json::Value> = None;
        // Periodic DB flush for streaming messages (LibreChat-style)
        let mut streaming_msg_id = uuid::Uuid::new_v4();
        let mut streaming_guard = StreamingMessageGuard::new(state.db.clone(), streaming_msg_id);
        let mut last_db_flush = std::time::Instant::now();
        let mut session_uuid: Option<uuid::Uuid> = None;
        let flush_interval = std::time::Duration::from_secs(2);
        while let Some(event) = event_rx.recv().await {
            // Abort engine on explicit cancel via API
            if cancel_token.as_ref().is_some_and(|t| t.is_cancelled()) {
                engine_handle.abort();
                break;
            }
            // AUDIT:SSE-03 (verified 2026-03-30): Safety net for client disconnect.
            // See stream_registry.rs for full SSE-03 audit. This 10-minute timeout
            // ensures no hanging tasks if client disconnects and never reconnects.
            // Safety net: abort if client gone for 10+ minutes (runaway engine protection)
            if client_gone_since.is_some_and(|t| t.elapsed().as_secs() > 600) {
                tracing::warn!("SSE client gone for 10min, aborting runaway engine");
                engine_handle.abort();
                break;
            }
            // If there's a pending text-end needed, send it first
            if let Some(text_id) = pending_text_end.take() {
                let end_data = json!({"type": sse_types::TEXT_END, "id": text_id}).to_string();
                let _ = send_and_buffer!(end_data);
            }

            let data = match event {
                StreamEvent::SessionId(sid) => {
                    let parsed_uuid = uuid::Uuid::from_str(&sid).ok();
                    // Register stream in registry for resume + abort support
                    if let Some(uuid) = parsed_uuid
                        && let Some((token, jid)) = registry.register(uuid, &agent_name).await {
                            cancel_token = Some(token);
                            job_id = Some(jid);
                        }
                    session_id_str = Some(sid.clone());
                    session_uuid = parsed_uuid;
                    if let Some(sid_uuid) = session_uuid {
                        streaming_guard.set_session_id(sid_uuid);
                    }
                    // Auto-invite the mentioned agent if it differs from the session owner
                    if let Some(sid_uuid) = session_uuid
                        && let Some(ref mentioned) = mentioned_for_invite
                    {
                        let db = invite_db.clone();
                        let agent = mentioned.clone();
                        tokio::spawn(async move {
                            let _ = crate::db::sessions::add_participant(&db, sid_uuid, &agent).await;
                        });
                    }
                    // Write empty streaming record immediately — gives frontend a persistent DB signal
                    // before the first token arrives. Single source of truth for "is agent thinking?".
                    if let Some(sid_uuid) = session_uuid
                        && let Err(e) = crate::db::sessions::upsert_streaming_message(
                            &chat_db, streaming_msg_id, sid_uuid, &agent_name, "", None
                        ).await {
                            tracing::warn!(error = %e, "failed to upsert initial streaming message to DB");
                        }
                    // Custom data part: session_id for UI to track the active session
                    json!({"type": sse_types::DATA_SESSION_ID, "data": {"sessionId": sid}, "transient": true})
                }
                StreamEvent::MessageStart { message_id } => {
                    json!({"type": sse_types::START, "messageId": message_id, "agentName": current_responding_agent})
                }
                StreamEvent::StepStart { step_id: _ } => {
                    continue;
                }
                StreamEvent::TextDelta(ref text) => {
                    if session_uuid.is_none() && accumulated_text.is_empty() {
                        tracing::error!("TextDelta received but session_uuid is None — DB flush will be skipped");
                    }
                    // AI SDK v3: text-start → text-delta → text-end
                    text_id_counter += 1;
                    let text_id = format!("text-{}", text_id_counter);
                    let start_data = json!({"type": sse_types::TEXT_START, "id": text_id.clone(), "agentName": current_responding_agent}).to_string();
                    let delta_data = json!({"type": sse_types::TEXT_DELTA, "id": text_id.clone(), "delta": text}).to_string();
                    let _ = send_and_buffer!(start_data);
                    let _ = send_and_buffer!(delta_data);
                    pending_text_end = Some(text_id);
                    accumulated_text.push_str(text);
                    // Periodic DB flush every 2s so reload shows partial response
                    // Uses append-mode SQL so accumulated_text can be cleared after flush (bounded memory)
                    if last_db_flush.elapsed() >= flush_interval
                        && let Some(sid) = session_uuid {
                            let tools_json = build_tools_json(&accumulated_tools, &mut tools_flushed_count, &mut cached_tools_json);
                            if let Err(e) = upsert_streaming_append(&chat_db, streaming_msg_id, sid, &agent_name, &accumulated_text, tools_json.as_ref()).await {
                                tracing::warn!(error = %e, "failed to flush streaming message to DB");
                            } else {
                                // Only clear after successful flush -- on failure, text stays for retry
                                accumulated_text.clear();
                            }
                            last_db_flush = std::time::Instant::now();
                        }
                    continue;
                }
                StreamEvent::ToolCallStart { id, name } => {
                    tool_name_map.insert(id.clone(), name.clone());
                    json!({
                        "type": sse_types::TOOL_INPUT_START,
                        "toolCallId": id,
                        "toolName": name,
                        "agentName": current_responding_agent,
                    })
                }
                StreamEvent::ToolCallArgs { id, args_text } => {
                    let delta_data = json!({
                        "type": sse_types::TOOL_INPUT_DELTA,
                        "toolCallId": id,
                        "inputTextDelta": args_text
                    }).to_string();
                    let _ = send_and_buffer!(delta_data);

                    let input: serde_json::Value = serde_json::from_str(&args_text)
                        .unwrap_or(serde_json::Value::Object(Default::default()));
                    let tool_name = tool_name_map.get(&id).cloned().unwrap_or_default();
                    json!({
                        "type": sse_types::TOOL_INPUT_AVAILABLE,
                        "toolCallId": id,
                        "toolName": tool_name,
                        "input": input
                    })
                }
                StreamEvent::ToolResult { ref id, ref result } => {
                    // Accumulate tool calls in-memory (single DB write at finish)
                    let tname = tool_name_map.get(id).cloned().unwrap_or_default();
                    accumulated_tools.push(json!({"toolCallId": id, "toolName": tname, "output": result}));
                    cached_tools_json = None; // Invalidate cache when new tool arrives
                    json!({
                        "type": sse_types::TOOL_OUTPUT_AVAILABLE,
                        "toolCallId": id,
                        "output": result
                    })
                }
                StreamEvent::StepFinish { step_id: _, finish_reason: _ } => {
                    continue;
                }
                StreamEvent::RichCard { card_type, data } => {
                    json!({
                        "type": sse_types::RICH_CARD,
                        "cardType": card_type,
                        "data": data
                    })
                }
                StreamEvent::File { url, media_type } => {
                    json!({
                        "type": sse_types::FILE,
                        "url": url,
                        "mediaType": media_type
                    })
                }
                StreamEvent::AgentSwitch { agent_name: new_agent } => {
                    current_responding_agent = new_agent;
                    continue; // Internal event — don't emit SSE
                }
                StreamEvent::ApprovalNeeded { approval_id, tool_name, tool_input, timeout_ms } => {
                    let data = json!({
                        "type": sse_types::APPROVAL_NEEDED,
                        "approvalId": approval_id,
                        "toolName": tool_name,
                        "toolInput": tool_input,
                        "timeoutMs": timeout_ms,
                    }).to_string();
                    let _ = send_and_buffer!(data);
                    continue;
                }
                StreamEvent::ApprovalResolved { approval_id, action, modified_input } => {
                    let data = json!({
                        "type": sse_types::APPROVAL_RESOLVED,
                        "approvalId": approval_id,
                        "action": action,
                        "modifiedInput": modified_input,
                    }).to_string();
                    let _ = send_and_buffer!(data);
                    continue;
                }
                StreamEvent::Finish { .. } => {
                    // Send any pending text-end first
                    if let Some(text_id) = pending_text_end.take() {
                        let end_data = json!({"type": sse_types::TEXT_END, "id": text_id}).to_string();
                        let _ = send_and_buffer!(end_data);
                    }
                    let finish_data = json!({"type": sse_types::FINISH, "agentName": current_responding_agent}).to_string();
                    let _ = send_and_buffer!(finish_data);
                    // Final flush of streaming message + mark complete
                    // CRITICAL ORDERING: upsert → read_streaming_content → set_content → finalize (DELETE)
                    if let Some(sid) = session_uuid {
                        let tools_json = build_tools_json(&accumulated_tools, &mut tools_flushed_count, &mut cached_tools_json);
                        // Step 1: Flush remaining text delta to streaming message (APPEND mode)
                        if let Err(e) = upsert_streaming_append(&chat_db, streaming_msg_id, sid, &agent_name, &accumulated_text, tools_json.as_ref()).await {
                            tracing::warn!(error = %e, "failed to upsert streaming message on Finish");
                        }
                        // Step 2: Read back full aggregated text BEFORE the row is deleted
                        let full_text = read_streaming_content(&chat_db, streaming_msg_id).await;
                        // Step 3: Persist full content to stream_jobs (needs complete text)
                        if let Some(jid) = job_id
                            && let Err(e) = crate::gateway::stream_jobs::set_content(&chat_db, jid, &full_text, &accumulated_tools).await {
                                tracing::warn!(error = %e, "failed to set stream job content on Finish");
                            }
                        // Step 4: NOW safe to finalize (DELETE) the streaming message row
                        if let Err(e) = crate::db::sessions::finalize_streaming_message(&chat_db, streaming_msg_id).await {
                            tracing::warn!(error = %e, "failed to finalize streaming message on Finish");
                        }
                    }
                    streaming_guard.mark_finalized();
                    // DON'T break here — agent-to-agent turn loop may send more events
                    // (AgentSwitch → MessageStart → TextDelta → Finish for next agent).
                    // The loop exits naturally when event_tx is dropped (engine task completes).
                    // Send [DONE] only after all turns are done (handled in post-loop block).
                    // Reset accumulated state for next agent turn:
                    accumulated_text.clear();
                    accumulated_tools.clear();
                    tools_flushed_count = 0;
                    cached_tools_json = None;
                    streaming_msg_id = uuid::Uuid::new_v4();
                    text_id_counter = 0;
                    continue;
                }
                StreamEvent::Error(ref text) => {
                    let err_data = json!({"type": sse_types::ERROR, "errorText": text}).to_string();
                    let _ = send_and_buffer!(err_data);
                    if let Some(ref sid) = session_id_str {
                        registry.mark_error(sid, text).await;
                    }
                    // Finalize streaming message on error too
                    // CRITICAL ORDERING: upsert → read_streaming_content → set_content → finalize (DELETE)
                    if let Some(sid) = session_uuid {
                        let tools_json = build_tools_json(&accumulated_tools, &mut tools_flushed_count, &mut cached_tools_json);
                        // Step 1: Flush remaining text delta (APPEND mode)
                        if let Err(e) = upsert_streaming_append(&chat_db, streaming_msg_id, sid, &agent_name, &accumulated_text, tools_json.as_ref()).await {
                            tracing::warn!(error = %e, "failed to upsert streaming message on Error");
                        }
                        // Step 2: Read back full aggregated text BEFORE the row is deleted
                        let full_text = read_streaming_content(&chat_db, streaming_msg_id).await;
                        // Step 3: Persist full content to stream_jobs
                        if let Some(jid) = job_id
                            && let Err(e) = crate::gateway::stream_jobs::set_content(&chat_db, jid, &full_text, &accumulated_tools).await {
                                tracing::warn!(error = %e, "failed to set stream job content on Error");
                            }
                        // Step 4: NOW safe to finalize (DELETE) the streaming message row
                        if let Err(e) = crate::db::sessions::finalize_streaming_message(&chat_db, streaming_msg_id).await {
                            tracing::warn!(error = %e, "failed to finalize streaming message on Error");
                        }
                    }
                    streaming_guard.mark_finalized();
                    finished_sent = true;
                    break;
                }
            };

            let json_str = data.to_string();
            let _ = send_and_buffer!(json_str);
        }

        // Only send [DONE] and mark_finished if the Finish branch didn't already do it
        if !finished_sent {
            // Finalize streaming message on unexpected exit
            // CRITICAL ORDERING: upsert → read_streaming_content → set_content → finalize (DELETE)
            if let Some(sid) = session_uuid {
                let tools_json = build_tools_json(&accumulated_tools, &mut tools_flushed_count, &mut cached_tools_json);
                // Step 1: Flush remaining text delta (APPEND mode)
                if let Err(e) = upsert_streaming_append(&chat_db, streaming_msg_id, sid, &agent_name, &accumulated_text, tools_json.as_ref()).await {
                    tracing::warn!(error = %e, "failed to upsert streaming message on unexpected exit");
                }
                // Step 2: Read back full aggregated text BEFORE the row is deleted
                let full_text = read_streaming_content(&chat_db, streaming_msg_id).await;
                // Step 3: Persist full content to stream_jobs
                if let Some(jid) = job_id
                    && let Err(e) = crate::gateway::stream_jobs::set_content(&chat_db, jid, &full_text, &accumulated_tools).await {
                        tracing::warn!(error = %e, "failed to set stream job content on unexpected exit");
                    }
                // Step 4: NOW safe to finalize (DELETE) the streaming message row
                if let Err(e) = crate::db::sessions::finalize_streaming_message(&chat_db, streaming_msg_id).await {
                    tracing::warn!(error = %e, "failed to finalize streaming message on unexpected exit");
                }
            }
            streaming_guard.mark_finalized();
            if let Some(ref sid) = session_id_str {
                registry.mark_finished(sid).await;
            }
            // Flush any remaining text-end (if stream ended without Finish event)
            if let Some(text_id) = pending_text_end {
                let end_data = json!({"type": sse_types::TEXT_END, "id": text_id});
                let _ = sse_tx.send(Ok(Event::default().data(end_data.to_string()))).await;
            }
            let _ = sse_tx.send(Ok(Event::default().data("[DONE]"))).await;
        }

        // Auto-title: set session title from first user message if not already titled
        if let Some(sid) = session_uuid {
            let title_db = chat_db.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::db::sessions::auto_title_session(&title_db, sid, &user_text_for_title).await {
                    tracing::debug!(error = %e, "auto-title failed");
                }
            });
        }
    });

    let stream = ReceiverStream::new(sse_rx);

    (
        [(
            axum::http::header::HeaderName::from_static("x-vercel-ai-ui-message-stream"),
            "v1",
        )],
        Sse::new(stream).keep_alive(KeepAlive::default()),
    )
        .into_response()
}

// ── Stream Resume endpoint ──

/// Resume an active SSE stream by session ID.
/// AI SDK calls GET /api/chat/{id}/stream on mount when resume=true.
/// Returns 204 if no active stream, or SSE with replay + live events.
pub(crate) async fn api_chat_resume_stream(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    use async_stream::stream;
    use tokio::sync::broadcast;

    match state.stream_registry.subscribe(&id).await {
        None => {
            // No in-memory stream — check DB for recently finished/interrupted job
            let session_uuid = uuid::Uuid::parse_str(&id).ok();
            if let Some(sid) = session_uuid
                && let Ok(Some(job)) = crate::gateway::stream_jobs::get_active_job(
                    state.stream_registry.db(), sid
                ).await {
                    let sync_status = match job.status.as_str() {
                        "finished" => "finished",
                        "error" => "error",
                        "running" => {
                            // Running in DB but not in memory = Core restarted mid-stream
                            if let Err(e) = crate::gateway::stream_jobs::error_job(
                                state.stream_registry.db(), job.id, "stream lost: core restarted"
                            ).await {
                                tracing::warn!(error = %e, "failed to mark stream job as error on resume");
                            }
                            "interrupted"
                        }
                        _ => "error",
                    };
                    let sync = serde_json::json!({
                        "type": sse_types::SYNC,
                        "content": job.aggregated_text,
                        "toolCalls": job.tool_calls,
                        "status": sync_status,
                        "error": job.error_text,
                    });
                    let sync_str = sync.to_string();
                    let sse_stream = async_stream::stream! {
                        yield Ok::<_, std::convert::Infallible>(Event::default().data(sync_str));
                        yield Ok(Event::default().data("[DONE]"));
                    };
                    return Sse::new(sse_stream)
                        .keep_alive(KeepAlive::default())
                        .into_response();
                }
            StatusCode::NO_CONTENT.into_response()
        }
        Some((buffered_events, mut broadcast_rx, already_finished)) => {
            let replay_count = buffered_events.len();

            let sse_stream = stream! {
                // Phase 1: Replay buffered events
                for (_id, event_json) in buffered_events {
                    yield Ok::<_, std::convert::Infallible>(
                        Event::default().data(event_json)
                    );
                }

                if already_finished {
                    yield Ok(Event::default().data("[DONE]"));
                    return;
                }

                // Phase 2: Live events via broadcast subscription
                // Events between subscribe() and here may overlap with buffer — skip them
                let mut skip_remaining = replay_count;
                loop {
                    match broadcast_rx.recv().await {
                        Ok((_id, event_json)) => {
                            if skip_remaining > 0 {
                                skip_remaining -= 1;
                                continue;
                            }
                            let is_terminal =
                                event_json.contains("\"type\":\"finish\"")
                                || event_json.contains("\"type\":\"error\"");
                            yield Ok(Event::default().data(event_json));
                            if is_terminal {
                                yield Ok(Event::default().data("[DONE]"));
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                lagged = n,
                                session = %id,
                                "Resume stream lagged"
                            );
                            skip_remaining = skip_remaining.saturating_sub(n as usize);
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            break;
                        }
                    }
                }
            };

            (
                [(
                    axum::http::header::HeaderName::from_static(
                        "x-vercel-ai-ui-message-stream",
                    ),
                    "v1",
                )],
                Sse::new(sse_stream).keep_alive(KeepAlive::default()),
            )
                .into_response()
        }
    }
}

// ── Per-session model override ──

#[derive(Debug, serde::Deserialize)]
pub(crate) struct ModelOverrideBody {
    model: Option<String>,
}

pub(crate) async fn set_model_override(
    State(state): State<AppState>,
    Path(agent_name): Path<String>,
    Json(body): Json<ModelOverrideBody>,
) -> impl IntoResponse {
    let Some(engine) = state.get_engine(&agent_name).await else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":"not found"}))).into_response();
    };
    engine.set_model_override(body.model.clone());
    let current = engine.current_model();
    Json(serde_json::json!({"model": current})).into_response()
}

pub(crate) async fn health(State(state): State<AppState>) -> Json<Value> {
    let db_ok = sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .is_ok();

    let config = state.shared_config.read().await;

    // Agent names and icons are intentionally omitted here — /health is unauthenticated
    // and must not leak information about which agents are configured.
    // Authenticated callers should use GET /api/agents instead.
    Json(json!({
        "status": if db_ok { "ok" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "db": db_ok,
        "listen": config.gateway.listen,
    }))
}

pub(crate) async fn mcp_callback(
    State(state): State<AppState>,
    Json(payload): Json<hydeclaw_types::McpCallback>,
) -> StatusCode {
    tracing::info!(
        task_id = %payload.task_id,
        status = %payload.status,
        "MCP callback received"
    );

    if let Err(e) = tasks::update_step_from_callback(&state.db, &payload).await {
        tracing::error!(error = %e, "failed to process MCP callback");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    StatusCode::OK
}

/// POST /api/chat/{id}/abort — cancel an in-progress stream from any client.
pub(crate) async fn api_chat_abort(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let cancelled = state.stream_registry.cancel(&session_id).await;
    if cancelled {
        tracing::info!(session_id = %session_id, "stream cancelled via API");
        Json(json!({"ok": true, "message": "stream cancelled"})).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(json!({"error": "no active stream for this session"}))).into_response()
    }
}
