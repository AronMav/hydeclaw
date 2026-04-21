//! Integration tests for `mark_session_run_status_if_running` — the
//! conditional status transition used on the cancel-grace path to
//! prevent the `SessionLifecycleGuard`'s `'failed'` fallback from
//! overwriting an earlier `'interrupted'` write.
//!
//! The invariant under test: a session already in a terminal state
//! (`'done'`, `'failed'`, `'interrupted'`, `'timeout'`, `'cancelled'`)
//! cannot transition to a new status via this helper. Only
//! `'running'` sessions can.
//!
//! Gated to Linux x86_64 because testcontainers requires Docker
//! (matches the pattern used by `integration_aborted_usage.rs`).

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

mod support;
use support::TestHarness;

use hydeclaw_core::db::sessions::{
    mark_session_run_status_if_running, set_session_run_status,
};
use sqlx::PgPool;
use uuid::Uuid;

async fn seed_session(pool: &PgPool, session_id: Uuid, initial_status: &str) {
    sqlx::query(
        "INSERT INTO sessions (id, agent_id, user_id, channel, title, started_at, last_message_at, run_status) \
         VALUES ($1, 'Arty', 'test-user', 'test', 'status-transition-test', NOW(), NOW(), $2) \
         ON CONFLICT (id) DO UPDATE SET run_status = EXCLUDED.run_status",
    )
    .bind(session_id)
    .bind(initial_status)
    .execute(pool)
    .await
    .expect("seed session row");
}

async fn current_status(pool: &PgPool, session_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT run_status FROM sessions WHERE id = $1")
        .bind(session_id)
        .fetch_one(pool)
        .await
        .expect("read run_status")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transitions_running_to_interrupted() {
    let harness = TestHarness::new().await.unwrap();
    let pool = harness.pool();
    let sid = Uuid::new_v4();
    seed_session(pool, sid, "running").await;

    let affected = mark_session_run_status_if_running(pool, sid, "interrupted")
        .await
        .expect("update query");
    assert_eq!(affected, 1, "expected 1 row updated on running → interrupted");
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("interrupted"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transitions_running_to_failed() {
    let harness = TestHarness::new().await.unwrap();
    let pool = harness.pool();
    // The Drop guard path.
    let sid = Uuid::new_v4();
    seed_session(pool, sid, "running").await;

    let affected = mark_session_run_status_if_running(pool, sid, "failed")
        .await
        .expect("update query");
    assert_eq!(affected, 1);
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("failed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn does_not_overwrite_done() {
    let harness = TestHarness::new().await.unwrap();
    let pool = harness.pool();
    let sid = Uuid::new_v4();
    seed_session(pool, sid, "done").await;

    let affected = mark_session_run_status_if_running(pool, sid, "failed")
        .await
        .expect("update query");
    assert_eq!(affected, 0, "must not overwrite done with failed");
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("done"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn does_not_overwrite_interrupted_with_failed() {
    let harness = TestHarness::new().await.unwrap();
    let pool = harness.pool();
    // This is the critical race the helper prevents: the chat handler
    // writes `'interrupted'` on grace-exceeded, then the engine task is
    // hard-aborted, the guard drops, and its Drop impl tries to write
    // `'failed'`. That write MUST be a no-op.
    let sid = Uuid::new_v4();
    seed_session(pool, sid, "running").await;

    // Handler writes interrupted first.
    mark_session_run_status_if_running(pool, sid, "interrupted")
        .await
        .expect("first update");
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("interrupted"));

    // Guard drop then attempts failed — must be a no-op.
    let affected = mark_session_run_status_if_running(pool, sid, "failed")
        .await
        .expect("second update");
    assert_eq!(affected, 0, "guard drop must not overwrite interrupted");
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("interrupted"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn does_not_overwrite_failed() {
    let harness = TestHarness::new().await.unwrap();
    let pool = harness.pool();
    let sid = Uuid::new_v4();
    seed_session(pool, sid, "failed").await;

    let affected = mark_session_run_status_if_running(pool, sid, "interrupted")
        .await
        .expect("update query");
    assert_eq!(affected, 0);
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("failed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn is_idempotent_for_already_terminal_sessions() {
    let harness = TestHarness::new().await.unwrap();
    let pool = harness.pool();
    // Calling the helper twice on an already-terminal session is safe.
    let sid = Uuid::new_v4();
    seed_session(pool, sid, "interrupted").await;

    for _ in 0..3 {
        let affected = mark_session_run_status_if_running(pool, sid, "failed")
            .await
            .expect("update query");
        assert_eq!(affected, 0);
    }
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("interrupted"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_session_run_status_still_overwrites_non_done() {
    let harness = TestHarness::new().await.unwrap();
    let pool = harness.pool();
    // Regression guard: the unconditional `set_session_run_status` helper
    // must still work for the happy-path `handle_with_status` flow. It
    // overwrites anything except `'done'`. We keep this coverage here so
    // the two helpers' semantics stay clearly distinguishable.
    let sid = Uuid::new_v4();
    seed_session(pool, sid, "running").await;

    set_session_run_status(pool, sid, "interrupted")
        .await
        .expect("interrupted");
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("interrupted"));

    // Unconditional helper DOES overwrite interrupted (old behavior preserved).
    set_session_run_status(pool, sid, "failed")
        .await
        .expect("failed");
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("failed"));

    // But stops at `'done'`.
    set_session_run_status(pool, sid, "done")
        .await
        .expect("done");
    set_session_run_status(pool, sid, "running")
        .await
        .expect("attempt overwrite done");
    assert_eq!(current_status(pool, sid).await.as_deref(), Some("done"));
}
