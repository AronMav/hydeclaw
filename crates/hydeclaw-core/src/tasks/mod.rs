use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool, Row};
use uuid::Uuid;

#[derive(Debug, FromRow, serde::Serialize)]
pub struct TaskRow {
    pub id: Uuid,
    pub agent_id: String,
    pub user_id: String,
    pub source: String,
    pub status: String,
    pub input: String,
    pub plan: Option<serde_json::Value>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// List tasks for an agent, most recent first.
pub async fn list_tasks(db: &PgPool, agent_id: &str, limit: i64) -> Result<Vec<TaskRow>> {
    let rows = sqlx::query_as::<_, TaskRow>(
        "SELECT id, agent_id, user_id, source, status, input, plan, result, error, created_at, updated_at \
         FROM tasks WHERE agent_id = $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(agent_id)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Get a single task by ID.
pub async fn get_task(db: &PgPool, task_id: Uuid) -> Result<Option<TaskRow>> {
    let row = sqlx::query_as::<_, TaskRow>(
        "SELECT id, agent_id, user_id, source, status, input, plan, result, error, created_at, updated_at \
         FROM tasks WHERE id = $1",
    )
    .bind(task_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// Delete a task and its steps (cascade).
pub async fn delete_task(db: &PgPool, task_id: Uuid) -> Result<bool> {
    let result = sqlx::query("DELETE FROM tasks WHERE id = $1")
        .bind(task_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Create a new task.
pub async fn create_task(
    db: &PgPool,
    agent_id: &str,
    user_id: &str,
    source: &str,
    input: &str,
) -> Result<Uuid> {
    let row = sqlx::query(
        "INSERT INTO tasks (agent_id, user_id, source, input) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(source)
    .bind(input)
    .fetch_one(db)
    .await?;

    Ok(row.get("id"))
}

/// Update task status.
#[allow(dead_code)] // Used by execute_task pipeline
pub async fn update_task_status(
    db: &PgPool,
    task_id: Uuid,
    status: &str,
    result: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE tasks SET status = $1, result = $2, error = $3, updated_at = now() WHERE id = $4",
    )
    .bind(status)
    .bind(result)
    .bind(error)
    .bind(task_id)
    .execute(db)
    .await?;

    Ok(())
}

/// Create a task step.
#[allow(dead_code)] // Used by execute_task pipeline
pub async fn create_step(
    db: &PgPool,
    task_id: Uuid,
    step_order: i32,
    mcp_name: &str,
    action: &str,
    params: Option<&serde_json::Value>,
) -> Result<Uuid> {
    let row = sqlx::query(
        "INSERT INTO task_steps (task_id, step_order, mcp_name, action, params) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(task_id)
    .bind(step_order)
    .bind(mcp_name)
    .bind(action)
    .bind(params)
    .fetch_one(db)
    .await?;

    Ok(row.get("id"))
}

/// Update step status.
pub async fn update_step_status(
    db: &PgPool,
    step_id: Uuid,
    status: &str,
    output: Option<&serde_json::Value>,
) -> Result<()> {
    let now = chrono::Utc::now();
    match status {
        "running" => {
            sqlx::query(
                "UPDATE task_steps SET status = $1, started_at = $2 WHERE id = $3",
            )
            .bind(status)
            .bind(now)
            .bind(step_id)
            .execute(db)
            .await?;
        }
        "completed" | "failed" => {
            sqlx::query(
                "UPDATE task_steps SET status = $1, output = $2, completed_at = $3 WHERE id = $4",
            )
            .bind(status)
            .bind(output)
            .bind(now)
            .bind(step_id)
            .execute(db)
            .await?;
        }
        _ => {
            sqlx::query("UPDATE task_steps SET status = $1 WHERE id = $2")
                .bind(status)
                .bind(step_id)
                .execute(db)
                .await?;
        }
    }

    Ok(())
}

/// Process a MCP callback — update step status and optionally complete the task.
pub async fn update_step_from_callback(
    db: &PgPool,
    callback: &hydeclaw_types::McpCallback,
) -> Result<()> {
    let step_id = callback.step_id.unwrap_or(callback.task_id);

    match callback.status.as_str() {
        "completed" => {
            update_step_status(db, step_id, "completed", callback.result.as_ref()).await?;
        }
        "failed" => {
            let error_output = callback
                .error
                .as_ref()
                .map(|e| serde_json::json!({"error": e}));
            update_step_status(db, step_id, "failed", error_output.as_ref()).await?;
        }
        "progress" => {
            tracing::debug!(step_id = %step_id, "MCP progress update");
        }
        other => {
            tracing::warn!(status = %other, "unknown callback status");
        }
    }

    Ok(())
}

/// Load pending steps for a task, ordered by `step_order`.
pub async fn load_task_steps(
    db: &PgPool,
    task_id: Uuid,
) -> Result<Vec<TaskStepRow>> {
    let rows = sqlx::query_as::<_, TaskStepRow>(
        "SELECT id, task_id, step_order, mcp_name, action, params, status, output \
         FROM task_steps WHERE task_id = $1 ORDER BY step_order",
    )
    .bind(task_id)
    .fetch_all(db)
    .await?;

    Ok(rows)
}

/// Set task plan (JSON steps determined by agent).
#[allow(dead_code)] // Used by execute_task pipeline
pub async fn set_task_plan(db: &PgPool, task_id: Uuid, plan: &serde_json::Value) -> Result<()> {
    sqlx::query("UPDATE tasks SET plan = $1, status = 'planning', updated_at = now() WHERE id = $2")
        .bind(plan)
        .bind(task_id)
        .execute(db)
        .await?;
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)] // Fields read via FromRow derive
pub struct TaskStepRow {
    pub id: Uuid,
    pub task_id: Uuid,
    pub step_order: i32,
    pub mcp_name: String,
    pub action: String,
    pub params: Option<serde_json::Value>,
    pub status: String,
    pub output: Option<serde_json::Value>,
}

/// Execute a multi-step task: run steps sequentially, calling skills via MCP.
#[allow(dead_code)] // Will be called from agent engine when task execution is triggered
pub async fn execute_task(
    db: &PgPool,
    task_id: Uuid,
    skills: &crate::mcp::McpRegistry,
) -> Result<String> {
    update_task_status(db, task_id, "running", None, None).await?;

    let steps = load_task_steps(db, task_id).await?;
    let mut last_output = String::new();

    for step in &steps {
        if step.status == "completed" {
            // Already done (e.g., from a retry)
            if let Some(ref out) = step.output {
                last_output = serde_json::to_string(out).unwrap_or_default();
            }
            continue;
        }

        update_step_status(db, step.id, "running", None).await?;

        let args = step.params.clone().unwrap_or(serde_json::json!({}));
        match skills.call_tool(&step.mcp_name, &step.action, &args).await {
            Ok(result) => {
                let output = serde_json::json!({"result": result});
                update_step_status(db, step.id, "completed", Some(&output)).await?;
                last_output = result;
            }
            Err(e) => {
                let error = serde_json::json!({"error": e.to_string()});
                update_step_status(db, step.id, "failed", Some(&error)).await?;
                update_task_status(db, task_id, "failed", None, Some(&e.to_string())).await?;
                return Err(e);
            }
        }
    }

    update_task_status(db, task_id, "completed", Some(&last_output), None).await?;
    Ok(last_output)
}
