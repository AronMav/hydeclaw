//! Assembles finalized `MessagePart[]` JSON for assistant messages.
//!
//! Called at assistant turn completion to persist the text/reasoning parts
//! so the frontend renders the final response identically on reload.
//! Tool calls are NOT included — they render via intermediate DB messages.

use serde_json::{json, Value};

/// Assemble text and reasoning parts from the final assistant response.
/// Tools, step-groups, and approvals are intentionally excluded —
/// they are rendered from the intermediate assistant/tool DB rows.
pub fn assemble_parts(content: &str) -> Value {
    let parts = parse_content_parts(content);
    json!(parts)
}

/// Parse content string into text and reasoning parts.
/// Handles `<think>...</think>` blocks.
fn parse_content_parts(content: &str) -> Vec<Value> {
    let mut parts = Vec::new();
    let mut remaining = content;

    while let Some(start) = remaining.find("<think>") {
        let before = &remaining[..start];
        if !before.trim().is_empty() {
            parts.push(json!({"type": "text", "text": before.trim()}));
        }
        remaining = &remaining[start + 7..];

        if let Some(end) = remaining.find("</think>") {
            let thinking = &remaining[..end];
            if !thinking.trim().is_empty() {
                parts.push(json!({"type": "reasoning", "text": thinking.trim()}));
            }
            remaining = &remaining[end + 8..];
        } else {
            if !remaining.trim().is_empty() {
                parts.push(json!({"type": "reasoning", "text": remaining.trim()}));
            }
            remaining = "";
        }
    }

    if !remaining.trim().is_empty() {
        parts.push(json!({"type": "text", "text": remaining.trim()}));
    }

    parts
}
