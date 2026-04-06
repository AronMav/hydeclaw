use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use std::sync::LazyLock;
use regex::Regex;

use crate::agent::providers::PROVIDER_CREDENTIALS;
use crate::db::providers::{self, CreateProvider, UpdateProvider, ProviderRow};
use crate::gateway::AppState;
use crate::secrets::SecretsManager;
use super::secrets::mask_secret_value;

// ── Constants ───────────────────────────────────────────────────────────────
const VALID_TYPES: &[&str] = &["text", "stt", "tts", "vision", "imagegen", "embedding"];
const VALID_CAPABILITIES: &[&str] = &["graph_extraction", "stt", "tts", "vision", "imagegen", "embedding", "compaction"];

static NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9_-]+$").expect("valid regex pattern")
});

/// Media capabilities that should trigger toolgate reload when changed.
const MEDIA_CAPABILITIES: &[&str] = &["stt", "tts", "vision", "imagegen", "embedding"];

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Notify toolgate to reload config and invalidate caches.
pub(crate) fn notify_toolgate_reload(toolgate_url: Option<String>) {
    let url = toolgate_url.unwrap_or_else(|| "http://localhost:9011".to_string());
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        if let Err(e) = client.post(format!("{}/reload", url)).send().await {
            tracing::warn!(error = %e, "failed to reload toolgate config");
        }
    });
}

/// Resolve API key for a provider from vault (scoped by UUID).
async fn resolve_key(secrets: &SecretsManager, provider: &ProviderRow) -> Option<String> {
    secrets.get_scoped(PROVIDER_CREDENTIALS, &provider.id.to_string()).await
}

/// Build the public JSON representation of a provider (masked api_key).
async fn provider_json(secrets: &SecretsManager, p: &ProviderRow) -> Value {
    let key = resolve_key(secrets, p).await;
    let mut obj = serde_json::to_value(p).unwrap_or_default();
    if let Some(map) = obj.as_object_mut() {
        map.insert("api_key".into(), json!(key.as_deref().map(mask_secret_value)));
        map.insert("has_api_key".into(), json!(key.is_some()));
    }
    obj
}

// ── CRUD handlers ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct ListProvidersQuery {
    #[serde(rename = "type")]
    pub category: Option<String>,
}

pub(crate) async fn api_list_providers(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ListProvidersQuery>,
) -> impl IntoResponse {
    let result = if let Some(ref cat) = params.category {
        providers::list_providers_by_type(&state.db, cat).await
    } else {
        providers::list_providers(&state.db).await
    };
    match result {
        Ok(providers) => {
            let mut out = Vec::with_capacity(providers.len());
            for p in &providers {
                out.push(provider_json(&state.secrets, p).await);
            }
            (StatusCode::OK, Json(json!({ "providers": out }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// Inline body that extends CreateProvider with an optional api_key.
#[derive(Debug, Deserialize)]
pub(crate) struct CreateProviderBody {
    pub name: String,
    #[serde(rename = "type")]
    pub category: String,
    pub provider_type: String,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub enabled: Option<bool>,
    pub options: Option<Value>,
    pub notes: Option<String>,
    pub api_key: Option<String>,
}

pub(crate) async fn api_create_provider(
    State(state): State<AppState>,
    Json(body): Json<CreateProviderBody>,
) -> impl IntoResponse {
    // Validate type
    if !VALID_TYPES.contains(&body.category.as_str()) {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": format!("invalid type '{}', must be one of: {}", body.category, VALID_TYPES.join(", "))
        }))).into_response();
    }
    // Validate name
    if !NAME_RE.is_match(&body.name) {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "name must match [a-zA-Z0-9_-]+"
        }))).into_response();
    }
    // For type=text, require default_model
    if body.category == "text" && body.default_model.as_ref().is_none_or(|m| m.is_empty()) {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "default_model is required for type=text"
        }))).into_response();
    }

    let api_key = body.api_key.clone().filter(|k| !k.is_empty());
    let input = CreateProvider {
        name: body.name,
        category: body.category,
        provider_type: body.provider_type,
        base_url: body.base_url,
        default_model: body.default_model,
        enabled: body.enabled,
        options: body.options,
        notes: body.notes,
    };

    match providers::create_provider(&state.db, input).await {
        Ok(p) => {
            if let Some(key) = api_key {
                let desc = format!("Credentials for provider '{}'", p.name);
                if let Err(e) = state.secrets.set_scoped(PROVIDER_CREDENTIALS, &p.id.to_string(), &key, Some(&desc)).await {
                    tracing::warn!(provider = %p.name, error = %e, "failed to store provider key in vault");
                }
            }
            if p.category != "text" {
                notify_toolgate_reload(state.config.toolgate_url.clone());
            }
            let json = provider_json(&state.secrets, &p).await;
            (StatusCode::CREATED, Json(json)).into_response()
        }
        Err(e) if e.to_string().contains("unique") || e.to_string().contains("duplicate") => {
            (StatusCode::CONFLICT, Json(json!({"error": "a provider with this name already exists"}))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

pub(crate) async fn api_get_provider(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match providers::get_provider(&state.db, id).await {
        Ok(Some(p)) => (StatusCode::OK, Json(provider_json(&state.secrets, &p).await)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Inline body that extends UpdateProvider with an optional api_key.
#[derive(Debug, Deserialize)]
pub(crate) struct UpdateProviderBody {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub category: Option<String>,
    pub provider_type: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub enabled: Option<bool>,
    pub options: Option<Value>,
    pub notes: Option<String>,
    pub api_key: Option<String>,
}

pub(crate) async fn api_update_provider(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateProviderBody>,
) -> impl IntoResponse {
    // Validate type if changing
    if let Some(ref cat) = body.category
        && !VALID_TYPES.contains(&cat.as_str())
    {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": format!("invalid type '{}', must be one of: {}", cat, VALID_TYPES.join(", "))
        }))).into_response();
    }

    // Check if type is changing — need to clear provider_active references
    let old_provider = if body.category.is_some() {
        providers::get_provider(&state.db, id).await.ok().flatten()
    } else {
        None
    };

    let api_key = body.api_key.clone().filter(|k| !k.is_empty());
    let input = UpdateProvider {
        name: body.name,
        category: body.category,
        provider_type: body.provider_type,
        base_url: body.base_url,
        default_model: body.default_model,
        enabled: body.enabled,
        options: body.options,
        notes: body.notes,
    };

    match providers::update_provider(&state.db, id, input).await {
        Ok(Some(p)) => {
            if let Some(key) = api_key {
                let desc = format!("Credentials for provider '{}'", p.name);
                if let Err(e) = state.secrets.set_scoped(PROVIDER_CREDENTIALS, &p.id.to_string(), &key, Some(&desc)).await {
                    tracing::warn!(provider = %p.name, error = %e, "failed to update provider key in vault");
                }
            }

            // If type changed, clear provider_active entries that referenced this provider by name
            if let Some(old) = old_provider
                && old.category != p.category
            {
                // Clear active binding for old capabilities that referenced this provider
                let active = providers::list_provider_active(&state.db).await.unwrap_or_default();
                for a in active {
                    if a.provider_name.as_deref() == Some(&p.name) {
                        let _ = providers::set_provider_active(&state.db, &a.capability, None).await;
                    }
                }
            }

            if p.category != "text" {
                notify_toolgate_reload(state.config.toolgate_url.clone());
            }
            let json = provider_json(&state.secrets, &p).await;
            (StatusCode::OK, Json(json)).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

pub(crate) async fn api_delete_provider(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // Check type before deleting to decide about toolgate reload
    let provider = providers::get_provider(&state.db, id).await.ok().flatten();
    match providers::delete_provider(&state.db, id).await {
        Ok(true) => {
            if let Err(e) = state.secrets.delete_scoped(PROVIDER_CREDENTIALS, &id.to_string()).await {
                tracing::debug!(provider = %id, error = %e, "no vault key to delete for provider");
            }
            if provider.is_some_and(|p| p.category != "text") {
                notify_toolgate_reload(state.config.toolgate_url.clone());
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Model discovery ─────────────────────────────────────────────────────────

pub(crate) async fn api_unified_provider_models(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let provider = match providers::get_provider(&state.db, id).await {
        Ok(Some(p)) => p,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"error": "provider not found"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    let api_key = resolve_key(&state.secrets, &provider).await;

    let models = crate::agent::model_discovery::discover_models_with_key(
        &provider.provider_type,
        &state.secrets,
        provider.base_url.as_deref(),
        api_key.as_deref(),
    )
    .await
    .unwrap_or_default();

    (StatusCode::OK, Json(json!({ "models": models }))).into_response()
}

// ── Resolve (unmasked credentials for internal use) ─────────────────────────

pub(crate) async fn api_provider_resolve(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let provider = match providers::get_provider(&state.db, id).await {
        Ok(Some(p)) => p,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"error": "provider not found"}))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    };

    let api_key = resolve_key(&state.secrets, &provider).await.unwrap_or_default();

    Json(json!({
        "base_url": provider.base_url,
        "provider_type": provider.provider_type,
        "default_model": provider.default_model,
        "api_key": api_key,
    })).into_response()
}

// ── Active handlers ─────────────────────────────────────────────────────────

pub(crate) async fn api_list_provider_active(State(state): State<AppState>) -> impl IntoResponse {
    match providers::list_provider_active(&state.db).await {
        Ok(active) => (StatusCode::OK, Json(json!({ "active": active }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetProviderActiveInput {
    pub capability: String,
    pub provider_name: Option<String>,
}

pub(crate) async fn api_set_provider_active(
    State(state): State<AppState>,
    Json(input): Json<SetProviderActiveInput>,
) -> impl IntoResponse {
    if !VALID_CAPABILITIES.contains(&input.capability.as_str()) {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": format!("invalid capability '{}', must be one of: {}", input.capability, VALID_CAPABILITIES.join(", "))
        }))).into_response();
    }
    match providers::set_provider_active(
        &state.db,
        &input.capability,
        input.provider_name.as_deref(),
    )
    .await
    {
        Ok(row) => {
            if MEDIA_CAPABILITIES.contains(&input.capability.as_str()) {
                notify_toolgate_reload(state.config.toolgate_url.clone());
            }
            (StatusCode::OK, Json(json!(row))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

// ── Toolgate config export (internal, unmasked keys) ────────────────────────

/// Internal endpoint for toolgate — returns full config with real api_keys.
/// Emits `"driver"` field (mapped from `provider_type`) which toolgate matches on.
/// Build media config JSON — used by API handler and main.rs toolgate export.
pub(crate) async fn build_media_config(state: &AppState) -> Value {
    // Collect all media-type providers
    let mut all_providers = Vec::new();
    for media_type in &["stt", "tts", "vision", "imagegen", "embedding"] {
        if let Ok(rows) = providers::list_providers_by_type(&state.db, media_type).await {
            all_providers.extend(rows);
        }
    }

    let active_rows = providers::list_provider_active(&state.db).await.unwrap_or_default();

    let mut provider_map = serde_json::Map::new();
    for p in &all_providers {
        if !p.enabled {
            continue;
        }
        let api_key = resolve_key(&state.secrets, p).await;
        provider_map.insert(
            p.name.clone(),
            json!({
                "type":     p.category,
                "driver":   p.provider_type,
                "base_url": p.base_url,
                "model":    p.default_model,
                "api_key":  api_key,
                "enabled":  p.enabled,
                "options":  p.options,
            }),
        );
    }

    let mut active_map = serde_json::Map::new();
    for a in active_rows {
        // Only include media capabilities
        if MEDIA_CAPABILITIES.contains(&a.capability.as_str()) {
            active_map.insert(a.capability, json!(a.provider_name));
        }
    }

    json!({
        "version": 1,
        "active": active_map,
        "providers": provider_map,
    })
}

pub(crate) async fn api_media_config_export(State(state): State<AppState>) -> Json<Value> {
    Json(build_media_config(&state).await)
}

// ── Static metadata ─────────────────────────────────────────────────────────

pub(crate) async fn api_list_media_drivers() -> Json<Value> {
    Json(json!({
        "drivers": {
            "stt": [
                {"driver": "whisper-local", "label": "Local Whisper (faster-whisper)", "requires_key": false},
                {"driver": "openai", "label": "OpenAI Whisper", "requires_key": true},
                {"driver": "groq", "label": "Groq", "requires_key": true},
                {"driver": "deepgram", "label": "Deepgram", "requires_key": true},
                {"driver": "google", "label": "Google Gemini", "requires_key": true},
                {"driver": "mistral", "label": "Mistral (Voxtral)", "requires_key": true},
                {"driver": "assemblyai", "label": "AssemblyAI (100+ langs)", "requires_key": true},
            ],
            "vision": [
                {"driver": "ollama", "label": "Local Ollama", "requires_key": false},
                {"driver": "openai", "label": "OpenAI GPT-4o", "requires_key": true},
                {"driver": "google", "label": "Google Gemini", "requires_key": true},
                {"driver": "anthropic", "label": "Anthropic Claude", "requires_key": true},
                {"driver": "replicate", "label": "Replicate (Moondream/LLaVA)", "requires_key": true},
                {"driver": "qwen", "label": "Qwen2-VL (Alibaba)", "requires_key": true},
                {"driver": "cloudsight", "label": "CloudSight", "requires_key": true},
            ],
            "tts": [
                {"driver": "openai", "label": "OpenAI TTS", "requires_key": true},
                {"driver": "elevenlabs", "label": "ElevenLabs", "requires_key": true},
                {"driver": "edge", "label": "Microsoft Edge TTS (free)", "requires_key": false},
                {"driver": "qwen3-tts", "label": "Local Qwen3-TTS", "requires_key": false},
                {"driver": "fish-audio", "label": "Fish Audio (Russian voices)", "requires_key": true},
                {"driver": "murf", "label": "Murf AI", "requires_key": true},
            ],
            "imagegen": [
                {"driver": "openai", "label": "OpenAI (DALL-E / GPT Image)", "requires_key": true},
                {"driver": "runware", "label": "Runware (FLUX, SDXL, etc.)", "requires_key": true},
                {"driver": "stability", "label": "Stability AI (SD3/SD3.5)", "requires_key": true},
                {"driver": "fal", "label": "fal.ai (FLUX fast)", "requires_key": true},
                {"driver": "pixazo", "label": "Pixazo", "requires_key": true},
            ],
            "embedding": [
                {"driver": "ollama", "label": "Ollama Embedding", "requires_key": false},
                {"driver": "openai", "label": "OpenAI Embedding", "requires_key": true},
            ],
        }
    }))
}

pub(crate) async fn api_list_provider_types() -> Json<Value> {
    let types: Vec<Value> = crate::agent::providers::PROVIDER_TYPES
        .iter()
        .map(|pt| {
            json!({
                "id": pt.id,
                "name": pt.name,
                "default_base_url": pt.default_base_url,
                "chat_path": pt.chat_path,
                "default_secret_name": pt.default_secret_name,
                "requires_api_key": pt.requires_api_key,
                "supports_model_listing": pt.supports_model_listing,
            })
        })
        .collect();
    Json(json!({ "provider_types": types }))
}

// ── Vault migration ─────────────────────────────────────────────────────────

/// One-time startup migration: copy provider API keys from legacy vault patterns
/// (LLM_CREDENTIALS::{uuid} and MEDIA_CREDENTIALS::{name}) into the new
/// PROVIDER_CREDENTIALS::{uuid} pattern.
/// Idempotent — providers already migrated are skipped.
pub async fn migrate_provider_keys_to_vault(db: &PgPool, secrets: &SecretsManager) {
    let all_providers = match providers::list_providers(db).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "migrate_provider_keys: failed to list providers");
            return;
        }
    };

    let mut migrated = 0u32;
    for p in all_providers {
        let scope = p.id.to_string();

        // Already migrated?
        if secrets.get_scoped(PROVIDER_CREDENTIALS, &scope).await.is_some() {
            continue;
        }

        // Try legacy LLM vault key: LLM_CREDENTIALS scoped by UUID
        if let Some(key) = secrets.get_scoped(crate::agent::providers::LLM_CREDENTIALS, &scope).await {
            let desc = format!("Credentials for provider '{}' (migrated from LLM_CREDENTIALS)", p.name);
            if let Err(e) = secrets.set_scoped(PROVIDER_CREDENTIALS, &scope, &key, Some(&desc)).await {
                tracing::error!(provider = %p.name, error = %e, "migrate_provider_keys: vault write failed");
            } else {
                migrated += 1;
                tracing::info!(provider = %p.name, "migrate_provider_keys: migrated from LLM_CREDENTIALS");
            }
            continue;
        }

        // Try legacy media vault key: MEDIA_CREDENTIALS scoped by name
        const LEGACY_MEDIA_CREDENTIALS: &str = "MEDIA_CREDENTIALS";
        if let Some(key) = secrets.get_scoped(LEGACY_MEDIA_CREDENTIALS, &p.name).await {
            let desc = format!("Credentials for provider '{}' (migrated from MEDIA_CREDENTIALS)", p.name);
            if let Err(e) = secrets.set_scoped(PROVIDER_CREDENTIALS, &scope, &key, Some(&desc)).await {
                tracing::error!(provider = %p.name, error = %e, "migrate_provider_keys: vault write failed");
            } else {
                migrated += 1;
                tracing::info!(provider = %p.name, "migrate_provider_keys: migrated from MEDIA_CREDENTIALS");
            }
            continue;
        }
    }

    if migrated > 0 {
        tracing::info!(count = migrated, "migrate_provider_keys: complete");
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_types_complete() {
        assert!(VALID_TYPES.contains(&"text"));
        assert!(VALID_TYPES.contains(&"stt"));
        assert!(VALID_TYPES.contains(&"embedding"));
        assert!(!VALID_TYPES.contains(&"audio"));
    }

    #[test]
    fn valid_capabilities_complete() {
        assert!(VALID_CAPABILITIES.contains(&"graph_extraction"));
        assert!(VALID_CAPABILITIES.contains(&"stt"));
        assert!(!VALID_CAPABILITIES.contains(&"text"));
    }

    #[test]
    fn provider_active_row_serializes() {
        let row = crate::db::providers::ProviderActiveRow {
            capability: "stt".into(),
            provider_name: Some("whisper-local".into()),
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["capability"], "stt");
        assert_eq!(json["provider_name"], "whisper-local");
    }

    #[test]
    fn create_provider_deserializes() {
        let json = serde_json::json!({
            "name": "my-provider",
            "type": "text",
            "provider_type": "openai",
            "default_model": "gpt-4o"
        });
        let input: crate::db::providers::CreateProvider = serde_json::from_value(json).unwrap();
        assert_eq!(input.category, "text");
        assert_eq!(input.provider_type, "openai");
    }

    fn is_valid_type(t: &str) -> bool { VALID_TYPES.contains(&t) }
    fn is_valid_capability(c: &str) -> bool { VALID_CAPABILITIES.contains(&c) }

    #[test]
    fn type_validation() {
        assert!(is_valid_type("text"));
        assert!(is_valid_type("embedding"));
        assert!(!is_valid_type(""));
        assert!(!is_valid_type("TEXT"));
    }

    #[test]
    fn capability_validation() {
        assert!(is_valid_capability("graph_extraction"));
        assert!(is_valid_capability("stt"));
        assert!(!is_valid_capability("text"));
        assert!(!is_valid_capability(""));
    }
}
