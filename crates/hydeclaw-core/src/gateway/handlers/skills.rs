use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use super::super::AppState;

/// Sanitize a skill name to a safe filename stem (same logic as write_skill).
pub(crate) fn skill_safe_name(name: &str) -> String {
    name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|', ' '], "-")
}

/// Resolve the actual .md file path for a skill name.
/// Tries sanitized-name first, then scans all .md files for matching frontmatter name.
/// Returns None if no matching file found.
pub(crate) async fn find_skill_path(
    workspace_dir: &str,
    skill_name: &str,
) -> Option<std::path::PathBuf> {
    let skills_dir = std::path::PathBuf::from(workspace_dir).join("skills");

    // 1. Try sanitized name (skills created/saved via UI)
    let safe = skill_safe_name(skill_name);
    let candidate = skills_dir.join(format!("{}.md", safe));
    if candidate.exists() {
        return Some(candidate);
    }

    // 2. Fallback: scan all .md files for matching frontmatter name
    let mut rd = tokio::fs::read_dir(&skills_dir).await.ok()?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(content) = tokio::fs::read_to_string(&path).await
            && let Some(skill) = crate::skills::SkillDef::parse(&content)
                && skill.meta.name == skill_name {
                    return Some(path);
                }
    }
    None
}

// ── Global skills endpoints ───────────────────────────────────────────────────

/// GET /api/skills
pub(crate) async fn api_skills_list_global(
    State(_state): State<AppState>,
) -> impl IntoResponse {
    let skills = crate::skills::load_skills(crate::config::WORKSPACE_DIR).await;
    let result: Vec<serde_json::Value> = skills.iter().map(|s| {
        serde_json::json!({
            "name": s.meta.name,
            "description": s.meta.description,
            "triggers": s.meta.triggers,
            "tools_required": s.meta.tools_required,
            "priority": s.meta.priority,
            "instructions_len": s.instructions.len(),
        })
    }).collect();
    Json(serde_json::json!({"skills": result})).into_response()
}

/// GET /api/skills/{skill}
pub(crate) async fn api_skill_get_global(
    State(_state): State<AppState>,
    axum::extract::Path(skill_name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(path) = find_skill_path(crate::config::WORKSPACE_DIR, &skill_name).await else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "skill not found"}))).into_response();
    };

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            let mut result = serde_json::json!({ "name": skill_name, "content": content });
            if let Some(skill) = crate::skills::SkillDef::parse(&content) {
                result["description"] = serde_json::json!(skill.meta.description);
                result["triggers"] = serde_json::json!(skill.meta.triggers);
                result["tools_required"] = serde_json::json!(skill.meta.tools_required);
                result["priority"] = serde_json::json!(skill.meta.priority);
                result["instructions"] = serde_json::json!(skill.instructions);
            }
            Json(result).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "skill not found"}))).into_response(),
    }
}

/// PUT /api/skills/{skill}
pub(crate) async fn api_skill_upsert_global(
    State(_state): State<AppState>,
    axum::extract::Path(skill_name): axum::extract::Path<String>,
    axum::extract::Json(body): axum::extract::Json<SkillUpsertBody>,
) -> impl IntoResponse {
    let frontmatter = crate::skills::SkillFrontmatter {
        name: skill_name.clone(),
        description: body.description.unwrap_or_default(),
        triggers: body.triggers,
        tools_required: body.tools_required,
        priority: body.priority,
    };
    match crate::skills::write_skill(
        crate::config::WORKSPACE_DIR,
        &skill_name,
        &frontmatter,
        &body.instructions,
    ).await {
        Ok(_) => {
            tracing::info!(skill = %skill_name, "skill upserted via UI (global)");
            Json(serde_json::json!({"ok": true})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

/// DELETE /api/skills/{skill}
pub(crate) async fn api_skill_delete_global(
    State(_state): State<AppState>,
    axum::extract::Path(skill_name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(path) = find_skill_path(crate::config::WORKSPACE_DIR, &skill_name).await else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "skill not found"}))).into_response();
    };

    match tokio::fs::remove_file(&path).await {
        Ok(_) => {
            tracing::info!(skill = %skill_name, "skill deleted via UI (global)");
            Json(serde_json::json!({"ok": true})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Per-agent skills endpoints (compat) ──────────────────────────────────────

/// GET /api/agents/{name}/skills
/// Returns list of all skills for the agent (name, description, triggers, tools_required, priority).
pub(crate) async fn api_skills_list(
    State(_state): State<AppState>,
    axum::extract::Path(_agent_name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let skills = crate::skills::load_skills(crate::config::WORKSPACE_DIR).await;
    let result: Vec<serde_json::Value> = skills.iter().map(|s| {
        serde_json::json!({
            "name": s.meta.name,
            "description": s.meta.description,
            "triggers": s.meta.triggers,
            "tools_required": s.meta.tools_required,
            "priority": s.meta.priority,
            "instructions_len": s.instructions.len(),
        })
    }).collect();
    Json(serde_json::json!({"skills": result})).into_response()
}

/// GET /api/agents/{name}/skills/{skill}
/// Returns the skill content and parsed structured fields.
pub(crate) async fn api_skill_get(
    State(_state): State<AppState>,
    axum::extract::Path((_agent_name, skill_name)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    let Some(path) = find_skill_path(crate::config::WORKSPACE_DIR, &skill_name).await else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "skill not found"}))).into_response();
    };

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            let mut result = serde_json::json!({ "name": skill_name, "content": content });
            if let Some(skill) = crate::skills::SkillDef::parse(&content) {
                result["description"] = serde_json::json!(skill.meta.description);
                result["triggers"] = serde_json::json!(skill.meta.triggers);
                result["tools_required"] = serde_json::json!(skill.meta.tools_required);
                result["priority"] = serde_json::json!(skill.meta.priority);
                result["instructions"] = serde_json::json!(skill.instructions);
            }
            Json(result).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "skill not found"}))).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct SkillUpsertBody {
    description: Option<String>,
    #[serde(default)]
    triggers: Vec<String>,
    #[serde(default)]
    tools_required: Vec<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    pub(crate) instructions: String,
}

/// PUT /api/agents/{name}/skills/{skill}
/// Creates or updates a skill file.
pub(crate) async fn api_skill_upsert(
    State(_state): State<AppState>,
    axum::extract::Path((agent_name, skill_name)): axum::extract::Path<(String, String)>,
    axum::extract::Json(body): axum::extract::Json<SkillUpsertBody>,
) -> impl IntoResponse {
    let frontmatter = crate::skills::SkillFrontmatter {
        name: skill_name.clone(),
        description: body.description.unwrap_or_default(),
        triggers: body.triggers,
        tools_required: body.tools_required,
        priority: body.priority,
    };
    match crate::skills::write_skill(
        crate::config::WORKSPACE_DIR,
        &skill_name,
        &frontmatter,
        &body.instructions,
    ).await {
        Ok(_) => {
            tracing::info!(agent = %agent_name, skill = %skill_name, "skill upserted via UI");
            Json(serde_json::json!({"ok": true})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

/// DELETE /api/agents/{name}/skills/{skill}
/// Deletes a skill file.
pub(crate) async fn api_skill_delete(
    State(_state): State<AppState>,
    axum::extract::Path((_agent_name, skill_name)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    let Some(path) = find_skill_path(crate::config::WORKSPACE_DIR, &skill_name).await else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "skill not found"}))).into_response();
    };

    match tokio::fs::remove_file(&path).await {
        Ok(_) => {
            tracing::info!(skill = %skill_name, "skill deleted via UI");
            Json(serde_json::json!({"ok": true})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}
