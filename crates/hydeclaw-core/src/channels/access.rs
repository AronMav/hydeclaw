use rand::Rng;
use sqlx::PgPool;

use crate::db::access;

/// Manages access control for a channel bot.
/// Pairing codes are stored in PostgreSQL (survive restarts).
pub struct AccessGuard {
    pub agent_id: String,
    pub(crate) owner_id: Option<String>,
    pub restricted: bool,
    pub(crate) db: PgPool,
}

impl AccessGuard {
    pub fn new(
        agent_id: String,
        owner_id: Option<String>,
        restricted: bool,
        db: PgPool,
    ) -> Self {
        Self { agent_id, owner_id, restricted, db }
    }

    /// Check if a user is allowed to use this bot.
    pub async fn is_allowed(&self, channel_user_id: &str) -> bool {
        if !self.restricted {
            return true;
        }
        if self.is_owner(channel_user_id) {
            return true;
        }
        access::is_user_allowed(&self.db, &self.agent_id, channel_user_id)
            .await
            .unwrap_or(false)
    }

    /// Check if a user is the owner.
    pub fn is_owner(&self, channel_user_id: &str) -> bool {
        self.owner_id.as_deref() == Some(channel_user_id)
    }

    /// Generate a 6-digit pairing code for an unknown user (persisted in DB).
    pub async fn create_pairing_code(
        &self,
        channel_user_id: &str,
        display_name: Option<&str>,
    ) -> String {
        let code = format!("{:06}", rand::rng().random_range(0..1_000_000u32));
        if let Err(e) = access::store_pairing_code(
            &self.db, &self.agent_id, &code, channel_user_id, display_name,
        ).await {
            tracing::error!(error = %e, "failed to store pairing code in DB");
        }
        code
    }

    /// Try to approve a pairing by code.
    /// Returns (success, user_display_info).
    pub async fn approve_pairing(&self, code: &str, approver_id: &str) -> (bool, String) {
        match access::take_pairing_code(&self.db, &self.agent_id, code).await {
            Ok(Some((user_id, name, false))) => {
                let display = name.clone().unwrap_or_else(|| user_id.clone());
                if let Err(e) = access::add_allowed_user(
                    &self.db, &self.agent_id, &user_id, name.as_deref(), approver_id,
                ).await {
                    tracing::error!(error = %e, "failed to add allowed user");
                    return (false, display);
                }
                (true, display)
            }
            Ok(Some((_, _, true))) => (false, "expired".to_string()),
            Ok(None) => (false, "not_found".to_string()),
            Err(e) => {
                tracing::error!(error = %e, "failed to take pairing code from DB");
                (false, "db_error".to_string())
            }
        }
    }

    /// Reject a pending pairing by code.
    pub async fn reject_pairing(&self, code: &str) -> bool {
        access::remove_pairing_code(&self.db, &self.agent_id, code)
            .await
            .unwrap_or(false)
    }

    /// List all pending pairing codes with user info (for UI display).
    pub async fn pending_pairings_list(&self) -> Vec<serde_json::Value> {
        match access::list_pairing_codes(&self.db, &self.agent_id).await {
            Ok(codes) => codes.iter().map(|p| {
                serde_json::json!({
                    "code": p.code,
                    "channel_user_id": p.channel_user_id,
                    "display_name": p.display_name,
                    "created_at": p.created_at.to_rfc3339(),
                })
            }).collect(),
            Err(e) => {
                tracing::error!(error = %e, "failed to list pairing codes");
                vec![]
            }
        }
    }
}
