//! Pipeline step: handlers — tool result handlers for workspace, browser (migrated from engine_handlers.rs).
//!
//! Each function takes explicit dependencies instead of `&self` on `AgentEngine`.

use anyhow::Result;

use crate::agent::workspace;

// ── Workspace handlers ──────────────────────────────────────────

/// Internal tool: write a workspace file.
pub async fn handle_workspace_write(
    workspace_dir: &str,
    agent_name: &str,
    is_base: bool,
    args: &serde_json::Value,
) -> String {
    let filename = args.get("filename").and_then(|v| v.as_str()).unwrap_or("");
    // Accept content as string or convert other JSON types to string
    let content = match args.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };

    if filename.is_empty() {
        return "Error: 'filename' is required".to_string();
    }

    match workspace::write_workspace_file(workspace_dir, agent_name, filename, &content, is_base)
        .await
    {
        Ok(()) => format!("Successfully updated {} ({}B)", filename, content.len()),
        Err(e) => {
            tracing::error!(
                filename = %filename,
                workspace = %workspace_dir,
                agent = %agent_name,
                error = %e,
                "workspace_write failed"
            );
            format!("Error writing {}: {}", filename, e)
        }
    }
}

/// Internal tool: read a file from workspace.
pub async fn handle_workspace_read(
    workspace_dir: &str,
    agent_name: &str,
    args: &serde_json::Value,
) -> String {
    let filename = args.get("filename").and_then(|v| v.as_str()).unwrap_or("");

    if filename.is_empty() {
        return "Error: 'filename' is required".to_string();
    }

    match workspace::read_workspace_file(workspace_dir, agent_name, filename).await {
        Ok(content) => content,
        Err(e) => format!("Error reading '{}': {}", filename, e),
    }
}

/// Internal tool: list files in workspace directory.
pub async fn handle_workspace_list(
    workspace_dir: &str,
    agent_name: &str,
    args: &serde_json::Value,
) -> String {
    let directory = args
        .get("directory")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    match workspace::list_workspace_files(workspace_dir, agent_name, directory).await {
        Ok(listing) => listing,
        Err(e) => format!("Error listing '{}': {}", directory, e),
    }
}

/// Internal tool: edit a file by replacing a text substring.
pub async fn handle_workspace_edit(
    workspace_dir: &str,
    agent_name: &str,
    is_base: bool,
    args: &serde_json::Value,
) -> String {
    let filename = args.get("filename").and_then(|v| v.as_str()).unwrap_or("");
    let old_text = args.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
    let new_text = args.get("new_text").and_then(|v| v.as_str()).unwrap_or("");

    if filename.is_empty() || old_text.is_empty() {
        return "Error: 'filename' and 'old_text' are required".to_string();
    }

    match workspace::edit_workspace_file(
        workspace_dir,
        agent_name,
        filename,
        old_text,
        new_text,
        is_base,
    )
    .await
    {
        Ok(()) => format!("Successfully edited '{}'", filename),
        Err(e) => format!("Error editing '{}': {}", filename, e),
    }
}

/// Internal tool: delete a workspace file.
pub async fn handle_workspace_delete(
    workspace_dir: &str,
    agent_name: &str,
    args: &serde_json::Value,
) -> String {
    let filename = args.get("filename").and_then(|v| v.as_str()).unwrap_or("");
    if filename.is_empty() {
        return "Error: 'filename' is required".to_string();
    }
    match workspace::delete_workspace_file(workspace_dir, agent_name, filename).await {
        Ok(()) => format!("Deleted '{}'", filename),
        Err(e) => format!("Error deleting '{}': {}", filename, e),
    }
}

/// Internal tool: rename/move a workspace file.
pub async fn handle_workspace_rename(
    workspace_dir: &str,
    agent_name: &str,
    args: &serde_json::Value,
) -> String {
    let old_path = args
        .get("old_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new_path = args
        .get("new_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if old_path.is_empty() || new_path.is_empty() {
        return "Error: 'old_path' and 'new_path' are required".to_string();
    }
    match workspace::rename_workspace_file(workspace_dir, agent_name, old_path, new_path).await {
        Ok(()) => format!("Moved '{}' → '{}'", old_path, new_path),
        Err(e) => format!("Error moving '{}': {}", old_path, e),
    }
}

// ── Browser handler ─────────────────────────────────────────────

/// Handle browser automation actions via browser-renderer /automation endpoint.
pub async fn handle_browser_action(
    http_client: &reqwest::Client,
    browser_renderer_url: &str,
    args: &serde_json::Value,
) -> String {
    // SSRF protection: validate URL in navigate actions to block internal services
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
    if (action == "navigate" || action == "create_session")
        && let Some(url) = args.get("url").and_then(|v| v.as_str())
        && let Err(e) = crate::tools::ssrf::validate_url_scheme(url)
    {
        return format!("Error: {e}");
    }
    match br_post(http_client, browser_renderer_url, "/automation", args.clone()).await {
        Ok(result) => {
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
        }
        Err(e) => format!("Error: {e}"),
    }
}

/// POST to browser-renderer at the given base URL + path.
async fn br_post(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("browser-renderer request failed: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("failed to read browser-renderer response: {e}"))?;
    if !status.is_success() {
        return Err(format!("browser-renderer {status}: {text}"));
    }
    serde_json::from_str(&text)
        .map_err(|e| format!("invalid JSON from browser-renderer: {e} — raw: {text}"))
}

// ── Media helpers ───────────────────────────────────────────────

/// Save binary data to workspace/uploads/ and return (public_url, media_type).
pub async fn save_binary_to_uploads(
    workspace_dir: &str,
    data: &[u8],
    hint: &str,
) -> Result<(String, String)> {
    let uploads_dir = std::path::PathBuf::from(workspace_dir).join("uploads");
    tokio::fs::create_dir_all(&uploads_dir).await?;

    // Detect image type from magic bytes
    let (ext, media_type) = detect_media_type(data, hint);
    let uuid = uuid::Uuid::new_v4();
    let filename = format!("{}.{}", uuid, ext);
    let path = uploads_dir.join(&filename);

    tokio::fs::write(&path, data).await?;

    let url = format!("/uploads/{}", filename);
    tracing::info!(url = %url, media_type = %media_type, bytes = data.len(), "saved media to uploads");
    Ok((url, media_type))
}

/// Detect media type from magic bytes, returning (extension, mime_type).
pub fn detect_media_type(data: &[u8], hint: &str) -> (&'static str, String) {
    // Check magic bytes
    if data.len() >= 8 {
        if data.starts_with(b"\x89PNG") {
            return ("png", "image/png".into());
        }
        if data.starts_with(b"\xFF\xD8\xFF") {
            return ("jpg", "image/jpeg".into());
        }
        if data.starts_with(b"GIF8") {
            return ("gif", "image/gif".into());
        }
        if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
            return ("webp", "image/webp".into());
        }
        if data.starts_with(b"OggS") {
            return ("ogg", "audio/ogg".into());
        }
    }
    // Fallback based on hint
    match hint {
        "image" => ("png", "image/png".into()),
        "audio" => ("ogg", "audio/ogg".into()),
        _ => ("bin", "application/octet-stream".into()),
    }
}
