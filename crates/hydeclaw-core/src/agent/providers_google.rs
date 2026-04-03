//! Google Gemini API provider —
//! extracted from providers.rs for readability.

use super::*;

// ── Google Gemini API Provider ──────────────────────────────────────────────

pub struct GoogleProvider {
    client: reqwest::Client,
    streaming_client: reqwest::Client,
    base_url: String,
    api_key_name: String,
    /// Vault scope for LLM_CREDENTIALS (provider UUID). When set, checked first.
    credential_scope: Option<String>,
    secrets: Arc<SecretsManager>,
    model: ModelOverride,
    temperature: f64,
    max_tokens: Option<u32>,
}

impl GoogleProvider {
    pub fn new(model: String, temperature: f64, max_tokens: Option<u32>, secrets: Arc<SecretsManager>) -> Self {
        Self::with_options(model, temperature, max_tokens, secrets, None, None)
    }

    pub fn with_options(
        model: String,
        temperature: f64,
        max_tokens: Option<u32>,
        secrets: Arc<SecretsManager>,
        base_url: Option<String>,
        api_key_env: Option<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        let streaming_client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            client,
            streaming_client,
            base_url: base_url.unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string()),
            api_key_name: api_key_env.unwrap_or_else(|| "GOOGLE_API_KEY".to_string()),
            credential_scope: None,
            secrets,
            model: ModelOverride::new(model),
            temperature,
            max_tokens,
        }
    }

    /// Set vault credential scope (provider UUID) for LLM_CREDENTIALS lookup.
    pub fn with_credential_scope(mut self, scope: String) -> Self {
        self.credential_scope = Some(scope);
        self
    }

    async fn resolve_api_key(&self) -> Option<String> {
        super::resolve_credential(
            &self.secrets,
            self.credential_scope.as_deref(),
            &self.api_key_name,
        ).await
    }
}

/// Convert internal messages to Gemini `contents` format.
pub(super) fn messages_to_gemini_format(messages: &[Message]) -> (Option<String>, Vec<serde_json::Value>) {
    let system = messages
        .iter()
        .find(|m| m.role == MessageRole::System)
        .map(|m| m.content.clone());

    let contents: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| m.role != MessageRole::System)
        .map(|msg| {
            let role = match msg.role {
                MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "model",
                _ => "user",
            };

            if msg.role == MessageRole::Tool {
                // Tool result → functionResponse part
                return serde_json::json!({
                    "role": role,
                    "parts": [{
                        "functionResponse": {
                            "name": msg.tool_call_id.as_deref().unwrap_or("unknown"),
                            "response": {
                                "result": msg.content,
                            }
                        }
                    }]
                });
            }

            if msg.role == MessageRole::Assistant
                && let Some(ref tool_calls) = msg.tool_calls
                && !tool_calls.is_empty()
            {
                let mut parts: Vec<serde_json::Value> = Vec::new();
                if !msg.content.is_empty() {
                    parts.push(serde_json::json!({"text": msg.content}));
                }
                for tc in tool_calls {
                    parts.push(serde_json::json!({
                        "functionCall": {
                            "name": tc.name,
                            "args": tc.arguments,
                        }
                    }));
                }
                return serde_json::json!({"role": role, "parts": parts});
            }

            serde_json::json!({
                "role": role,
                "parts": [{"text": msg.content}]
            })
        })
        .collect();

    (system, contents)
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    parts: Option<Vec<GeminiPart>>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GeminiUsage {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<u32>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<u32>,
}

#[async_trait]
impl LlmProvider for GoogleProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let api_key = self.resolve_api_key().await
            .ok_or_else(|| anyhow::anyhow!("Google API key not found ({})", self.api_key_name))?;

        let effective_model = self.model.effective();
        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.base_url.trim_end_matches('/'),
            effective_model,
            api_key
        );

        let (system, contents) = messages_to_gemini_format(messages);

        let mut body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "temperature": self.temperature,
                "maxOutputTokens": self.max_tokens.unwrap_or(8192),
            }
        });

        if let Some(ref sys) = system {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{"text": sys}]
            });
        }

        if !tools.is_empty() {
            let fn_decls: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    let mut params = t.input_schema.clone();
                    strip_empty_required(&mut params);
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": params,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!([{"functionDeclarations": fn_decls}]);
        }

        tracing::info!(
            provider = "google",
            model = %self.model,
            messages = messages.len(),
            tools = tools.len(),
            "calling Google Gemini API"
        );

        // Google uses ?key= in URL, no auth header needed
        let body_text = crate::agent::providers_http::retry_http_post(
            &self.client, &url, &body, "",
            "google", crate::agent::providers_http::RETRYABLE_OPENAI,
        ).await?;

        let api_resp: GeminiResponse = serde_json::from_str(&body_text).map_err(|e| {
            let preview_len = body_text.len().min(500);
            let preview = &body_text[..body_text.floor_char_boundary(preview_len)];
            tracing::error!(provider = "google", body_preview = %preview, "failed to parse response");
            anyhow::anyhow!("google response parse error: {}", e)
        })?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();

        if let Some(candidates) = api_resp.candidates
            && let Some(candidate) = candidates.into_iter().next()
                && let Some(c) = candidate.content
                    && let Some(parts) = c.parts {
                        for (i, part) in parts.into_iter().enumerate() {
                            if let Some(text) = part.text {
                                if !content.is_empty() {
                                    content.push('\n');
                                }
                                content.push_str(&text);
                            }
                            if let Some(fc) = part.function_call {
                                tool_calls.push(hydeclaw_types::ToolCall {
                                    id: format!("call_{}", i),
                                    name: fc.name,
                                    arguments: fc.args.unwrap_or(serde_json::Value::Object(Default::default())),
                                });
                            }
                        }
                    }

        let usage = api_resp.usage_metadata.map(|u| hydeclaw_types::TokenUsage {
            input_tokens: u.prompt_token_count.unwrap_or(0),
            output_tokens: u.candidates_token_count.unwrap_or(0),
        });

        tracing::info!(
            provider = "google",
            content_len = content.len(),
            tool_calls = tool_calls.len(),
            input_tokens = usage.as_ref().map(|u| u.input_tokens).unwrap_or(0),
            output_tokens = usage.as_ref().map(|u| u.output_tokens).unwrap_or(0),
            "Google response parsed"
        );

        Ok(LlmResponse {
            content,
            tool_calls,
            usage,
            model: Some(effective_model),
            provider: Some("google".to_string()),
            fallback_notice: None,
            tools_used: vec![],
            iterations: 0,
            thinking_blocks: vec![],
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        chunk_tx: mpsc::UnboundedSender<String>,
    ) -> Result<LlmResponse> {
        if !tools.is_empty() {
            let response = self.chat(messages, tools).await?;
            if response.tool_calls.is_empty() {
                let filtered = crate::agent::thinking::strip_thinking(&response.content);
                if !filtered.is_empty() {
                    chunk_tx.send(filtered).ok();
                }
            }
            return Ok(response);
        }

        let api_key = self.resolve_api_key().await
            .ok_or_else(|| anyhow::anyhow!("Google API key not found ({})", self.api_key_name))?;

        let effective_model = self.model.effective();
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url.trim_end_matches('/'),
            effective_model,
            api_key
        );

        let (system, contents) = messages_to_gemini_format(messages);

        let mut body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "temperature": self.temperature,
                "maxOutputTokens": self.max_tokens.unwrap_or(8192),
            }
        });

        if let Some(ref sys) = system {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{"text": sys}]
            });
        }

        tracing::info!(provider = "google", model = %self.model, "calling Google Gemini API (streaming)");

        let start = std::time::Instant::now();
        let req = self.streaming_client.post(&url).json(&body);
        let resp = req.send().await?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("google API error: {}", err_text);
        }

        let mut full_content = String::new();
        let mut buffer = String::new();
        let mut thinking_filter = crate::agent::thinking::ThinkingFilter::new();

        use tokio_stream::StreamExt;
        let mut byte_stream = resp.bytes_stream();
        while let Some(chunk_result) = StreamExt::next(&mut byte_stream).await {
            let chunk_bytes = chunk_result?;
            buffer.push_str(&String::from_utf8_lossy(&chunk_bytes));

            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim().to_string();
                buffer = buffer[line_end + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ")
                    && let Ok(chunk_json) = serde_json::from_str::<GeminiResponse>(data)
                        && let Some(candidates) = chunk_json.candidates {
                            for candidate in candidates {
                                if let Some(c) = candidate.content
                                    && let Some(parts) = c.parts {
                                        for part in parts {
                                            if let Some(ref text) = part.text {
                                                full_content.push_str(text);
                                                let filtered = thinking_filter.process(text);
                                                if !filtered.is_empty() {
                                                    chunk_tx.send(filtered).ok();
                                                }
                                            }
                                        }
                                    }
                            }
                        }
            }
        }

        let elapsed = start.elapsed();
        tracing::info!(
            provider = "google",
            content_len = full_content.len(),
            elapsed_ms = elapsed.as_millis() as u64,
            "streaming response complete"
        );

        Ok(LlmResponse {
            content: full_content,
            tool_calls: vec![],
            usage: None,
            model: Some(effective_model),
            provider: Some("google".to_string()),
            fallback_notice: None,
            tools_used: vec![],
            iterations: 0,
            thinking_blocks: vec![],
        })
    }

    fn name(&self) -> &str {
        "google"
    }

    fn set_model_override(&self, model: Option<String>) {
        self.model.set(model);
    }

    fn current_model(&self) -> String {
        self.model.effective()
    }
}

/// Recursively strip `"required": []` from JSON schemas.
/// Google's Gemini API rejects empty required arrays.
fn strip_empty_required(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut() {
        obj.retain(|k, v| !(k == "required" && v.as_array().is_some_and(|a| a.is_empty())));
        for v in obj.values_mut() {
            strip_empty_required(v);
        }
    } else if let Some(arr) = value.as_array_mut() {
        for v in arr.iter_mut() {
            strip_empty_required(v);
        }
    }
}
