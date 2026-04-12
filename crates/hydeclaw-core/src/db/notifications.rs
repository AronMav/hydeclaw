use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

/// One row from the notifications table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Notification {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub notification_type: String,
    pub title: String,
    pub body: String,
    pub data: serde_json::Value,
    pub read: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Insert a new notification. Returns the inserted row (with generated id and `created_at`).
pub async fn create_notification(
    db: &PgPool,
    notification_type: &str,
    title: &str,
    body: &str,
    data: serde_json::Value,
) -> Result<Notification> {
    let row = sqlx::query_as::<_, Notification>(
        r"
        INSERT INTO notifications (type, title, body, data)
        VALUES ($1, $2, $3, $4)
        RETURNING id, type AS notification_type, title, body, data, read, created_at
        ",
    )
    .bind(notification_type)
    .bind(title)
    .bind(body)
    .bind(data)
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// List notifications newest-first with pagination.
/// Returns (rows, `total_unread_count`).
pub async fn list_notifications(
    db: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Notification>, i64)> {
    let rows = sqlx::query_as::<_, Notification>(
        r"
        SELECT id, type AS notification_type, title, body, data, read, created_at
        FROM notifications
        ORDER BY created_at DESC
        LIMIT $1 OFFSET $2
        ",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await?;

    let unread: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE read = FALSE")
        .fetch_one(db)
        .await?;

    Ok((rows, unread))
}

/// Mark a single notification as read by id. Returns true if a row was updated.
pub async fn mark_read(db: &PgPool, id: Uuid) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE notifications SET read = TRUE WHERE id = $1 AND read = FALSE",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Mark ALL notifications as read. Returns the count of updated rows.
pub async fn mark_all_read(db: &PgPool) -> Result<u64> {
    let result = sqlx::query("UPDATE notifications SET read = TRUE WHERE read = FALSE")
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

/// Count unread notifications.
#[allow(dead_code)]
pub async fn unread_count(db: &PgPool) -> Result<i64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE read = FALSE")
        .fetch_one(db)
        .await?;
    Ok(count)
}
