//! Pipeline step: approval resolution.
//! Extracted from engine.rs as a free function taking &CommandContext.

use super::CommandContext;
use uuid::Uuid;
use crate::agent::engine::{ApprovalResult, StreamEvent};

/// Resolve a pending approval (called from API/callback handler).
pub async fn resolve_approval(
    ctx: &CommandContext<'_>,
    approval_id: Uuid,
    approved: bool,
    resolved_by: &str,
    modified_input: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let status = if approved { "approved" } else { "rejected" };
    // Phase 63 DATA-04: switch to the transactional strict variant so we can
    // surface typed outcomes. Distinct bail! messages let `api_resolve_approval`
    // pattern-match on the anyhow root cause when deciding HTTP status.
    match crate::db::approvals::resolve_approval_strict(
        &ctx.cfg.db,
        approval_id,
        status,
        resolved_by,
    )
    .await
    {
        Ok(()) => { /* fall through to downstream audit/SSE/waiter logic */ }
        Err(crate::db::approvals::ApprovalError::NotFound { id }) => {
            anyhow::bail!("approval {id} not found");
        }
        Err(crate::db::approvals::ApprovalError::AlreadyResolved { id, status: current }) => {
            anyhow::bail!("approval {id} already resolved (status={current})");
        }
        Err(crate::db::approvals::ApprovalError::Db(e)) => {
            return Err(anyhow::Error::from(e).context("resolve_approval_strict DB error"));
        }
    }

    crate::agent::pipeline::llm_call::audit(
        ctx.cfg.db.clone(),
        ctx.cfg.agent.name.clone(),
        crate::db::audit::event_types::APPROVAL_RESOLVED,
        Some(resolved_by),
        serde_json::json!({
            "approval_id": approval_id.to_string(), "status": status
        }),
    );

    if let Some(ref tx) = ctx.state.ui_event_tx {
        tx.send(serde_json::json!({
            "type": "approval_resolved",
            "approval_id": approval_id.to_string(),
            "agent": ctx.cfg.agent.name,
            "status": status,
        }).to_string()).ok();
    }

    // Emit SSE event for inline approval resolution in chat UI
    let action_str = if approved { "approved" } else { "rejected" };
    if let Some(tx) = ctx.tex.sse_event_tx.lock().await.as_ref() {
        tx.send(StreamEvent::ApprovalResolved {
            approval_id: approval_id.to_string(),
            action: action_str.to_string(),
            modified_input: modified_input.clone(),
        }).ok();
    }

    // Wake up the waiting tool execution
    let mut waiters = ctx.cfg.approval_manager.waiters().write().await;
    if let Some((tx, _created_at)) = waiters.remove(&approval_id) {
        let result = if approved {
            match modified_input {
                Some(args) => ApprovalResult::ApprovedWithModifiedArgs(args),
                None => ApprovalResult::Approved,
            }
        } else {
            ApprovalResult::Rejected(format!("rejected by {resolved_by}"))
        };
        tx.send(result).ok();
    }

    // Opportunistic cleanup: remove stale waiters (>5 min old, oneshot already dropped)
    let stale_threshold = std::time::Duration::from_secs(300);
    waiters.retain(|id, (_tx, created_at)| {
        let stale = created_at.elapsed() > stale_threshold;
        if stale {
            tracing::debug!(approval_id = %id, "cleaning up stale approval waiter");
        }
        !stale
    });

    Ok(())
}
