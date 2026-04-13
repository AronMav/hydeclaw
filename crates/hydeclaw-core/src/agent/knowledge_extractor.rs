//! Post-session knowledge extraction.
//!
//! After a session completes with ≥ 5 messages, extracts user facts, outcomes,
//! and tool insights via LLM and saves them to long-term memory.

use std::sync::Arc;
use anyhow::Result;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::agent::memory_service::MemoryService;
use crate::agent::providers::LlmProvider;
use hydeclaw_types::{Message, MessageRole};

/// Minimum messages in a session to trigger extraction.
const MIN_MESSAGES: usize = 5;
/// Maximum messages to include in the extraction prompt.
const MAX_CONTEXT_MESSAGES: usize = 20;
/// LLM call timeout.
const EXTRACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Similarity threshold for dedup — skip saving if existing chunk is this similar.
const DEDUP_THRESHOLD: f64 = 0.9;

#[derive(Debug, Deserialize)]
struct ExtractedKnowledge {
    #[serde(default)]
    user_facts: Vec<String>,
    #[serde(default)]
    outcomes: Vec<String>,
    #[serde(default)]
    tool_insights: Vec<String>,
}

/// Extract knowledge from a completed session and save to memory.
/// Runs in background — errors are logged, never propagated.
pub async fn extract_and_save(
    db: PgPool,
    session_id: Uuid,
    agent_name: String,
    provider: Arc<dyn LlmProvider>,
    memory_store: Arc<dyn MemoryService>,
) {
    if !memory_store.is_available() {
        return;
    }

    if let Err(e) = extract_and_save_inner(&db, session_id, &agent_name, &provider, &memory_store).await {
        tracing::warn!(
            session_id = %session_id,
            agent = %agent_name,
            error = %e,
            "knowledge extraction failed"
        );
    }
}

async fn extract_and_save_inner(
    db: &PgPool,
    session_id: Uuid,
    agent_name: &str,
    provider: &Arc<dyn LlmProvider>,
    memory_store: &Arc<dyn MemoryService>,
) -> Result<()> {
    // 1. Load messages
    let rows = crate::db::sessions::load_messages(db, session_id, None).await?;
    if rows.len() < MIN_MESSAGES {
        return Ok(());
    }

    // 2. Build context: last N user+assistant messages (skip tool results to save tokens)
    let relevant: Vec<&crate::db::sessions::MessageRow> = rows.iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .collect();
    let context_msgs: Vec<&crate::db::sessions::MessageRow> = if relevant.len() > MAX_CONTEXT_MESSAGES {
        relevant[relevant.len() - MAX_CONTEXT_MESSAGES..].to_vec()
    } else {
        relevant
    };

    if context_msgs.is_empty() {
        return Ok(());
    }

    // 3. Format conversation for LLM
    let mut conversation = String::new();
    for m in &context_msgs {
        let role_label = match m.role.as_str() {
            "user" => "User",
            "assistant" => "Assistant",
            _ => continue,
        };
        let content = m.content.trim();
        if !content.is_empty() {
            conversation.push_str(&format!("{}: {}\n\n", role_label, content));
        }
    }

    if conversation.len() < 50 {
        return Ok(()); // Too short to extract anything meaningful
    }

    // 4. Call LLM for extraction
    let prompt = format!(
        "You are a knowledge extraction assistant. Analyze the conversation below and extract information worth remembering long-term.\n\n\
         Return a JSON object with three arrays:\n\
         {{\n\
           \"user_facts\": [\"...\"],\n\
           \"outcomes\": [\"...\"],\n\
           \"tool_insights\": [\"...\"]\n\
         }}\n\n\
         Rules:\n\
         - Only extract non-trivial information. Skip greetings, small talk, obvious context.\n\
         - Each item should be a self-contained sentence that makes sense without the conversation.\n\
         - Write in the same language as the conversation.\n\
         - Return empty arrays if nothing worth saving.\n\
         - Maximum 5 items per category.\n\n\
         Conversation:\n{}", conversation
    );

    let messages = vec![
        Message {
            role: MessageRole::User,
            content: prompt,
            tool_calls: None,
            tool_call_id: None,
            thinking_blocks: vec![],
        },
    ];

    let response = tokio::time::timeout(
        EXTRACTION_TIMEOUT,
        provider.chat(&messages, &[]),
    )
    .await
    .map_err(|_| anyhow::anyhow!("extraction LLM call timed out"))??;

    // 5. Parse JSON from response
    let extracted = parse_extraction(&response.content)?;

    // 6. Dedup and save each fact
    let mut saved = 0u32;
    let source_prefix = format!("auto:session:{}", session_id);

    for fact in &extracted.user_facts {
        if save_if_new(memory_store, fact, &format!("{}:user", source_prefix), agent_name, "shared").await {
            saved += 1;
        }
    }
    for outcome in &extracted.outcomes {
        if save_if_new(memory_store, outcome, &format!("{}:outcome", source_prefix), agent_name, "shared").await {
            saved += 1;
        }
    }
    for insight in &extracted.tool_insights {
        if save_if_new(memory_store, insight, &format!("{}:tool", source_prefix), agent_name, "private").await {
            saved += 1;
        }
    }

    if saved > 0 {
        tracing::info!(
            session_id = %session_id,
            agent = %agent_name,
            saved,
            user_facts = extracted.user_facts.len(),
            outcomes = extracted.outcomes.len(),
            tool_insights = extracted.tool_insights.len(),
            "knowledge extracted from session"
        );
    }

    Ok(())
}

/// Parse the LLM response into ExtractedKnowledge.
/// Handles markdown fences, <think> blocks, and partial JSON.
fn parse_extraction(content: &str) -> Result<ExtractedKnowledge> {
    // Strip <think>...</think> blocks
    let mut cleaned = content.to_string();
    while let Some(start) = cleaned.find("<think>") {
        if let Some(end) = cleaned.find("</think>") {
            cleaned = format!("{}{}", &cleaned[..start], &cleaned[end + 8..]);
        } else {
            cleaned = cleaned[..start].to_string();
            break;
        }
    }

    // Strip markdown fences
    let cleaned = cleaned
        .replace("```json", "")
        .replace("```", "")
        .trim()
        .to_string();

    // Find JSON object in the text
    if let Some(start) = cleaned.find('{') {
        if let Some(end) = cleaned.rfind('}') {
            let json_str = &cleaned[start..=end];
            return Ok(serde_json::from_str(json_str)?);
        }
    }

    anyhow::bail!("no JSON object found in extraction response")
}

/// Save a fact to memory if it's not already known (similarity < threshold).
async fn save_if_new(
    memory_store: &Arc<dyn MemoryService>,
    text: &str,
    source: &str,
    agent_name: &str,
    scope: &str,
) -> bool {
    let text = text.trim();
    if text.is_empty() || text.len() < 10 {
        return false;
    }

    // Check for duplicates via search
    match memory_store.search(text, 1, &[], None, None, agent_name).await {
        Ok((results, _)) => {
            if let Some(top) = results.first() {
                if top.similarity >= DEDUP_THRESHOLD {
                    return false; // Already known
                }
            }
        }
        Err(e) => {
            tracing::debug!(error = %e, "dedup search failed, saving anyway");
        }
    }

    // Save as new memory chunk
    match memory_store.index(text, source, false, None, None, scope, agent_name).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, "failed to save extracted knowledge");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_extraction tests ──────────────────────────────────────

    #[test]
    fn parse_clean_json() {
        let input = r#"{"user_facts":["User works in IT"],"outcomes":["Decided to use GraphQL"],"tool_insights":["API responded in 2s"]}"#;
        let result = parse_extraction(input).unwrap();
        assert_eq!(result.user_facts, vec!["User works in IT"]);
        assert_eq!(result.outcomes, vec!["Decided to use GraphQL"]);
        assert_eq!(result.tool_insights, vec!["API responded in 2s"]);
    }

    #[test]
    fn parse_with_markdown_fences() {
        let input = "Here is the result:\n```json\n{\"user_facts\":[\"Fact one\"],\"outcomes\":[],\"tool_insights\":[]}\n```";
        let result = parse_extraction(input).unwrap();
        assert_eq!(result.user_facts, vec!["Fact one"]);
    }

    #[test]
    fn parse_with_think_blocks() {
        let input = "<think>Let me analyze this...</think>\n{\"user_facts\":[\"Important fact\"],\"outcomes\":[],\"tool_insights\":[]}";
        let result = parse_extraction(input).unwrap();
        assert_eq!(result.user_facts, vec!["Important fact"]);
    }

    #[test]
    fn parse_with_surrounding_text() {
        let input = "Based on my analysis, here are the extracted facts:\n\n{\"user_facts\":[\"A\"],\"outcomes\":[\"B\"],\"tool_insights\":[\"C\"]}\n\nI hope this helps!";
        let result = parse_extraction(input).unwrap();
        assert_eq!(result.user_facts, vec!["A"]);
        assert_eq!(result.outcomes, vec!["B"]);
        assert_eq!(result.tool_insights, vec!["C"]);
    }

    #[test]
    fn parse_empty_arrays() {
        let input = r#"{"user_facts":[],"outcomes":[],"tool_insights":[]}"#;
        let result = parse_extraction(input).unwrap();
        assert!(result.user_facts.is_empty());
        assert!(result.outcomes.is_empty());
        assert!(result.tool_insights.is_empty());
    }

    #[test]
    fn parse_missing_fields_default_empty() {
        let input = r#"{"user_facts":["Only this"]}"#;
        let result = parse_extraction(input).unwrap();
        assert_eq!(result.user_facts, vec!["Only this"]);
        assert!(result.outcomes.is_empty());
        assert!(result.tool_insights.is_empty());
    }

    #[test]
    fn parse_no_json_fails() {
        let input = "I could not extract anything from this conversation.";
        assert!(parse_extraction(input).is_err());
    }

    #[test]
    fn parse_nested_think_blocks() {
        let input = "<think>first</think>Some text<think>second</think>{\"user_facts\":[\"X\"],\"outcomes\":[],\"tool_insights\":[]}";
        let result = parse_extraction(input).unwrap();
        assert_eq!(result.user_facts, vec!["X"]);
    }

    #[test]
    fn parse_unclosed_think_block() {
        let input = "<think>thinking forever... {\"user_facts\":[\"should not parse\"]}";
        // Unclosed think — everything after <think> is stripped
        assert!(parse_extraction(input).is_err());
    }

    #[test]
    fn parse_multiple_items_per_category() {
        let input = r#"{"user_facts":["F1","F2","F3"],"outcomes":["O1","O2"],"tool_insights":["T1"]}"#;
        let result = parse_extraction(input).unwrap();
        assert_eq!(result.user_facts.len(), 3);
        assert_eq!(result.outcomes.len(), 2);
        assert_eq!(result.tool_insights.len(), 1);
    }

    // ── save_if_new tests ───────────────────────────────────────────

    #[tokio::test]
    async fn save_if_new_skips_short_text() {
        let mock = Arc::new(crate::agent::memory_service::mock::MockMemoryService::available()) as Arc<dyn MemoryService>;
        assert!(!save_if_new(&mock, "", "src", "agent", "private").await);
        assert!(!save_if_new(&mock, "short", "src", "agent", "private").await);
        assert!(!save_if_new(&mock, "  ", "src", "agent", "private").await);
    }

    #[tokio::test]
    async fn save_if_new_saves_valid_text() {
        let mock = Arc::new(crate::agent::memory_service::mock::MockMemoryService::available()) as Arc<dyn MemoryService>;
        // Mock search returns empty results → no duplicate → should save
        let result = save_if_new(&mock, "This is a long enough fact to save", "auto:test", "agent", "shared").await;
        assert!(result);
    }
}
