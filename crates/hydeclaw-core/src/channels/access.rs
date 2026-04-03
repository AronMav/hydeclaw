use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rand::Rng;
use sqlx::PgPool;
use tokio::sync::RwLock;

use crate::db::access;

struct PairingEntry {
    channel_user_id: String,
    display_name: Option<String>,
    created_at: DateTime<Utc>,
}

/// Manages access control for a channel bot.
pub struct AccessGuard {
    pub agent_id: String,
    pub(crate) owner_id: Option<String>,
    pub restricted: bool,
    pub(crate) db: PgPool,
    /// Pending pairing codes: code -> PairingEntry
    pending_pairings: Arc<RwLock<HashMap<String, PairingEntry>>>,
}

impl AccessGuard {
    pub fn new(
        agent_id: String,
        owner_id: Option<String>,
        restricted: bool,
        db: PgPool,
    ) -> Self {
        Self {
            agent_id,
            owner_id,
            restricted,
            db,
            pending_pairings: Arc::new(RwLock::new(HashMap::new())),
        }
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

    /// Generate a 6-digit pairing code for an unknown user.
    pub async fn create_pairing_code(
        &self,
        channel_user_id: &str,
        display_name: Option<&str>,
    ) -> String {
        let code: String = format!("{:06}", rand::rng().random_range(0..1_000_000u32));

        let entry = PairingEntry {
            channel_user_id: channel_user_id.to_string(),
            display_name: display_name.map(|s| s.to_string()),
            created_at: Utc::now(),
        };

        let mut pairings = self.pending_pairings.write().await;
        // Remove any existing code for this user (re-pairing)
        pairings.retain(|_, e| e.channel_user_id != channel_user_id);
        pairings.insert(code.clone(), entry);

        code
    }

    /// Try to approve a pairing by code.
    /// Returns (success, user_display_info).
    pub async fn approve_pairing(&self, code: &str, approver_id: &str) -> (bool, String) {
        let mut pairings = self.pending_pairings.write().await;

        if let Some(entry) = pairings.remove(code) {
            // Check expiration (5 minutes)
            let elapsed = Utc::now() - entry.created_at;
            if elapsed.num_seconds() > 300 {
                return (false, "expired".to_string());
            }

            let display = entry
                .display_name
                .clone()
                .unwrap_or_else(|| entry.channel_user_id.clone());

            if let Err(e) = access::add_allowed_user(
                &self.db,
                &self.agent_id,
                &entry.channel_user_id,
                entry.display_name.as_deref(),
                approver_id,
            )
            .await
            {
                tracing::error!(error = %e, "failed to add allowed user");
                return (false, display);
            }

            (true, display)
        } else {
            (false, "not_found".to_string())
        }
    }

    /// Reject a pending pairing by code.
    pub async fn reject_pairing(&self, code: &str) -> bool {
        self.pending_pairings.write().await.remove(code).is_some()
    }

    /// List all pending pairing codes with user info (for UI display).
    pub async fn pending_pairings_list(&self) -> Vec<serde_json::Value> {
        let pairings = self.pending_pairings.read().await;
        pairings
            .iter()
            .map(|(code, entry)| {
                serde_json::json!({
                    "code": code,
                    "channel_user_id": entry.channel_user_id,
                    "display_name": entry.display_name,
                    "created_at": entry.created_at.to_rfc3339(),
                })
            })
            .collect()
    }

}
