//! Tests for `RoutingProvider` failover behavior driven by
//! `LlmCallError::is_failover_worthy`. See Tasks 17/18 of the LLM-timeout
//! refactor.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use hydeclaw_types::{LlmResponse, Message, ToolDefinition};
use tokio::sync::mpsc;

use super::{LlmCallError, LlmProvider, RoutingProvider};

// ── Mock providers ───────────────────────────────────────────────────────────

/// Always returns `InactivityTimeout` (failover-worthy).
struct MockInactivityProvider;

#[async_trait]
impl LlmProvider for MockInactivityProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Err(anyhow::Error::new(LlmCallError::InactivityTimeout {
            provider: "mock-inactivity".into(),
            silent_secs: 60,
            partial_text: "partial".into(),
        }))
    }

    async fn chat_stream(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _chunk_tx: mpsc::UnboundedSender<String>,
    ) -> anyhow::Result<LlmResponse> {
        Err(anyhow::Error::new(LlmCallError::InactivityTimeout {
            provider: "mock-inactivity".into(),
            silent_secs: 60,
            partial_text: "partial".into(),
        }))
    }

    fn name(&self) -> &str {
        "mock-inactivity"
    }
}

/// Always returns `UserCancelled` (NOT failover-worthy).
struct MockUserCancelProvider;

#[async_trait]
impl LlmProvider for MockUserCancelProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Err(anyhow::Error::new(LlmCallError::UserCancelled {
            partial_text: "partial-before-cancel".into(),
        }))
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        _chunk_tx: mpsc::UnboundedSender<String>,
    ) -> anyhow::Result<LlmResponse> {
        self.chat(messages, tools).await
    }

    fn name(&self) -> &str {
        "mock-user-cancel"
    }
}

/// Always returns `AuthError` (NOT failover-worthy — typed path).
struct MockAuthErrorProvider;

#[async_trait]
impl LlmProvider for MockAuthErrorProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Err(anyhow::Error::new(LlmCallError::AuthError {
            provider: "mock-auth".into(),
            status: 401,
        }))
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        _chunk_tx: mpsc::UnboundedSender<String>,
    ) -> anyhow::Result<LlmResponse> {
        self.chat(messages, tools).await
    }

    fn name(&self) -> &str {
        "mock-auth"
    }
}

/// Records whether it was called and returns success with a distinctive content.
struct MockSuccessProvider {
    called: Arc<AtomicBool>,
    marker: &'static str,
}

#[async_trait]
impl LlmProvider for MockSuccessProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.called.store(true, Ordering::SeqCst);
        Ok(LlmResponse {
            content: self.marker.to_string(),
            tool_calls: vec![],
            usage: None,
            finish_reason: Some("stop".to_string()),
            model: None,
            provider: Some("mock-success".to_string()),
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
        _chunk_tx: mpsc::UnboundedSender<String>,
    ) -> anyhow::Result<LlmResponse> {
        self.chat(messages, tools).await
    }

    fn name(&self) -> &str {
        "mock-success"
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn routing_fails_over_on_inactivity_timeout() {
    let called = Arc::new(AtomicBool::new(false));
    let primary: Arc<dyn LlmProvider> = Arc::new(MockInactivityProvider);
    let fallback: Arc<dyn LlmProvider> = Arc::new(MockSuccessProvider {
        called: called.clone(),
        marker: "from-fallback",
    });

    let routing = RoutingProvider::new_for_test(vec![
        ("primary:mock-inactivity".into(), primary, 60),
        ("fallback:mock-success".into(), fallback, 60),
    ]);

    let resp = routing.chat(&[], &[]).await.expect("failover should succeed");
    assert_eq!(resp.content, "from-fallback");
    assert!(called.load(Ordering::SeqCst), "fallback must have been called");
}

#[tokio::test]
async fn routing_does_not_fail_over_on_user_cancel() {
    let called = Arc::new(AtomicBool::new(false));
    let primary: Arc<dyn LlmProvider> = Arc::new(MockUserCancelProvider);
    let fallback: Arc<dyn LlmProvider> = Arc::new(MockSuccessProvider {
        called: called.clone(),
        marker: "from-fallback",
    });

    let routing = RoutingProvider::new_for_test(vec![
        ("primary:mock-user-cancel".into(), primary, 60),
        ("fallback:mock-success".into(), fallback, 60),
    ]);

    let err = routing
        .chat(&[], &[])
        .await
        .expect_err("user-cancelled must bubble up, not fail over");
    let typed = err
        .downcast_ref::<LlmCallError>()
        .expect("error must be an LlmCallError");
    assert!(
        matches!(typed, LlmCallError::UserCancelled { .. }),
        "expected UserCancelled, got {typed:?}"
    );
    // Partial text preserved.
    assert_eq!(typed.partial_text(), Some("partial-before-cancel"));
    assert!(
        !called.load(Ordering::SeqCst),
        "fallback MUST NOT have been called for non-failover-worthy error"
    );
}

#[tokio::test]
async fn routing_does_not_fail_over_on_auth_error() {
    let called = Arc::new(AtomicBool::new(false));
    let primary: Arc<dyn LlmProvider> = Arc::new(MockAuthErrorProvider);
    let fallback: Arc<dyn LlmProvider> = Arc::new(MockSuccessProvider {
        called: called.clone(),
        marker: "from-fallback",
    });

    let routing = RoutingProvider::new_for_test(vec![
        ("primary:mock-auth".into(), primary, 60),
        ("fallback:mock-success".into(), fallback, 60),
    ]);

    let err = routing
        .chat(&[], &[])
        .await
        .expect_err("auth error must bubble up, not fail over");
    let typed = err
        .downcast_ref::<LlmCallError>()
        .expect("error must be an LlmCallError");
    assert!(
        matches!(typed, LlmCallError::AuthError { .. }),
        "expected AuthError, got {typed:?}"
    );
    assert!(
        !called.load(Ordering::SeqCst),
        "fallback MUST NOT have been called for auth error"
    );
}

#[tokio::test]
async fn routing_fails_over_on_streaming_inactivity() {
    let called = Arc::new(AtomicBool::new(false));
    let primary: Arc<dyn LlmProvider> = Arc::new(MockInactivityProvider);
    let fallback: Arc<dyn LlmProvider> = Arc::new(MockSuccessProvider {
        called: called.clone(),
        marker: "streamed-fallback",
    });

    let routing = RoutingProvider::new_for_test(vec![
        ("primary:mock-inactivity".into(), primary, 60),
        ("fallback:mock-success".into(), fallback, 60),
    ]);

    let (tx, _rx) = mpsc::unbounded_channel::<String>();
    let resp = routing
        .chat_stream(&[], &[], tx)
        .await
        .expect("streaming failover should succeed");
    assert_eq!(resp.content, "streamed-fallback");
    assert!(called.load(Ordering::SeqCst));
}
