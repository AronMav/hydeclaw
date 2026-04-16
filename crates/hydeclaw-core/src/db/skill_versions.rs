use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Compute SHA256 hex digest of content.
fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Save a new skill version to the DAG.
///
/// - Computes `content_hash` via SHA256.
/// - Determines `generation` as MAX(generation) + 1 for the given `skill_name` (0 if first).
/// - Inserts the row and returns the new UUID.
pub async fn save_version(
    db: &PgPool,
    skill_name: &str,
    content: &str,
    evolution_type: &str,
    parent_id: Option<Uuid>,
    trigger_reason: Option<&str>,
) -> sqlx::Result<Uuid> {
    let content_hash = sha256_hex(content);

    let row = sqlx::query(
        "INSERT INTO skill_versions \
         (skill_name, generation, parent_id, evolution_type, content, content_hash, trigger_reason) \
         VALUES ($1, \
             (SELECT COALESCE(MAX(generation), -1) + 1 FROM skill_versions WHERE skill_name = $1), \
             $2, $3, $4, $5, $6) \
         RETURNING id",
    )
    .bind(skill_name)
    .bind(parent_id)
    .bind(evolution_type)
    .bind(content)
    .bind(&content_hash)
    .bind(trigger_reason)
    .fetch_one(db)
    .await?;

    Ok(row.get("id"))
}
