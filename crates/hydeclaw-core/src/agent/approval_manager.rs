//! Approval workflow manager: check/create/wait/cleanup for tool-call approvals.
//! Extracted from `engine_dispatch.rs` for readability and encapsulation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::PgPool;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use super::channel_actions::{ChannelAction, ChannelActionRouter};
use super::engine::{ApprovalResult, StreamEvent};

// ── Types ────────────────────────────────────────────────────────────────────

/// Map of pending approval waiters: approval_id → (oneshot sender, creation time).
pub(crate) type ApprovalWaitersMap = Arc<
    RwLock<
        HashMap<Uuid, (tokio::sync::oneshot::Sender<ApprovalResult>, Instant)>,
    >,
>;

/// Outcome of `request_approval`: tells the caller how to proceed.
#[derive(Debug)]
pub(crate) enum ApprovalOutcome {
    /// Tool was approved — execute with original arguments.
    Approved,
    /// Tool was approved with modified arguments — caller should re-dispatch.
    ApprovedWithModifiedArgs(serde_json::Value),
    /// Tool was rejected by the user.
    Rejected(String),
    /// Approval was cancelled (sender dropped).
    Cancelled,
    /// Approval timed out.
    TimedOut { timeout_secs: u64 },
}

// ── ApprovalManager ──────────────────────────────────────────────────────────

/// Encapsulates the full approval lifecycle: DB record, channel notification,
/// UI broadcast, waiter management, and timeout handling.
pub(crate) struct ApprovalManager {
    db: PgPool,
    waiters: ApprovalWaitersMap,
}

impl ApprovalManager {
    pub(crate) fn new(db: PgPool, waiters: ApprovalWaitersMap) -> Self {
        Self { db, waiters }
    }

    /// Shared waiters map — needed by `resolve_approval` on `AgentEngine`.
    pub(crate) fn waiters(&self) -> &ApprovalWaitersMap {
        &self.waiters
    }

    /// Request approval for a tool call. Blocks until approved, rejected, or timed out.
    ///
    /// Steps:
    /// 1. Create DB approval record
    /// 2. Audit + broadcast UI event
    /// 3. Send approval request via channel router (if available)
    /// 4. Emit SSE `ApprovalNeeded` event
    /// 5. Wait with timeout
    /// 6. Clean up waiter on completion/timeout/error
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn request_approval(
        &self,
        agent_name: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
        context: &serde_json::Value,
        timeout_secs: u64,
        channel_router: Option<&ChannelActionRouter>,
        ui_event_tx: Option<&tokio::sync::broadcast::Sender<String>>,
        sse_event_tx: &Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<StreamEvent>>>>,
    ) -> ApprovalOutcome {
        let session_id = context
            .get("session_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok());

        // 1. Create DB record
        let approval_id = match crate::db::approvals::create_approval(
            &self.db,
            agent_name,
            session_id,
            tool_name,
            arguments,
            context,
        )
        .await
        {
            Ok(id) => {
                crate::db::audit::audit_spawn(
                    self.db.clone(),
                    agent_name.to_string(),
                    crate::db::audit::event_types::APPROVAL_REQUESTED,
                    None,
                    serde_json::json!({
                        "tool": tool_name, "approval_id": id.to_string()
                    }),
                );
                Self::broadcast_ui(ui_event_tx, serde_json::json!({
                    "type": "approval_requested",
                    "approval_id": id.to_string(),
                    "agent": agent_name,
                    "tool_name": tool_name,
                }));
                // Fire-and-forget notification
                if let Some(ui_tx) = ui_event_tx {
                    let db = self.db.clone();
                    let tx = ui_tx.clone();
                    let tool_name_owned = tool_name.to_string();
                    let agent_name_owned = agent_name.to_string();
                    let approval_id_str = id.to_string();
                    tokio::spawn(async move {
                        crate::gateway::notify(
                            &db,
                            &tx,
                            "tool_approval",
                            "Tool Approval Required",
                            &format!(
                                "Agent {} is requesting approval to use tool: {}",
                                agent_name_owned, tool_name_owned
                            ),
                            serde_json::json!({
                                "agent": agent_name_owned,
                                "tool_name": tool_name_owned,
                                "approval_id": approval_id_str,
                            }),
                        )
                        .await
                        .ok();
                    });
                }
                id
            }
            Err(e) => return ApprovalOutcome::Rejected(format!("Error creating approval: {}", e)),
        };

        // 2. Send approval request via channel adapter
        let clean_args = {
            let mut args_clone = arguments.clone();
            if let Some(obj) = args_clone.as_object_mut() {
                obj.remove("_context");
            }
            args_clone
        };

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let action = ChannelAction {
            name: "approval_request".to_string(),
            params: serde_json::json!({
                "tool_name": tool_name,
                "args": clean_args,
                "approval_id": approval_id.to_string(),
            }),
            context: context.clone(),
            reply: reply_tx,
            target_channel: None,
        };
        if let Some(router) = channel_router {
            if let Err(e) = router.send(action).await {
                tracing::error!(
                    approval_id = %approval_id,
                    error = %e,
                    "failed to send approval_request to channel"
                );
            }
            tokio::time::timeout(Duration::from_secs(5), reply_rx)
                .await
                .ok();
        } else {
            tracing::warn!(
                tool = %tool_name,
                "no channel_router — cannot send approval buttons"
            );
        }

        // 3. Create oneshot waiter and insert into map
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        {
            let mut waiters = self.waiters.write().await;
            // Opportunistic cleanup: remove expired entries (>5 min).
            let cutoff = Instant::now() - Duration::from_secs(300);
            waiters.retain(|_, (_, created_at)| *created_at > cutoff);
            waiters.insert(approval_id, (result_tx, Instant::now()));
        }

        // 4. Emit SSE event for inline approval in chat UI
        if let Some(tx) = sse_event_tx.lock().await.as_ref() {
            let clean_input = {
                let mut args_clone = arguments.clone();
                if let Some(obj) = args_clone.as_object_mut() {
                    obj.remove("_context");
                }
                args_clone
            };
            tx.send(StreamEvent::ApprovalNeeded {
                approval_id: approval_id.to_string(),
                tool_name: tool_name.to_string(),
                tool_input: clean_input,
                timeout_ms: timeout_secs * 1000,
            })
            .ok();
        }

        // 5. Wait for approval with timeout
        match tokio::time::timeout(Duration::from_secs(timeout_secs), result_rx).await {
            Ok(Ok(ApprovalResult::Approved)) => {
                tracing::info!(tool = %tool_name, approval_id = %approval_id, "tool approved");
                ApprovalOutcome::Approved
            }
            Ok(Ok(ApprovalResult::ApprovedWithModifiedArgs(modified_args))) => {
                tracing::info!(
                    tool = %tool_name,
                    approval_id = %approval_id,
                    "tool approved with modified args"
                );
                ApprovalOutcome::ApprovedWithModifiedArgs(modified_args)
            }
            Ok(Ok(ApprovalResult::Rejected(reason))) => {
                ApprovalOutcome::Rejected(format!("Tool `{}` was rejected: {}", tool_name, reason))
            }
            Ok(Err(_)) => {
                // Sender dropped (pruned or retain cleanup) — resolve DB record
                {
                    let mut waiters = self.waiters.write().await;
                    waiters.remove(&approval_id);
                }
                let _ = crate::db::approvals::resolve_approval(
                    &self.db, approval_id, "cancelled", "system",
                ).await;
                ApprovalOutcome::Cancelled
            }
            Err(_) => {
                // Timeout — attempt to mark as timed out in DB.
                let was_pending = crate::db::approvals::resolve_approval(
                    &self.db,
                    approval_id,
                    "timeout",
                    "system",
                )
                .await
                .unwrap_or(false);

                // Release waiters lock BEFORE acquiring sse_event_tx (prevents deadlock
                // with resolve_approval which acquires locks in opposite order)
                {
                    let mut waiters = self.waiters.write().await;
                    waiters.remove(&approval_id);
                }

                // If timeout raced with approval (DB already resolved), log it
                if !was_pending {
                    tracing::warn!(
                        tool = %tool_name,
                        approval_id = %approval_id,
                        "approval timeout raced with resolution — timeout takes precedence"
                    );
                }

                // Emit SSE event for timeout (lock-safe: waiters already released)
                if let Some(tx) = sse_event_tx.lock().await.as_ref() {
                    tx.send(StreamEvent::ApprovalResolved {
                        approval_id: approval_id.to_string(),
                        action: "timeout_rejected".to_string(),
                        modified_input: None,
                    })
                    .ok();
                }

                ApprovalOutcome::TimedOut { timeout_secs }
            }
        }
    }

    /// Evict stale approval waiters (older than 10 minutes).
    pub(crate) async fn prune_stale(&self) {
        let mut waiters = self.waiters.write().await;
        let now = Instant::now();
        waiters.retain(|id, (_, created)| {
            let stale = now.duration_since(*created) > Duration::from_secs(600);
            if stale {
                tracing::debug!(approval_id = %id, "evicting stale approval waiter (>10min)");
            }
            !stale
        });
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn broadcast_ui(
        ui_event_tx: Option<&tokio::sync::broadcast::Sender<String>>,
        event: serde_json::Value,
    ) {
        if let Some(tx) = ui_event_tx {
            tx.send(event.to_string()).ok();
        }
    }
}
