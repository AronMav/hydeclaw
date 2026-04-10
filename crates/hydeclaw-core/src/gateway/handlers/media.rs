use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
};
use serde_json::json;

use super::super::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/uploads/{filename}", get(api_media_serve))
        .merge(
            Router::new()
                .route("/api/media/upload", post(api_media_upload))
                .layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024)) // 20 MB
        )
}

/// POST /api/media/upload — multipart upload, saves to workspace/uploads/{uuid}.{ext}
pub(crate) async fn api_media_upload(
    State(state): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    let workspace_dir = state.agent_deps.read().await.workspace_dir.clone();
    let uploads_dir = std::path::PathBuf::from(&workspace_dir).join("uploads");
    if let Err(e) = tokio::fs::create_dir_all(&uploads_dir).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("mkdir: {e}")}))).into_response();
    }

    let field = match multipart.next_field().await {
        Ok(Some(f)) => f,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": "no file field in multipart"}))).into_response(),
    };

    let original_name = field.file_name().unwrap_or("file").to_string();
    let ext = original_name.rsplit('.').next().unwrap_or("bin").to_lowercase();
    // Only allow safe media extensions — reject html/svg/etc to prevent XSS
    const SAFE_EXTENSIONS: &[&str] = &[
        "jpg", "jpeg", "png", "gif", "webp", "bmp", "ico",
        "mp4", "webm", "mov", "avi",
        "ogg", "oga", "mp3", "wav", "flac", "aac", "m4a",
        "pdf", "docx", "xlsx", "pptx",
        "txt", "md", "csv", "log", "json", "toml", "yaml", "yml",
        "zip", "tar", "gz", "bin",
    ];
    let ext = if SAFE_EXTENSIONS.contains(&ext.as_str()) { ext } else { "bin".to_string() };
    let uuid = uuid::Uuid::new_v4();
    let filename = format!("{}.{}", uuid, ext);
    let path = uploads_dir.join(&filename);

    let data = match field.bytes().await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("read: {e}")}))).into_response(),
    };

    // 20 MB limit
    if data.len() > 20 * 1024 * 1024 {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error": "file too large (max 20MB)"}))).into_response();
    }

    if let Err(e) = tokio::fs::write(&path, &data).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("write: {e}")}))).into_response();
    }

    let base = if let Some(ref pu) = state.config.gateway.public_url {
        pu.trim_end_matches('/').to_string()
    } else {
        let port = state.config.gateway.listen.rsplit(':').next().unwrap_or("18789");
        format!("http://localhost:{}", port)
    };
    let url = format!("{}/uploads/{}", base, filename);
    Json(json!({"url": url, "filename": filename, "size": data.len()})).into_response()
}

/// GET /uploads/{filename} — serve uploaded files (public, no auth)
pub(crate) async fn api_media_serve(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> impl IntoResponse {
    // Prevent path traversal
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let workspace_dir = state.agent_deps.read().await.workspace_dir.clone();
    let path = std::path::PathBuf::from(&workspace_dir).join("uploads").join(&filename);

    let data = match tokio::fs::read(&path).await {
        Ok(d) => d,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    // Guess content-type from extension
    let ct = match filename.rsplit('.').next().unwrap_or("") {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "txt" | "md" | "csv" | "log" => "text/plain; charset=utf-8",
        "json" => "application/json",
        _ => "application/octet-stream",
    };

    let disposition = if ct.starts_with("image/") || ct.starts_with("audio/") || ct.starts_with("video/") {
        "inline"
    } else {
        "attachment"
    };

    ([
        (axum::http::header::CONTENT_TYPE, ct),
        (axum::http::header::CONTENT_DISPOSITION, disposition),
        (axum::http::header::CACHE_CONTROL, "private, no-store"),
    ], data).into_response()
}
