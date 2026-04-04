//! Shared HTTP utilities for LLM providers: retry loop, SSE parsing.

use anyhow::Result;
use rand::Rng;
use std::time::Duration;
use tokio::sync::mpsc;

/// Configurable backoff policy for HTTP retries.
pub struct BackoffPolicy {
    pub base: Duration,
    pub factor: f64,
    pub max_delay: Duration,
    pub jitter: Duration,
    pub max_retries: u32,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            factor: 3.0,
            max_delay: Duration::from_secs(30),
            jitter: Duration::from_millis(500),
            max_retries: 3,
        }
    }
}

impl BackoffPolicy {
    fn delay(&self, attempt: u32) -> Duration {
        let exp = self.base.as_millis() as f64 * self.factor.powi(attempt as i32);
        let capped = exp.min(self.max_delay.as_millis() as f64) as u64;
        let jitter_ms = if self.jitter.as_millis() > 0 {
            rand::rng().random_range(0..self.jitter.as_millis() as u64)
        } else {
            0
        };
        Duration::from_millis(capped + jitter_ms)
    }
}

/// Retry an HTTP POST request with exponential backoff + jitter.
pub async fn retry_http_post(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    api_key: &str,
    provider_name: &str,
    retryable_codes: &[u16],
) -> Result<String> {
    retry_http_post_custom(client, url, body, provider_name, retryable_codes, |req| {
        if !api_key.is_empty() {
            req.bearer_auth(api_key)
        } else {
            req
        }
    }).await
}

/// Like [`retry_http_post`] but accepts a closure to customize each request
/// (e.g. add custom auth headers). The closure receives a `RequestBuilder`
/// that already has URL and JSON body set, and must return the builder.
pub async fn retry_http_post_custom(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    provider_name: &str,
    retryable_codes: &[u16],
    mut customize: impl FnMut(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> Result<String> {
    let policy = BackoffPolicy::default();
    let mut last_error = String::new();

    for attempt in 0..policy.max_retries {
        let start = std::time::Instant::now();
        let req = client.post(url).json(body);
        let req = customize(req);

        let resp_result = req.send().await;
        let elapsed = start.elapsed();

        match resp_result {
            Ok(resp) => {
                let status = resp.status();
                tracing::info!(
                    provider = %provider_name,
                    status = %status,
                    elapsed_ms = elapsed.as_millis() as u64,
                    attempt,
                    "LLM API responded"
                );

                if status.is_success() {
                    return Ok(resp.text().await?);
                }

                let err_text = resp.text().await.unwrap_or_default();
                last_error = format!("{} API error {}: {}", provider_name, status, err_text);

                if status.as_u16() == 400 {
                    let body_preview = serde_json::to_string(body).unwrap_or_default();
                    let truncated = &body_preview[..body_preview.len().min(4000)];
                    tracing::error!(
                        provider = %provider_name,
                        request_body = %truncated,
                        "400 Bad Request — dumping request body for diagnosis"
                    );
                }

                let retryable = retryable_codes.contains(&status.as_u16());
                if !retryable || attempt == policy.max_retries - 1 {
                    anyhow::bail!("{}", last_error);
                }

                let backoff = policy.delay(attempt);
                tracing::warn!(
                    provider = %provider_name,
                    status = %status,
                    attempt,
                    backoff_ms = backoff.as_millis() as u64,
                    "retrying LLM request"
                );
                tokio::time::sleep(backoff).await;
            }
            Err(e) => {
                last_error = format!("{} request error: {}", provider_name, e);
                tracing::warn!(
                    provider = %provider_name,
                    error = %e,
                    attempt,
                    "LLM request failed"
                );

                if attempt == policy.max_retries - 1 {
                    anyhow::bail!("{}", last_error);
                }

                tokio::time::sleep(policy.delay(attempt)).await;
            }
        }
    }

    if !last_error.is_empty() {
        anyhow::bail!("{}", last_error);
    }
    anyhow::bail!("{} request failed after all retries", provider_name)
}

/// Standard retryable HTTP status codes for OpenAI-compatible providers.
pub const RETRYABLE_OPENAI: &[u16] = &[429, 500, 502, 503];

/// Retryable codes for Anthropic (includes 529 overloaded).
pub const RETRYABLE_ANTHROPIC: &[u16] = &[429, 500, 502, 503, 529];

/// Parse an SSE byte stream, calling `on_data` for each `data:` line.
/// Returns the accumulated full content and thinking filter state.
#[allow(dead_code)]
pub async fn parse_sse_stream(
    resp: reqwest::Response,
    chunk_tx: &mpsc::UnboundedSender<String>,
    mut on_data: impl FnMut(&str, &mut crate::agent::thinking::ThinkingFilter, &mpsc::UnboundedSender<String>) -> SseAction,
) -> Result<()> {
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
            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    return Ok(());
                }
                match on_data(data, &mut thinking_filter, chunk_tx) {
                    SseAction::Continue => {}
                    SseAction::Done => return Ok(()),
                }
            }
        }
    }
    Ok(())
}

/// Control flow from SSE data handler.
#[allow(dead_code)]
pub enum SseAction {
    Continue,
    Done,
}
