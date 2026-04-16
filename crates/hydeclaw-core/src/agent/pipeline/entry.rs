//! Pipeline step: entry — SSE streaming helpers (migrated from engine_sse.rs).
//!
//! These free functions handle SSE-specific concerns like extracting inline
//! markers from tool results and converting them into `StreamEvent`s.

use tokio::sync::mpsc;

use crate::agent::engine::{StreamEvent, FILE_PREFIX, RICH_CARD_PREFIX};

/// Result of processing a tool result for SSE event emission.
///
/// `display_result` is what goes into the LLM context (with markers stripped).
/// `db_result` is preserved verbatim for DB storage (markers intact).
pub struct ToolResultParts {
    pub display_result: String,
    pub db_result: String,
}

/// Extract inline markers (`__rich_card__:`, `__file__:`) from a tool result,
/// emit corresponding `StreamEvent`s, and return cleaned display + raw DB text.
///
/// Three cases:
/// 1. **Rich card** — entire result is a `__rich_card__:` JSON blob.
/// 2. **File markers** — one or more `__file__:` lines mixed with text.
/// 3. **Plain text** — no markers, returned as-is.
pub fn extract_tool_result_events(
    tool_result: &str,
    event_tx: &mpsc::UnboundedSender<StreamEvent>,
) -> ToolResultParts {
    if let Some(json_str) = tool_result.strip_prefix(RICH_CARD_PREFIX) {
        // Case 1: Rich card
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(json_str) {
            let card_type = data
                .get("card_type")
                .and_then(|v| v.as_str())
                .unwrap_or("table")
                .to_string();
            if event_tx
                .send(StreamEvent::RichCard { card_type, data })
                .is_err()
            {
                tracing::debug!("SSE event channel closed, engine continues for DB save");
            }
        }
        // Preserve raw marker in db_result so parts_builder can extract rich-card parts
        ToolResultParts {
            display_result: "Rich card displayed".to_string(),
            db_result: tool_result.to_string(),
        }
    } else if tool_result.contains(FILE_PREFIX) {
        // Case 2: File markers (possibly mixed with text)
        let db_result = tool_result.to_string();
        let mut clean_lines = Vec::new();
        for line in tool_result.lines() {
            if let Some(json_str) = line.strip_prefix(FILE_PREFIX) {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(json_str) {
                    let url = meta
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let media_type = meta
                        .get("mediaType")
                        .and_then(|v| v.as_str())
                        .unwrap_or("application/octet-stream");
                    if !url.is_empty()
                        && event_tx
                            .send(StreamEvent::File {
                                url: url.to_string(),
                                media_type: media_type.to_string(),
                            })
                            .is_err()
                    {
                        tracing::debug!("SSE event channel closed, engine continues for DB save");
                    }
                }
            } else {
                clean_lines.push(line);
            }
        }
        let text = clean_lines.join("\n");
        let display_result = if text.is_empty() {
            "Image displayed inline in the chat. Do NOT use canvas or other tools to show it again."
                .to_string()
        } else {
            text
        };
        ToolResultParts {
            display_result,
            db_result,
        }
    } else {
        // Case 3: Plain text
        ToolResultParts {
            display_result: tool_result.to_string(),
            db_result: tool_result.to_string(),
        }
    }
}
