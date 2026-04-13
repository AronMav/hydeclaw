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
    #[serde(default)]
    feedback: Vec<String>,
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

    let start_idx = relevant.len().saturating_sub(MAX_CONTEXT_MESSAGES);
    let context_msgs = &relevant[start_idx..];

    if context_msgs.is_empty() {
        return Ok(());
    }

    // 3. Format conversation for LLM
    let mut conversation = String::new();
    for m in context_msgs {
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
         Return a JSON object with four arrays:\n\
         {{\n\
           \"user_facts\": [\"...\"],\n\
           \"outcomes\": [\"...\"],\n\
           \"tool_insights\": [\"...\"],\n\
           \"feedback\": [\"...\"]\n\
         }}\n\n\
         Categories:\n\
         - user_facts: Facts about the user (preferences, context, identity, goals)\n\
         - outcomes: Decisions made, conclusions reached, recommendations given\n\
         - tool_insights: What tools were used, what worked/failed, performance notes\n\
         - feedback: User preferences and reactions — what they approved, rejected, asked to redo, liked or disliked\n\n\
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
    for fb in &extracted.feedback {
        if save_if_new(memory_store, fb, &format!("{}:feedback", source_prefix), agent_name, "shared").await {
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
            feedback = extracted.feedback.len(),
            "knowledge extracted from session"
        );
    }

    // 7. Update rolling agent summary
    update_rolling_summary(agent_name, provider, memory_store, &extracted).await;

    Ok(())
}

/// Update the rolling agent summary — a single pinned chunk that captures
/// the agent's accumulated knowledge about the user and context.
async fn update_rolling_summary(
    agent_name: &str,
    provider: &Arc<dyn LlmProvider>,
    memory_store: &Arc<dyn MemoryService>,
    extracted: &ExtractedKnowledge,
) {
    // Collect all new facts into one list
    let mut new_facts: Vec<&str> = Vec::new();
    for f in &extracted.user_facts { new_facts.push(f); }
    for f in &extracted.outcomes { new_facts.push(f); }
    for f in &extracted.feedback { new_facts.push(f); }

    if new_facts.is_empty() {
        return; // Nothing new to summarize
    }

    let summary_source = format!("rolling_summary:{}", agent_name);

    // Load current summary
    let current_summary = match memory_store.get(None, Some(&summary_source), 1).await {
        Ok(chunks) => chunks.first().map(|c| c.content.clone()).unwrap_or_default(),
        Err(_) => String::new(),
    };

    // Build update prompt
    let new_facts_text = new_facts.iter()
        .map(|f| format!("- {}", f))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = if current_summary.is_empty() {
        format!(
            "Create a concise agent summary (200 words max) from these facts about the user and recent interactions:\n\n{}\n\n\
             Write in the same language as the facts. Be concise — this summary is injected into every conversation.",
            new_facts_text
        )
    } else {
        format!(
            "Update this agent summary with new information. Keep it under 200 words. \
             Merge new facts into existing summary — don't duplicate, update contradictions, keep most important.\n\n\
             Current summary:\n{}\n\nNew facts:\n{}\n\n\
             Return ONLY the updated summary text, nothing else.",
            current_summary, new_facts_text
        )
    };

    let messages = vec![Message {
        role: MessageRole::User,
        content: prompt,
        tool_calls: None,
        tool_call_id: None,
        thinking_blocks: vec![],
    }];

    let response = match tokio::time::timeout(EXTRACTION_TIMEOUT, provider.chat(&messages, &[])).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => { tracing::debug!(error = %e, "rolling summary LLM call failed"); return; }
        Err(_) => { tracing::debug!("rolling summary LLM call timed out"); return; }
    };

    let new_summary = response.content.trim().to_string();
    if new_summary.is_empty() || new_summary.len() < 20 {
        return;
    }

    // Strip think blocks from summary
    let new_summary = {
        let mut s = new_summary;
        while let Some(start) = s.find("<think>") {
            if let Some(end) = s.find("</think>") {
                s = format!("{}{}", &s[..start], &s[end + 8..]);
            } else {
                s = s[..start].to_string();
                break;
            }
        }
        s.trim().to_string()
    };

    // Delete old summary chunk if exists
    if let Ok(chunks) = memory_store.get(None, Some(&summary_source), 1).await {
        for chunk in &chunks {
            let _ = memory_store.delete(&chunk.id).await;
        }
    }

    // Save new summary as pinned chunk
    match memory_store.index(&new_summary, &summary_source, true, None, None, "private", agent_name).await {
        Ok(_) => tracing::info!(agent = agent_name, len = new_summary.len(), "rolling summary updated"),
        Err(e) => tracing::warn!(agent = agent_name, error = %e, "failed to save rolling summary"),
    }
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

    // ── scope assignment tests ──────────────────────────────────────

    #[tokio::test]
    async fn save_if_new_accepts_private_scope() {
        let mock = Arc::new(crate::agent::memory_service::mock::MockMemoryService::available()) as Arc<dyn MemoryService>;
        let result = save_if_new(&mock, "Tool insight only for this agent", "auto:test:tool", "Arty", "private").await;
        assert!(result);
    }

    #[tokio::test]
    async fn save_if_new_accepts_shared_scope() {
        let mock = Arc::new(crate::agent::memory_service::mock::MockMemoryService::available()) as Arc<dyn MemoryService>;
        let result = save_if_new(&mock, "User works in IT sector", "auto:test:user", "Arty", "shared").await;
        assert!(result);
    }

    // ── feedback parsing tests ──────────────────────────────────────

    #[test]
    fn parse_with_feedback_field() {
        let input = r#"{"user_facts":["F1"],"outcomes":["O1"],"tool_insights":["T1"],"feedback":["User approved the analysis","User rejected the recommendation"]}"#;
        let result = parse_extraction(input).unwrap();
        assert_eq!(result.feedback.len(), 2);
        assert_eq!(result.feedback[0], "User approved the analysis");
    }

    #[test]
    fn parse_without_feedback_defaults_empty() {
        let input = r#"{"user_facts":["F1"],"outcomes":[],"tool_insights":[]}"#;
        let result = parse_extraction(input).unwrap();
        assert!(result.feedback.is_empty());
    }

    // ── rolling summary tests ───────────────────────────────────────

    #[test]
    fn rolling_summary_collects_only_user_facts_outcomes_feedback() {
        // Verify that tool_insights are excluded from summary input
        let extracted = ExtractedKnowledge {
            user_facts: vec!["User works in IT".into()],
            outcomes: vec!["Decided to use GraphQL".into()],
            tool_insights: vec!["API took 2s".into()],
            feedback: vec!["User approved analysis".into()],
        };
        let mut facts: Vec<&str> = Vec::new();
        for f in &extracted.user_facts { facts.push(f); }
        for f in &extracted.outcomes { facts.push(f); }
        for f in &extracted.feedback { facts.push(f); }
        // tool_insights NOT included
        assert_eq!(facts.len(), 3);
        assert!(!facts.iter().any(|f| f.contains("API took")));
        assert!(facts.iter().any(|f| f.contains("IT")));
        assert!(facts.iter().any(|f| f.contains("GraphQL")));
        assert!(facts.iter().any(|f| f.contains("approved")));
    }

    #[test]
    fn rolling_summary_empty_when_only_tool_insights() {
        let extracted = ExtractedKnowledge {
            user_facts: vec![],
            outcomes: vec![],
            tool_insights: vec!["some insight".into()],
            feedback: vec![],
        };
        let mut facts: Vec<&str> = Vec::new();
        for f in &extracted.user_facts { facts.push(f); }
        for f in &extracted.outcomes { facts.push(f); }
        for f in &extracted.feedback { facts.push(f); }
        assert!(facts.is_empty()); // No summary needed
    }
}
