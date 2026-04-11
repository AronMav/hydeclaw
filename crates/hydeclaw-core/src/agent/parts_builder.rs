//! Assembles finalized `MessagePart[]` JSON for assistant messages.
//!
//! Called at assistant turn completion to persist the parts array that the
//! frontend renders identically in both live and history modes.

use anyhow::Result;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// Assemble parts from an assistant message's content, tool calls, tool results,
/// approvals, and inline markers (__file__, __rich_card__).
pub fn assemble_parts(
    content: &str,
    tool_calls: Option<&Value>,
    tool_results: &[(String, String)],  // (tool_call_id, result_text)
    approvals: &[ApprovalInfo],
    step_groups: &[StepGroupInfo],
) -> Value {
    let mut parts: Vec<Value> = Vec::new();

    // 1. Parse content into text + reasoning parts
    let content_parts = parse_content_parts(content);
    parts.extend(content_parts);

    // 2. Build tool call map: id -> (name, args)
    let tool_map = build_tool_map(tool_calls);

    // 3. Build tool result map: id -> output
    let result_map: std::collections::HashMap<&str, &str> = tool_results
        .iter()
        .map(|(id, result)| (id.as_str(), result.as_str()))
        .collect();

    // 4. If step groups exist, emit them with nested tools
    let mut grouped_tool_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for sg in step_groups {
        let mut tool_parts = Vec::new();
        for tc_id in &sg.tool_call_ids {
            grouped_tool_ids.insert(tc_id.clone());
            let (name, args) = tool_map.get(tc_id.as_str()).cloned().unwrap_or(("tool".to_string(), json!({})));
            let (output, extra) = match result_map.get(tc_id.as_str()) {
                Some(s) => { let (o, e) = extract_clean_output(s); (Some(o), e) }
                None => (None, vec![]),
            };

            // Check if this tool had an approval
            if let Some(approval) = approvals.iter().find(|a| a.tool_call_id.as_deref() == Some(tc_id.as_str())) {
                tool_parts.push(json!({
                    "type": "approval",
                    "approvalId": approval.id.to_string(),
                    "toolName": name,
                    "toolInput": args,
                    "timeoutMs": 0,
                    "receivedAt": 0,
                    "status": approval.status,
                }));
            }

            tool_parts.push(json!({
                "type": "tool",
                "toolCallId": tc_id,
                "toolName": name,
                "state": "output-available",
                "input": args,
                "output": output,
            }));
            tool_parts.extend(extra);
        }
        parts.push(json!({
            "type": "step-group",
            "stepId": sg.step_id,
            "toolParts": tool_parts,
            "finishReason": sg.finish_reason,
            "isStreaming": false,
        }));
    }

    // 5. Emit ungrouped tool calls (not in any step group)
    if let Some(calls) = tool_calls.and_then(|v| v.as_array()) {
        for tc in calls {
            let tc_id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if grouped_tool_ids.contains(tc_id) {
                continue;
            }
            let name = tc.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
            let args = tc.get("arguments").cloned().unwrap_or(json!({}));
            let (output, extra) = match result_map.get(tc_id) {
                Some(s) => { let (o, e) = extract_clean_output(s); (Some(o), e) }
                None => (None, vec![]),
            };

            // Check for approval
            if let Some(approval) = approvals.iter().find(|a| a.tool_call_id.as_deref() == Some(tc_id)) {
                parts.push(json!({
                    "type": "approval",
                    "approvalId": approval.id.to_string(),
                    "toolName": name,
                    "toolInput": args,
                    "timeoutMs": 0,
                    "receivedAt": 0,
                    "status": approval.status,
                }));
            }

            parts.push(json!({
                "type": "tool",
                "toolCallId": tc_id,
                "toolName": name,
                "state": "output-available",
                "input": args,
                "output": output,
            }));
            parts.extend(extra);
        }
    }

    json!(parts)
}

/// Parse content string into text and reasoning parts.
/// Handles `<think>...</think>` blocks.
fn parse_content_parts(content: &str) -> Vec<Value> {
    let mut parts = Vec::new();
    let mut remaining = content;

    while let Some(start) = remaining.find("<think>") {
        // Text before <think>
        let before = &remaining[..start];
        if !before.trim().is_empty() {
            parts.push(json!({"type": "text", "text": before.trim()}));
        }
        remaining = &remaining[start + 7..]; // skip "<think>"

        if let Some(end) = remaining.find("</think>") {
            let thinking = &remaining[..end];
            if !thinking.trim().is_empty() {
                parts.push(json!({"type": "reasoning", "text": thinking.trim()}));
            }
            remaining = &remaining[end + 8..]; // skip "</think>"
        } else {
            // Unclosed <think> — treat rest as reasoning
            if !remaining.trim().is_empty() {
                parts.push(json!({"type": "reasoning", "text": remaining.trim()}));
            }
            remaining = "";
        }
    }

    // Remaining text after last </think>
    if !remaining.trim().is_empty() {
        parts.push(json!({"type": "text", "text": remaining.trim()}));
    }

    parts
}

/// Extract file and rich-card markers from tool output.
/// Returns (cleaned_text, extra_parts) where extra_parts are typed file/rich-card parts.
fn extract_clean_output(raw: &str) -> (Value, Vec<Value>) {
    let mut clean_lines = Vec::new();
    let mut extra_parts = Vec::new();

    for line in raw.lines() {
        if let Some(json_str) = line.strip_prefix("__file__:") {
            if let Ok(meta) = serde_json::from_str::<Value>(json_str) {
                if let Some(url) = meta.get("url").and_then(|v| v.as_str()) {
                    let media_type = meta.get("mediaType").and_then(|v| v.as_str()).unwrap_or("application/octet-stream");
                    extra_parts.push(json!({
                        "type": "file",
                        "url": url,
                        "mediaType": media_type,
                    }));
                } else {
                    clean_lines.push(line); // malformed marker — keep as text
                }
            } else {
                clean_lines.push(line); // unparseable JSON — keep as text
            }
        } else if let Some(json_str) = line.strip_prefix("__rich_card__:") {
            if let Ok(meta) = serde_json::from_str::<Value>(json_str) {
                // Producer uses snake_case "card_type" (engine_tools.rs), consistent with engine_sse.rs
                let card_type = meta.get("card_type")
                    .or_else(|| meta.get("cardType"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let data = meta.get("data").cloned().unwrap_or(json!({}));
                extra_parts.push(json!({
                    "type": "rich-card",
                    "cardType": card_type,
                    "data": data,
                }));
            } else {
                clean_lines.push(line); // unparseable JSON — keep as text
            }
        } else {
            clean_lines.push(line);
        }
    }

    (json!(clean_lines.join("\n")), extra_parts)
}

/// Build map of tool_call_id -> (name, arguments) from tool_calls JSON array.
fn build_tool_map(tool_calls: Option<&Value>) -> std::collections::HashMap<String, (String, Value)> {
    let mut map = std::collections::HashMap::new();
    if let Some(calls) = tool_calls.and_then(|v| v.as_array()) {
        for tc in calls {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = tc.get("name").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
            let args = tc.get("arguments").cloned().unwrap_or(json!({}));
            map.insert(id, (name, args));
        }
    }
    map
}

/// Load resolved approvals for a session from the pending_approvals table.
pub async fn load_session_approvals(db: &PgPool, session_id: Uuid) -> Result<Vec<ApprovalInfo>> {
    let rows = sqlx::query_as::<_, ApprovalRow>(
        "SELECT id, tool_name, tool_args, status, context \
         FROM pending_approvals \
         WHERE session_id = $1 AND status != 'pending' \
         ORDER BY requested_at ASC",
    )
    .bind(session_id)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().map(|r| {
        let tool_call_id = r.context
            .as_ref()
            .and_then(|c| c.get("tool_call_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        ApprovalInfo {
            id: r.id,
            tool_name: r.tool_name,
            status: r.status,
            tool_call_id,
        }
    }).collect())
}

#[derive(Debug)]
pub struct ApprovalInfo {
    pub id: Uuid,
    pub tool_name: String,
    pub status: String,
    pub tool_call_id: Option<String>,
}

#[derive(Debug)]
pub struct StepGroupInfo {
    pub step_id: String,
    pub tool_call_ids: Vec<String>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ApprovalRow {
    id: Uuid,
    tool_name: String,
    #[allow(dead_code)]
    tool_args: Value,
    status: String,
    context: Option<Value>,
}
