//! Knowledge graph operations on pure PostgreSQL (relational tables).
//!
//! Tables: graph_entities, graph_edges, graph_episodes.
//! Replaces the previous Apache AGE Cypher-based implementation.

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

/// Entity extracted from memory content.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphEntity {
    pub name: String,
    pub entity_type: String,
}

/// Relation between two entities.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphRelation {
    pub source: String,
    pub target: String,
    pub relation_type: String,
}

// ── Entity operations ────────────────────────────────────────────────────────

/// Upsert with fuzzy resolution: exact match → trigram fuzzy (>0.5) → insert new.
pub async fn upsert_entity_resolved(db: &PgPool, name: &str, entity_type: &str) -> Result<Uuid> {
    let normalized = name.trim().to_lowercase();

    // 1. Exact match
    let exact: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM graph_entities WHERE name_normalized = $1 AND entity_type = $2",
    )
    .bind(&normalized)
    .bind(entity_type)
    .fetch_optional(db)
    .await?;
    if let Some((id,)) = exact {
        sqlx::query("UPDATE graph_entities SET updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(db)
            .await?;
        return Ok(id);
    }

    // 2. Fuzzy match (trigram similarity > 0.5, same entity_type)
    let fuzzy: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM graph_entities
         WHERE entity_type = $2 AND similarity(name_normalized, $1) > 0.5
         ORDER BY similarity(name_normalized, $1) DESC LIMIT 1",
    )
    .bind(&normalized)
    .bind(entity_type)
    .fetch_optional(db)
    .await?;
    if let Some((id,)) = fuzzy {
        sqlx::query("UPDATE graph_entities SET updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(db)
            .await?;
        return Ok(id);
    }

    // 3. New entity
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO graph_entities (name, name_normalized, entity_type)
         VALUES ($1, $2, $3)
         ON CONFLICT (name_normalized, entity_type) DO UPDATE SET updated_at = now()
         RETURNING id",
    )
    .bind(name.trim())
    .bind(&normalized)
    .bind(entity_type)
    .fetch_one(db)
    .await?;
    Ok(id)
}

// ── Relation operations ──────────────────────────────────────────────────────

/// Create or strengthen a relation between two entities (by normalized name + type).
/// Logs warning if source or target entity not found.
pub async fn upsert_relation(
    db: &PgPool,
    source_name: &str,
    source_type: &str,
    target_name: &str,
    target_type: &str,
    relation_type: &str,
    fact: Option<&str>,
) -> Result<()> {
    let src_norm = source_name.trim().to_lowercase();
    let tgt_norm = target_name.trim().to_lowercase();
    let result = sqlx::query(
        "INSERT INTO graph_edges (source_id, target_id, relation_type, fact)
         SELECT s.id, t.id, $5, $6
         FROM graph_entities s, graph_entities t
         WHERE s.name_normalized = $1 AND s.entity_type = $2
           AND t.name_normalized = $3 AND t.entity_type = $4
         ON CONFLICT (source_id, target_id, relation_type)
         DO UPDATE SET weight = graph_edges.weight + 1, updated_at = now(),
                       fact = COALESCE(EXCLUDED.fact, graph_edges.fact)",
    )
    .bind(&src_norm)
    .bind(source_type)
    .bind(&tgt_norm)
    .bind(target_type)
    .bind(relation_type)
    .bind(fact)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        tracing::warn!(
            source = source_name,
            target = target_name,
            rel = relation_type,
            "upsert_relation: source or target entity not found, relation skipped"
        );
    }
    Ok(())
}

// ── Graph traversal ──────────────────────────────────────────────────────────

/// Find entities connected to a given entity (multi-hop via recursive CTE).
pub async fn find_related(
    db: &PgPool,
    entity_name: &str,
    max_hops: u8,
) -> Result<Vec<GraphEntity>> {
    let normalized = entity_name.trim().to_lowercase();
    // SAFETY: `max_hops` is u8 — numeric type, cannot inject SQL.
    let query = format!(
        "WITH RECURSIVE hops AS (
            SELECT id, name, entity_type, 0 AS depth
            FROM graph_entities WHERE name_normalized = $1
            UNION
            SELECT DISTINCT e2.id, e2.name, e2.entity_type, h.depth + 1
            FROM hops h
            JOIN graph_edges ge ON (ge.source_id = h.id OR ge.target_id = h.id) AND ge.invalid_at IS NULL
            JOIN graph_entities e2 ON e2.id = CASE WHEN ge.source_id = h.id THEN ge.target_id ELSE ge.source_id END
            WHERE h.depth < {max_hops} AND e2.id != h.id
        )
        SELECT DISTINCT name, entity_type FROM hops WHERE depth > 0"
    );
    let rows: Vec<(String, String)> = sqlx::query_as(&query)
        .bind(&normalized)
        .fetch_all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(name, entity_type)| GraphEntity { name, entity_type })
        .collect())
}

// ── Episode linking ──────────────────────────────────────────────────────────

/// Link a memory chunk (and optional session) to extracted entities.
pub async fn link_chunk_entities(
    db: &PgPool,
    chunk_id: Uuid,
    session_id: Option<Uuid>,
    entities: &[(String, String, Uuid)], // (name, type, entity_uuid)
) -> Result<()> {
    for (_, _, entity_id) in entities {
        sqlx::query(
            "INSERT INTO graph_episodes (chunk_id, session_id, entity_id)
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(chunk_id)
        .bind(session_id)
        .bind(entity_id)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// Link a session (without chunk) to extracted entities.
pub async fn link_session_entities(
    db: &PgPool,
    session_id: Uuid,
    entity_ids: &[Uuid],
) -> Result<()> {
    for entity_id in entity_ids {
        sqlx::query(
            "INSERT INTO graph_episodes (session_id, entity_id)
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(session_id)
        .bind(entity_id)
        .execute(db)
        .await?;
    }
    Ok(())
}

// ── Query helpers (used by API handlers and search) ──────────────────────────

/// Get (chunk_id, entity_name, entity_type) for given chunk IDs.
pub async fn get_chunk_entity_rows(
    db: &PgPool,
    chunk_ids: &[Uuid],
) -> Result<Vec<(Uuid, String, String)>> {
    if chunk_ids.is_empty() {
        return Ok(vec![]);
    }
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT ep.chunk_id, ge.name, ge.entity_type
         FROM graph_episodes ep
         JOIN graph_entities ge ON ge.id = ep.entity_id
         WHERE ep.chunk_id = ANY($1)",
    )
    .bind(chunk_ids)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Get edges between a set of entities (by name). Only active edges (invalid_at IS NULL).
pub async fn get_entity_edges(
    db: &PgPool,
    entity_names: &[String],
) -> Result<Vec<GraphRelation>> {
    if entity_names.is_empty() {
        return Ok(vec![]);
    }
    let normalized: Vec<String> = entity_names.iter().map(|n| n.trim().to_lowercase()).collect();
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT s.name, t.name, e.relation_type
         FROM graph_edges e
         JOIN graph_entities s ON s.id = e.source_id
         JOIN graph_entities t ON t.id = e.target_id
         WHERE s.name_normalized = ANY($1) AND t.name_normalized = ANY($1)
           AND e.invalid_at IS NULL",
    )
    .bind(&normalized)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(s, t, r)| GraphRelation {
            source: s,
            target: t,
            relation_type: r,
        })
        .collect())
}

// ── Shared LLM entity extraction ─────────────────────────────────────────────

/// Extract entities/relations from a chunk via LLM and link to graph.
/// Single shared function used by: background worker, engine_memory, post-session.
pub async fn extract_entities_for_chunk(
    db: &PgPool,
    provider: &std::sync::Arc<dyn crate::agent::providers::LlmProvider>,
    content: &str,
    chunk_id: &str,
) -> Result<usize> {
    use hydeclaw_types::{Message, MessageRole};

    let chunk_uuid: Uuid = chunk_id.parse()?;
    let text = &content[..content.len().min(3000)];

    let prompt = format!(
        "Extract entities and relations from this text. Return JSON only:\n\
        {{\"entities\": [{{\"name\": \"...\", \"entity_type\": \"Person|Organization|Concept|Place|Event|Technology\"}}], \
        \"relations\": [{{\"source\": \"...\", \"target\": \"...\", \"relation_type\": \"KNOWS|WORKS_AT|LOCATED_IN|PART_OF|RELATED_TO|CREATED_BY|USES\"}}]}}\n\
        Text: {}",
        text
    );

    let response = provider
        .chat(
            &[Message {
                role: MessageRole::User,
                content: prompt,
                tool_calls: None,
                tool_call_id: None,
                thinking_blocks: vec![],
            }],
            &[],
        )
        .await?;

    let (entities, relations) = parse_extraction_response(&response.content);
    if entities.is_empty() {
        return Ok(0);
    }

    let mut entity_ids: Vec<(String, String, Uuid)> = Vec::new();
    for entity in &entities {
        match upsert_entity_resolved(db, &entity.name, &entity.entity_type).await {
            Ok(id) => entity_ids.push((entity.name.clone(), entity.entity_type.clone(), id)),
            Err(e) => tracing::warn!(error = %e, entity = %entity.name, "entity upsert failed"),
        }
    }

    for rel in &relations {
        let src_type = entities
            .iter()
            .find(|e| e.name == rel.source)
            .map(|e| e.entity_type.as_str())
            .unwrap_or("Concept");
        let tgt_type = entities
            .iter()
            .find(|e| e.name == rel.target)
            .map(|e| e.entity_type.as_str())
            .unwrap_or("Concept");
        let fact = format!("{} {} {}", rel.source, rel.relation_type, rel.target);
        if let Err(e) = upsert_relation(
            db, &rel.source, src_type, &rel.target, tgt_type, &rel.relation_type, Some(&fact),
        )
        .await
        {
            tracing::warn!(error = %e, "graph: failed to upsert relation");
        }
    }

    link_chunk_entities(db, chunk_uuid, None, &entity_ids).await?;
    Ok(entity_ids.len())
}

// ── Per-agent analytics ──────────────────────────────────────────────────────

/// Get entity count per agent (via episodes → sessions.agent_id).
pub async fn get_entity_counts_by_agent(db: &PgPool) -> Result<Vec<(String, i64)>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT s.agent_id, COUNT(DISTINCT ep.entity_id)
         FROM graph_episodes ep
         JOIN sessions s ON s.id = ep.session_id
         WHERE ep.session_id IS NOT NULL AND s.agent_id IS NOT NULL
         GROUP BY s.agent_id
         ORDER BY count DESC",
    )
    .fetch_all(db)
    .await?;
    Ok(rows)
}

// ── LLM response parsing (pure function, no DB) ─────────────────────────────

/// Extract entities and relations from LLM JSON response.
pub fn parse_extraction_response(json_text: &str) -> (Vec<GraphEntity>, Vec<GraphRelation>) {
    #[derive(serde::Deserialize)]
    struct Extraction {
        #[serde(default)]
        entities: Vec<GraphEntity>,
        #[serde(default)]
        relations: Vec<GraphRelation>,
    }
    // Strip <think>...</think> blocks
    let mut text = json_text.to_string();
    while let Some(start) = text.find("<think>") {
        if let Some(end) = text.find("</think>") {
            text = format!("{}{}", &text[..start], &text[end + 8..]);
        } else {
            text = text[..start].to_string();
            break;
        }
    }
    // Strip markdown fences
    let clean = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    // Try direct parse first
    if let Ok(e) = serde_json::from_str::<Extraction>(clean) {
        return (e.entities, e.relations);
    }
    // Fallback: find first { and last } in the text
    if let (Some(start), Some(end)) = (clean.find('{'), clean.rfind('}'))
        && let Ok(e) = serde_json::from_str::<Extraction>(&clean[start..=end]) {
            return (e.entities, e.relations);
        }
    (vec![], vec![])
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json() {
        let json = r#"{"entities": [{"name": "Alice", "entity_type": "Person"}], "relations": []}"#;
        let (ents, rels) = parse_extraction_response(json);
        assert_eq!(ents.len(), 1);
        assert_eq!(ents[0].name, "Alice");
        assert_eq!(ents[0].entity_type, "Person");
        assert!(rels.is_empty());
    }

    #[test]
    fn parse_with_relations() {
        let json = r#"{
            "entities": [
                {"name": "Alice", "entity_type": "Person"},
                {"name": "Acme", "entity_type": "Organization"}
            ],
            "relations": [
                {"source": "Alice", "target": "Acme", "relation_type": "WORKS_AT"}
            ]
        }"#;
        let (ents, rels) = parse_extraction_response(json);
        assert_eq!(ents.len(), 2);
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].source, "Alice");
        assert_eq!(rels[0].relation_type, "WORKS_AT");
    }

    #[test]
    fn parse_with_think_tags() {
        let json = r#"<think>I need to extract entities from this text.</think>{"entities": [{"name": "Bob", "entity_type": "Person"}], "relations": []}"#;
        let (ents, _) = parse_extraction_response(json);
        assert_eq!(ents.len(), 1);
        assert_eq!(ents[0].name, "Bob");
    }

    #[test]
    fn parse_with_unclosed_think() {
        let json = r#"<think>still thinking...{"entities": []}"#;
        let (ents, rels) = parse_extraction_response(json);
        assert!(ents.is_empty());
        assert!(rels.is_empty());
    }

    #[test]
    fn parse_with_markdown_fences() {
        let json = "```json\n{\"entities\": [{\"name\": \"X\", \"entity_type\": \"Concept\"}], \"relations\": []}\n```";
        let (ents, _) = parse_extraction_response(json);
        assert_eq!(ents.len(), 1);
    }

    #[test]
    fn parse_with_surrounding_text() {
        let json = "Here are the entities: {\"entities\": [{\"name\": \"Y\", \"entity_type\": \"Place\"}], \"relations\": []} hope that helps!";
        let (ents, _) = parse_extraction_response(json);
        assert_eq!(ents.len(), 1);
        assert_eq!(ents[0].name, "Y");
    }

    #[test]
    fn parse_garbage_returns_empty() {
        let (ents, rels) = parse_extraction_response("not json at all");
        assert!(ents.is_empty());
        assert!(rels.is_empty());
    }

    #[test]
    fn parse_empty_string() {
        let (ents, rels) = parse_extraction_response("");
        assert!(ents.is_empty());
        assert!(rels.is_empty());
    }

    #[test]
    fn parse_missing_fields_default_empty() {
        let json = r#"{"entities": [{"name": "Z", "entity_type": "Technology"}]}"#;
        let (ents, rels) = parse_extraction_response(json);
        assert_eq!(ents.len(), 1);
        assert!(rels.is_empty());
    }

    #[test]
    fn parse_multiple_think_blocks() {
        let json = r#"<think>first</think>some text<think>second</think>{"entities": [{"name": "A", "entity_type": "Concept"}], "relations": []}"#;
        let (ents, _) = parse_extraction_response(json);
        assert_eq!(ents.len(), 1);
    }
}
