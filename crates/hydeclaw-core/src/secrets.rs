// SEC-03 audit (2026-03-30): Credential leak paths verified:
// - Log statements: secret NAME only, never values (tracing grep clean)
// - Error responses: metadata only, no secret values in API responses
// - Backup export: includes decrypted secrets BY DESIGN (portability with different master key)
//   Protected by: API auth middleware + X-Confirm-Restore header on restore
// - Channel credentials: redacted from DB config, re-injected from vault on GET ?reveal=true only

use anyhow::{Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Manages encrypted secrets in `PostgreSQL` with in-memory caching.
///
/// Secrets are encrypted with ChaCha20-Poly1305 using a master key.
/// Cache key is `(name, scope)`:
///   - scope = "" means global (default, visible to all)
///   - scope = "`AgentName`" means per-agent (isolated to that agent)
///
/// Falls back to environment variables for migration convenience.
#[derive(Clone)]
pub struct SecretsManager {
    cipher: Arc<ChaCha20Poly1305>,
    db: PgPool,
    cache: Arc<RwLock<HashMap<(String, String), String>>>,
}

/// Plaintext secret for portable backup (decrypted, re-encrypted on restore).
#[derive(Debug, serde::Deserialize, Serialize)]
pub struct PlaintextSecret {
    pub name: String,
    pub scope: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct SecretInfo {
    pub name: String,
    pub scope: String,
    pub description: Option<String>,
    pub has_value: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(FromRow)]
struct SecretInfoRow {
    name: String,
    scope: String,
    description: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl SecretsManager {
    /// Create a new `SecretsManager`.
    ///
    /// `master_key_hex` must be exactly 64 hex characters (32 bytes).
    pub fn new(master_key_hex: &str, db: PgPool) -> Result<Self> {
        let key_bytes =
            hex::decode(master_key_hex).context("master key is not valid hex")?;
        if key_bytes.len() != 32 {
            anyhow::bail!(
                "master key must be 32 bytes (64 hex chars), got {}",
                key_bytes.len()
            );
        }
        let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
            .map_err(|e| anyhow::anyhow!("failed to create cipher: {e}"))?;

        Ok(Self {
            cipher: Arc::new(cipher),
            db,
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create a no-op SecretsManager for unit tests (never resolves any secrets).
    #[cfg(test)]
    pub fn new_noop() -> Self {
        let key = [0u8; 32];
        let cipher = ChaCha20Poly1305::new_from_slice(&key).expect("32-byte zero key");
        let db = PgPool::connect_lazy("postgres://invalid").expect("lazy pool");
        Self {
            cipher: Arc::new(cipher),
            db,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all secrets from DB into cache. Called once at startup.
    pub async fn load_all(&self) -> Result<usize> {
        let rows: Vec<(String, String, Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT name, scope, encrypted_value, nonce FROM secrets",
        )
        .fetch_all(&self.db)
        .await
        .context("failed to load secrets from DB")?;

        let mut cache = self.cache.write().await;
        let mut count = 0;
        for (name, scope, encrypted, nonce_bytes) in rows {
            if nonce_bytes.len() != 12 {
                tracing::error!(secret = %name, scope = %scope, "invalid nonce length, skipping");
                continue;
            }
            let nonce = Nonce::from_slice(&nonce_bytes);
            match self.cipher.decrypt(nonce, encrypted.as_ref()) {
                Ok(plaintext) => match String::from_utf8(plaintext) {
                    Ok(value) => {
                        cache.insert((name, scope), value);
                        count += 1;
                    }
                    Err(e) => {
                        tracing::error!(secret = %name, error = %e, "secret is not valid UTF-8");
                    }
                },
                Err(e) => {
                    tracing::error!(secret = %name, error = %e, "failed to decrypt secret (wrong master key?)");
                }
            }
        }
        Ok(count)
    }

    /// Get a global secret value from cache, falling back to env var.
    pub async fn get(&self, name: &str) -> Option<String> {
        let cache = self.cache.read().await;
        if let Some(val) = cache.get(&(name.to_string(), String::new())) {
            return Some(val.clone());
        }
        drop(cache);
        std::env::var(name).ok()
    }

    /// Get a secret with per-agent scope fallback chain:
    ///   1. (name, scope) — agent-specific secret
    ///   2. (name, "")   — global fallback
    ///   3. env var       — legacy env fallback
    pub async fn get_scoped(&self, name: &str, scope: &str) -> Option<String> {
        let cache = self.cache.read().await;
        if !scope.is_empty() {
            if let Some(val) = cache.get(&(name.to_string(), scope.to_string())) {
                return Some(val.clone());
            }
        }
        if let Some(val) = cache.get(&(name.to_string(), String::new())) {
            if !scope.is_empty() {
                tracing::debug!(secret = %name, scope = %scope, "scoped secret not found, using global fallback");
            }
            return Some(val.clone());
        }
        drop(cache);
        if let Ok(val) = std::env::var(name) {
            if !scope.is_empty() {
                tracing::warn!(secret = %name, scope = %scope, "secret resolved from env var — consider migrating to vault");
            }
            return Some(val);
        }
        None
    }

    /// Get a global secret value from cache only (no env fallback).
    pub async fn get_strict(&self, name: &str) -> Option<String> {
        self.cache.read().await.get(&(name.to_string(), String::new())).cloned()
    }

    /// Export all secrets as raw encrypted blobs (for backup).
    /// Export secrets as decrypted plaintext (for portable backups).
    /// The caller is responsible for protecting the output.
    pub async fn export_decrypted(&self) -> Result<Vec<PlaintextSecret>> {
        let cache = self.cache.read().await;
        Ok(cache
            .iter()
            .map(|((name, scope), value)| PlaintextSecret {
                name: name.clone(),
                scope: scope.clone(),
                value: value.clone(),
            })
            .collect())
    }

    /// Restore secrets from plaintext (encrypts with current master key).
    /// Upserts by (name, scope) and reloads the in-memory cache.
    pub async fn restore_plaintext(&self, secrets: Vec<PlaintextSecret>) -> Result<usize> {
        let count = secrets.len();
        for s in secrets {
            self.set_internal(&s.name, &s.scope, &s.value, None).await?;
        }
        Ok(count)
    }

    /// Set (upsert) a global secret (scope = "").
    /// Encrypts, stores in DB, updates cache.
    pub async fn set(
        &self,
        name: &str,
        value: &str,
        description: Option<&str>,
    ) -> Result<()> {
        self.set_internal(name, "", value, description).await
    }

    /// Set (upsert) a per-agent scoped secret.
    /// Encrypts, stores in DB, updates cache.
    #[allow(dead_code)]
    pub async fn set_scoped(
        &self,
        name: &str,
        scope: &str,
        value: &str,
        description: Option<&str>,
    ) -> Result<()> {
        self.set_internal(name, scope, value, description).await
    }

    async fn set_internal(
        &self,
        name: &str,
        scope: &str,
        value: &str,
        description: Option<&str>,
    ) -> Result<()> {
        let mut nonce_bytes = [0u8; 12];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(nonce, value.as_bytes())
            .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

        // Hold write lock across DB + cache to ensure atomicity.
        // The DB upsert is fast (single row), so lock contention is minimal.
        // Readers now use block_in_place to avoid blocking tokio threads.
        let mut cache = self.cache.write().await;
        sqlx::query(
            "INSERT INTO secrets (name, scope, encrypted_value, nonce, description, updated_at)
             VALUES ($1, $2, $3, $4, $5, now())
             ON CONFLICT (name, scope) DO UPDATE SET
               encrypted_value = EXCLUDED.encrypted_value,
               nonce = EXCLUDED.nonce,
               description = COALESCE(EXCLUDED.description, secrets.description),
               updated_at = now()",
        )
        .bind(name)
        .bind(scope)
        .bind(&ciphertext)
        .bind(&nonce_bytes[..])
        .bind(description)
        .execute(&self.db)
        .await
        .context("failed to store secret in DB")?;
        cache.insert((name.to_string(), scope.to_string()), value.to_string());
        drop(cache);

        if scope.is_empty() {
            tracing::info!(secret = %name, "secret updated");
        } else {
            tracing::info!(secret = %name, scope = %scope, "scoped secret updated");
        }
        Ok(())
    }

    /// Update only the description of an existing secret (no value change).
    pub async fn update_description(&self, name: &str, scope: &str, description: Option<&str>) -> Result<()> {
        sqlx::query(
            "UPDATE secrets SET description = $3, updated_at = now() WHERE name = $1 AND scope = $2",
        )
        .bind(name)
        .bind(scope)
        .bind(description)
        .execute(&self.db)
        .await
        .context("failed to update secret description")?;
        tracing::info!(secret = %name, "secret description updated");
        Ok(())
    }


    /// Delete a scoped secret from DB and cache. Returns true if it existed.
    pub async fn delete_scoped(&self, name: &str, scope: &str) -> Result<bool> {
        let mut cache = self.cache.write().await;

        let result = sqlx::query("DELETE FROM secrets WHERE name = $1 AND scope = $2")
            .bind(name)
            .bind(scope)
            .execute(&self.db)
            .await
            .context("failed to delete scoped secret from DB")?;

        cache.remove(&(name.to_string(), scope.to_string()));
        drop(cache);

        let deleted = result.rows_affected() > 0;
        if deleted {
            tracing::info!(secret = %name, scope = %scope, "scoped secret deleted");
        }
        Ok(deleted)
    }

    /// List all global (scope = "") secret names with metadata (without values).
    pub async fn list(&self) -> Result<Vec<SecretInfo>> {
        let cache = self.cache.read().await;
        let rows: Vec<SecretInfoRow> = sqlx::query_as(
            "SELECT name, scope, description, created_at, updated_at FROM secrets \
             WHERE name NOT IN ('CHANNEL_CREDENTIALS', 'PROVIDER_CREDENTIALS') \
             ORDER BY scope, name",
        )
        .fetch_all(&self.db)
        .await
        .context("failed to list secrets")?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let has_value = cache
                    .get(&(r.name.clone(), r.scope.clone()))
                    .is_some_and(|v| !v.is_empty());
                SecretInfo {
                    name: r.name,
                    scope: r.scope,
                    description: r.description,
                    has_value,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                }
            })
            .collect())
    }

    /// Rename all secrets belonging to `old_scope` → `new_scope`.
    /// Called when an agent is renamed to migrate its scoped secrets atomically.
    /// Returns the number of secrets migrated.
    pub async fn rename_scope(&self, old_scope: &str, new_scope: &str) -> Result<u64> {
        let mut cache = self.cache.write().await;

        let affected = sqlx::query("UPDATE secrets SET scope = $1 WHERE scope = $2")
            .bind(new_scope)
            .bind(old_scope)
            .execute(&self.db)
            .await
            .context("failed to rename secret scope in DB")?
            .rows_affected();

        // Re-key cache: (name, old_scope) → (name, new_scope)
        let old_keys: Vec<String> = cache
            .keys()
            .filter(|(_, s)| s == old_scope)
            .map(|(n, _)| n.clone())
            .collect();

        for name in old_keys {
            if let Some(value) = cache.remove(&(name.clone(), old_scope.to_string())) {
                cache.insert((name, new_scope.to_string()), value);
            }
        }
        drop(cache);

        tracing::info!(from = %old_scope, to = %new_scope, count = affected, "renamed secret scope");
        Ok(affected)
    }

    /// Delete all secrets belonging to a scope.
    /// Called when an agent is deleted to clean up its scoped secrets.
    pub async fn delete_scope(&self, scope: &str) -> Result<()> {
        sqlx::query("DELETE FROM secrets WHERE scope = $1")
            .bind(scope)
            .execute(&self.db)
            .await
            .context("failed to delete secrets for scope")?;

        // Remove from cache
        let mut cache = self.cache.write().await;
        cache.retain(|(_, s), _| s != scope);
        drop(cache);

        tracing::info!(scope = %scope, "deleted all secrets for scope");
        Ok(())
    }
}
