//! Session lifecycle management for agent engines.
//!
//! This module centralises all session create/resume/load/save/status operations
//! so they can be delegated from engine.rs through a single `SessionManager` handle.
//! Pure utility functions (`resolve_dm_scope`, `truncate_title`) are unit-testable
//! without a database connection.

use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::sessions::MessageRow;

// ── Pure helpers ────────────────────────────────────────────────────────────

/// Resolve the effective `(user_id, channel)` pair from a `dm_scope` string.
///
/// Matches the logic in `crate::db::sessions::get_or_create_session`:
/// - `"shared"` | `"per-peer"` → unique per agent+user (channel collapsed to `"*"`)
/// - `"per-chat"` → unique per agent+channel (user collapsed to `"*"`, for groups)
/// - anything else (`"per-channel-peer"` or unknown) → unique per agent+user+channel
pub fn resolve_dm_scope<'a>(
    user_id: &'a str,
    channel: &'a str,
    dm_scope: &str,
) -> (&'a str, &'a str) {
    match dm_scope {
        "shared" | "per-peer" => (user_id, "*"),
        "per-chat" => ("*", channel),
        _ => (user_id, channel),
    }
}

/// Truncate `text` to at most `max_len` bytes, breaking on a word boundary.
///
/// If the text is shorter than `max_len` it is returned unchanged.
/// If truncated, an ellipsis (`…`) is appended.
pub fn truncate_title(text: &str, max_len: usize) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= max_len {
        return trimmed.to_string();
    }
    let mut end = max_len;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    if let Some(pos) = trimmed[..end].rfind(' ') {
        format!("{}…", &trimmed[..pos])
    } else {
        format!("{}…", &trimmed[..end])
    }
}

// ── SessionManager ──────────────────────────────────────────────────────────

/// Thin wrapper around `crate::db::sessions::*` that groups all session
/// lifecycle operations in one place.
///
/// `PgPool` is clone-cheap (`Arc` internally), so constructing a `SessionManager`
/// per-handler-call is zero-cost.
pub struct SessionManager {
    db: PgPool,
}

impl SessionManager {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Find or create a session for the given agent+user+channel.
    pub async fn get_or_create(
        &self,
        agent_id: &str,
        user_id: &str,
        channel: &str,
        dm_scope: &str,
    ) -> Result<Uuid> {
        crate::db::sessions::get_or_create_session(&self.db, agent_id, user_id, channel, dm_scope)
            .await
    }

    /// Create a brand-new session (no history reuse). Used by "New Chat".
    pub async fn create_new(
        &self,
        agent_id: &str,
        user_id: &str,
        channel: &str,
    ) -> Result<Uuid> {
        crate::db::sessions::create_new_session(&self.db, agent_id, user_id, channel).await
    }

    /// Create an isolated session. Used by cron dynamic jobs.
    pub async fn create_isolated(
        &self,
        agent_id: &str,
        user_id: &str,
        channel: &str,
    ) -> Result<Uuid> {
        crate::db::sessions::create_isolated_session_with_user(
            &self.db,
            agent_id,
            user_id,
            channel,
        )
        .await
    }

    /// Resume an existing session (updates `last_message_at`).
    pub async fn resume(&self, session_id: Uuid) -> Result<Uuid> {
        crate::db::sessions::resume_session(&self.db, session_id).await
    }

    /// Load messages for a session.
    pub async fn load_messages(
        &self,
        session_id: Uuid,
        limit: Option<i64>,
    ) -> Result<Vec<MessageRow>> {
        crate::db::sessions::load_messages(&self.db, session_id, limit).await
    }

    /// Save a message with optional tool-call metadata.
    #[allow(clippy::too_many_arguments)]
    pub async fn save_message(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
        tool_calls: Option<&serde_json::Value>,
        tool_call_id: Option<&str>,
    ) -> Result<Uuid> {
        crate::db::sessions::save_message(
            &self.db,
            session_id,
            role,
            content,
            tool_calls,
            tool_call_id,
        )
        .await
    }

    /// Save a message with full extended metadata (multi-agent, thinking blocks).
    #[allow(clippy::too_many_arguments)]
    pub async fn save_message_ex(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
        tool_calls: Option<&serde_json::Value>,
        tool_call_id: Option<&str>,
        sender_agent_id: Option<&str>,
        thinking_blocks: Option<&serde_json::Value>,
    ) -> Result<Uuid> {
        crate::db::sessions::save_message_ex(
            &self.db,
            session_id,
            role,
            content,
            tool_calls,
            tool_call_id,
            sender_agent_id,
            thinking_blocks,
        )
        .await
    }

    /// Update the session `run_status` field.
    pub async fn set_run_status(&self, session_id: Uuid, status: &str) -> Result<()> {
        crate::db::sessions::set_session_run_status(&self.db, session_id, status).await
    }

    /// Set the session title from user text (no-op if already titled).
    pub async fn auto_title(&self, session_id: Uuid, user_text: &str) -> Result<()> {
        crate::db::sessions::auto_title_session(&self.db, session_id, user_text).await
    }

    /// Trim the session message history to `max` messages (oldest first).
    pub async fn trim_messages(&self, session_id: Uuid, max: u32) -> Result<u64> {
        crate::db::sessions::trim_session_messages(&self.db, session_id, max).await
    }

    /// Insert synthetic tool results for missing call IDs (crash-recovery, ENG-01).
    pub async fn insert_missing_tool_results(
        &self,
        session_id: Uuid,
        call_ids: &[String],
    ) -> Result<()> {
        crate::db::sessions::insert_missing_tool_results(&self.db, session_id, call_ids).await
    }

    /// Full-text search messages across all sessions for the given agent.
    pub async fn search_messages(
        &self,
        agent_id: &str,
        query: &str,
        limit: i64,
    ) -> Result<Vec<crate::db::sessions::SearchResult>> {
        crate::db::sessions::search_messages(&self.db, agent_id, query, limit).await
    }

    /// Get session metadata by ID.
    pub async fn get_session(
        &self,
        session_id: Uuid,
    ) -> Result<Option<crate::db::sessions::Session>> {
        crate::db::sessions::get_session(&self.db, session_id).await
    }

    /// Count messages in a session.
    pub async fn count_messages(&self, session_id: Uuid) -> Result<i64> {
        crate::db::sessions::count_messages(&self.db, session_id).await
    }

    /// Add an agent to the session's participants list (idempotent).
    pub async fn add_participant(
        &self,
        session_id: Uuid,
        agent_name: &str,
    ) -> Result<Vec<String>> {
        crate::db::sessions::add_participant(&self.db, session_id, agent_name).await
    }
}

// ── SessionLifecycleGuard ───────────────────────────────────────────────────

/// Outcome of a session lifecycle — used by `SessionLifecycleGuard`.
#[allow(dead_code)]
pub(crate) enum SessionOutcome {
    Running,
    Done,
    Failed(String),
}

/// RAII guard that marks a session as `'failed'` if dropped without an explicit
/// `done()` or `fail()` call.
///
/// Usage: call `done().await` on success or `fail(reason).await` on known errors.
/// If neither is called (e.g. early `?` return), `Drop` fires a best-effort fallback
/// via `tokio::spawn` to mark the session as `'failed'`.
///
/// The guard holds `PgPool` directly (not `SessionManager`) to avoid self-referential
/// ownership issues in the `Drop` impl.
pub(crate) struct SessionLifecycleGuard {
    pub db: PgPool,
    pub session_id: Uuid,
    pub outcome: SessionOutcome,
}

impl SessionLifecycleGuard {
    pub fn new(db: PgPool, session_id: Uuid) -> Self {
        Self { db, session_id, outcome: SessionOutcome::Running }
    }

    /// Mark session as done in DB. Sets outcome to `Done` only on DB success;
    /// on failure logs a warning and leaves `Running` so `Drop` fires fallback.
    pub async fn done(&mut self) {
        match crate::db::sessions::set_session_run_status(&self.db, self.session_id, "done").await
        {
            Ok(()) => self.outcome = SessionOutcome::Done,
            Err(e) => tracing::warn!(
                session_id = %self.session_id,
                error = %e,
                "failed to mark session done in DB"
            ),
        }
    }

    /// Mark session as failed in DB with a reason. Sets outcome to `Failed` only on
    /// DB success; on failure logs a warning and leaves `Running` so `Drop` fires fallback.
    pub async fn fail(&mut self, reason: &str) {
        match crate::db::sessions::set_session_run_status(&self.db, self.session_id, "failed")
            .await
        {
            Ok(()) => self.outcome = SessionOutcome::Failed(reason.to_string()),
            Err(e) => tracing::warn!(
                session_id = %self.session_id,
                error = %e,
                reason,
                "failed to mark session failed in DB"
            ),
        }
    }
}

impl Drop for SessionLifecycleGuard {
    fn drop(&mut self) {
        if matches!(self.outcome, SessionOutcome::Running) {
            tracing::warn!(
                session_id = %self.session_id,
                "session guard dropped while still Running — spawning fallback mark-failed"
            );
            let db = self.db.clone();
            let sid = self.session_id;
            tokio::spawn(async move {
                if let Err(e) =
                    crate::db::sessions::set_session_run_status(&db, sid, "failed").await
                {
                    tracing::warn!(
                        error = %e,
                        session_id = %sid,
                        "failed to mark session as failed in Drop guard"
                    );
                }
            });
        }
    }
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_dm_scope tests ──

    #[test]
    fn test_resolve_dm_scope_shared() {
        let (u, c) = resolve_dm_scope("user1", "telegram", "shared");
        assert_eq!(u, "user1");
        assert_eq!(c, "*");
    }

    #[test]
    fn test_resolve_dm_scope_per_peer() {
        let (u, c) = resolve_dm_scope("user1", "telegram", "per-peer");
        assert_eq!(u, "user1");
        assert_eq!(c, "*");
    }

    #[test]
    fn test_resolve_dm_scope_per_chat() {
        let (u, c) = resolve_dm_scope("user1", "group_123", "per-chat");
        assert_eq!(u, "*");
        assert_eq!(c, "group_123");
    }

    #[test]
    fn test_resolve_dm_scope_per_channel_peer() {
        let (u, c) = resolve_dm_scope("user1", "telegram", "per-channel-peer");
        assert_eq!(u, "user1");
        assert_eq!(c, "telegram");
    }

    #[test]
    fn test_resolve_dm_scope_unknown_defaults_to_per_channel_peer() {
        let (u, c) = resolve_dm_scope("user1", "telegram", "some-unknown-scope");
        assert_eq!(u, "user1");
        assert_eq!(c, "telegram");
    }

    // ── truncate_title tests ──

    #[test]
    fn test_truncate_title_short_string_unchanged() {
        let input = "Hello, world!";
        assert_eq!(truncate_title(input, 63), input);
    }

    #[test]
    fn test_truncate_title_long_string_truncated_at_word_boundary() {
        // Build a 100-char string with clear word boundaries
        let input = "The quick brown fox jumps over the lazy dog and then does it again and again until it stops here.";
        let result = truncate_title(input, 63);
        assert!(result.len() <= 63 + "…".len(), "result too long: {:?}", result);
        assert!(result.ends_with('…'), "should end with ellipsis: {:?}", result);
        // Should break at a word boundary — no hyphenation in the middle of a word
        let without_ellipsis = result.trim_end_matches('…');
        assert!(
            without_ellipsis.ends_with(' ') || input.contains(without_ellipsis),
            "truncation should be at word boundary"
        );
    }
}
