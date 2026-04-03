use anyhow::Result;
use hydeclaw_types::{Message, MessageRole, ToolDefinition};

use super::providers::LlmProvider;

/// Estimate token count from text (rough: ~4 chars per token).
/// Accounts for tool_calls JSON size when present.
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            let content_tokens = m.content.len() / 4 + 10;
            let tool_tokens = m
                .tool_calls
                .as_ref()
                .map(|tc| {
                    tc.iter()
                        .map(|t| {
                            let args_len = t.arguments.to_string().len();
                            let name_len = t.name.len();
                            (args_len + name_len + 20) / 4
                        })
                        .sum::<usize>()
                })
                .unwrap_or(0);
            content_tokens + tool_tokens
        })
        .sum()
}

/// Compact conversation history if it exceeds the threshold.
/// Extracts facts into memory-ready strings and replaces old messages with a summary.
/// `agent_language` is used to instruct the LLM to summarize in the correct language.
pub async fn compact_if_needed(
    messages: &mut Vec<Message>,
    provider: &dyn LlmProvider,
    compaction_provider: Option<&dyn LlmProvider>,
    max_tokens: usize,
    preserve_last_n: usize,
    agent_language: Option<&str>,
) -> Result<Option<Vec<String>>> {
    // Use dedicated compaction provider if available, otherwise fall back to main provider.
    let active_provider: &dyn LlmProvider = compaction_provider.unwrap_or(provider);
    let total = estimate_tokens(messages);
    let threshold = max_tokens * 80 / 100;

    if total < threshold {
        return Ok(None);
    }

    tracing::info!(
        total_tokens = total,
        threshold,
        messages = messages.len(),
        "context window threshold reached, compacting"
    );

    // Keep system message (first) and last N messages
    let system_msg = if !messages.is_empty() && messages[0].role == MessageRole::System {
        Some(messages[0].clone())
    } else {
        None
    };

    let start = if system_msg.is_some() { 1 } else { 0 };
    let mut end = if messages.len() > start + preserve_last_n {
        messages.len() - preserve_last_n
    } else {
        return Ok(None); // Not enough messages to compact
    };

    // Don't split in the middle of a tool call group:
    // move `end` backward until messages[end] is not a Tool message.
    while end > start && messages[end].role == MessageRole::Tool {
        end -= 1;
    }
    // If we also landed on an Assistant with tool_calls, include it in preserved part
    if end > start && messages[end].role == MessageRole::Assistant && messages[end].tool_calls.is_some() {
        end -= 1;
    }
    if end <= start {
        return Ok(None); // Not enough messages to compact after adjustment
    }

    let to_compact: Vec<Message> = messages[start..end].to_vec();
    if to_compact.is_empty() {
        return Ok(None);
    }

    let formatted = format_messages_for_compaction(&to_compact);

    // Step 1: Extract facts for long-term memory
    let lang_hint = match agent_language {
        Some("ru") => " Write each fact in Russian.",
        Some("en") => " Write each fact in English.",
        _ => "",
    };
    let extraction_prompt = vec![
        Message {
            role: MessageRole::System,
            content: format!(
                "Extract key facts from this conversation as a JSON array of strings.\n\n\
                MUST PRESERVE:\n\
                - Active tasks with their current status and progress (e.g. '5/17 items done')\n\
                - All identifiers: UUIDs, URLs, file paths, IPs, hostnames, port numbers, service names\n\
                - Decisions made and their rationale\n\
                - User preferences and requirements discovered\n\
                - Error conditions encountered and their resolutions\n\
                - Commitments, action items, and deadlines\n\n\
                MAY OMIT:\n\
                - Routine greetings and confirmations\n\
                - Tool calls that succeeded without noteworthy results\n\
                - Repeated information already captured in other facts\n\n\
                Each fact must be self-contained and useful without the original conversation.{}\n\
                Return ONLY the JSON array, no other text.",
                lang_hint
            ),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        },
        Message {
            role: MessageRole::User,
            content: formatted.clone(),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        },
    ];

    let empty_tools: Vec<ToolDefinition> = vec![];
    let facts_response = active_provider.chat(&extraction_prompt, &empty_tools).await?;
    let extracted_facts: Vec<String> =
        serde_json::from_str(&facts_response.content).unwrap_or_default();

    tracing::info!(facts = extracted_facts.len(), "extracted facts from history");

    // Step 2: Summarize for context continuity
    let summary_lang = match agent_language {
        Some("en") => "in English",
        _ => "in Russian",
    };
    let summary_prompt = vec![
        Message {
            role: MessageRole::System,
            content: format!(
                "Summarize this conversation concisely {}. Structure:\n\
                1. Active tasks and their progress\n\
                2. Key decisions made\n\
                3. Open questions or blockers\n\n\
                Preserve exact identifiers: UUIDs, URLs, file paths, IPs, hostnames, port numbers.\n\
                Be brief — 2-3 paragraphs max.",
                summary_lang
            ),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        },
        Message {
            role: MessageRole::User,
            content: formatted,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        },
    ];

    let summary_response = active_provider.chat(&summary_prompt, &empty_tools).await?;

    // Step 3: Rebuild messages — system + summary + preserved recent
    let preserved: Vec<Message> = messages[end..].to_vec();
    messages.clear();

    if let Some(sys) = system_msg {
        messages.push(sys);
    }

    messages.push(Message {
        role: MessageRole::System,
        content: format!(
            "[Previous conversation summary]\n{}",
            summary_response.content
        ),
        tool_calls: None,
        tool_call_id: None,
        thinking_blocks: vec![],
    });

    messages.extend(preserved);

    tracing::info!(
        new_messages = messages.len(),
        new_tokens = estimate_tokens(messages),
        "compaction complete"
    );

    if extracted_facts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(extracted_facts))
    }
}

/// Format messages for compaction prompt.
fn format_messages_for_compaction(messages: &[Message]) -> String {
    let mut formatted = String::new();
    for msg in messages {
        let role = match msg.role {
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
            MessageRole::System => "System",
            MessageRole::Tool => "Tool",
        };
        formatted.push_str(&format!("[{}]: {}\n\n", role, msg.content));
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;
    use hydeclaw_types::{Message, MessageRole, ToolCall};

    fn make_message(role: MessageRole, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        }
    }

    #[test]
    fn estimate_tokens_empty_slice() {
        assert_eq!(estimate_tokens(&[]), 0);
    }

    #[test]
    fn estimate_tokens_single_user_message() {
        let msg = make_message(MessageRole::User, "hello");
        // "hello".len() = 5, 5 / 4 + 10 = 11
        assert_eq!(estimate_tokens(&[msg]), 11);
    }

    #[test]
    fn estimate_tokens_multiple_messages() {
        let msgs = vec![
            make_message(MessageRole::User, "hello"),       // 5/4+10 = 11
            make_message(MessageRole::Assistant, "world!!"), // 7/4+10 = 11
            make_message(MessageRole::System, ""),           // 0/4+10 = 10
        ];
        let individual: usize = msgs.iter().map(|m| estimate_tokens(std::slice::from_ref(m))).sum();
        assert_eq!(estimate_tokens(&msgs), individual);
        assert_eq!(estimate_tokens(&msgs), 11 + 11 + 10);
    }

    #[test]
    fn estimate_tokens_with_tool_calls() {
        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: serde_json::json!({"city": "Moscow"}),
        };
        let msg = Message {
            role: MessageRole::Assistant,
            content: "".to_string(),
            tool_calls: Some(vec![tool_call.clone()]),
            tool_call_id: None,
            thinking_blocks: vec![],
        };

        let content_tokens = 10; // 0 / 4 + 10
        let args_str = tool_call.arguments.to_string();
        let tool_tokens = (args_str.len() + tool_call.name.len() + 20) / 4;
        let expected = content_tokens + tool_tokens;

        assert_eq!(estimate_tokens(&[msg]), expected);
        assert!(expected > 10, "tool calls should add tokens beyond content");
    }

    #[test]
    fn format_messages_mixed_roles() {
        let msgs = vec![
            make_message(MessageRole::User, "hi"),
            make_message(MessageRole::Assistant, "hello"),
            make_message(MessageRole::System, "you are helpful"),
            make_message(MessageRole::Tool, "result: 42"),
        ];

        let formatted = format_messages_for_compaction(&msgs);

        assert_eq!(
            formatted,
            "[User]: hi\n\n[Assistant]: hello\n\n[System]: you are helpful\n\n[Tool]: result: 42\n\n"
        );
    }

    #[test]
    fn format_messages_empty_content() {
        let msgs = vec![
            make_message(MessageRole::User, ""),
            make_message(MessageRole::Assistant, ""),
        ];

        let formatted = format_messages_for_compaction(&msgs);

        assert_eq!(formatted, "[User]: \n\n[Assistant]: \n\n");
    }
}
