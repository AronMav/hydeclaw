//! Startup-time TOML migrator (Tasks 20 + 21).
//!
//! Rewrites legacy inline-routing `config/agents/*.toml` files to use
//! `connection = "<name>"` references instead of inline `provider` /
//! `base_url` / `api_key_env` fields on each `[[agent.routing]]` entry.
//!
//! Guarantees:
//! - Atomic write via sibling tempfile + fsync + rename
//! - Backup to `config/agents/.backup/` (timestamped copy) before rewrite
//! - Per-agent marker file `config/agents/.migrated/{name}.v020.ok` that
//!   short-circuits subsequent startups (idempotent across restarts)
//! - Stale tempfile cleanup on startup (recovery from crash mid-rename)
//! - Fail-loud when a legacy route references a provider that has no
//!   matching row in the `providers` table — operators must create the
//!   connection first.
//!
//! Runs once at startup, after DB migrations and before agent engines spawn.

use std::path::Path;

use crate::db::providers::ProviderRow;

/// Marker version — bump this when the migrator contract changes to force a
/// re-run against previously migrated TOMLs.
const MARKER_VERSION: &str = "v020";

/// Inline legacy keys that must be stripped once a route references a
/// named connection. Kept in one place so the rewrite logic and tests stay
/// in lock-step.
const LEGACY_ROUTE_KEYS: &[&str] = &[
    "provider",
    "base_url",
    "api_key_env",
    "api_key_envs",
    "prompt_cache",
    "max_tokens",
];

pub struct TomlMigrator<'a> {
    /// Path to the `config/agents/` directory.
    pub config_dir: &'a Path,
    /// Snapshot of the `providers` table taken once at startup.
    pub db_providers: &'a [ProviderRow],
}

impl<'a> TomlMigrator<'a> {
    /// Run the migrator. Migrates each `*.toml` in `config_dir` that hasn't
    /// been migrated yet (marker file absent). Idempotent across restarts.
    pub async fn migrate_all(&self) -> anyhow::Result<()> {
        // Nothing to do if the agents dir doesn't exist yet (fresh install).
        if !self.config_dir.exists() {
            return Ok(());
        }

        // Preflight: create .backup and .migrated directories.
        let backup_dir = self.config_dir.join(".backup");
        let marker_dir = self.config_dir.join(".migrated");
        std::fs::create_dir_all(&backup_dir)
            .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", backup_dir.display()))?;
        std::fs::create_dir_all(&marker_dir)
            .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", marker_dir.display()))?;

        // Clean up stale `*.toml.tmp-*` files (crash recovery).
        if let Ok(entries) = std::fs::read_dir(self.config_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.contains(".toml.tmp-") {
                    let path = entry.path();
                    match std::fs::remove_file(&path) {
                        Ok(()) => tracing::warn!(
                            path = %path.display(),
                            "removed stale TOML temp file (previous migration crashed)"
                        ),
                        Err(e) => tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "failed to remove stale TOML temp file"
                        ),
                    }
                }
            }
        }

        // Migrate each agent TOML (skip dotfiles and non-TOML entries).
        for entry in std::fs::read_dir(self.config_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with('.')
            {
                continue;
            }
            self.migrate_one(&path, &backup_dir, &marker_dir).await?;
        }
        Ok(())
    }

    async fn migrate_one(
        &self,
        path: &Path,
        backup_dir: &Path,
        marker_dir: &Path,
    ) -> anyhow::Result<()> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid TOML filename: {}", path.display()))?
            .to_string();

        // Skip if already migrated.
        let marker = marker_dir.join(format!("{name}.{MARKER_VERSION}.ok"));
        if marker.exists() {
            return Ok(());
        }

        // 1. Read original.
        let original = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;

        // 2. Parse + check if any route needs rewriting.
        let mut doc: toml::Value = toml::from_str(&original)
            .map_err(|e| anyhow::anyhow!("parse {}: {e}", path.display()))?;
        let changed = self.rewrite_routes(&name, &mut doc)?;

        if !changed {
            // Nothing to rewrite — still mark so we don't re-parse on every boot.
            let _ = std::fs::write(&marker, chrono::Utc::now().to_rfc3339());
            return Ok(());
        }

        // 3. Backup (copy, not move).
        let backup_file = backup_dir.join(format!(
            "{name}.{}.toml",
            chrono::Utc::now().format("%Y%m%d%H%M%S")
        ));
        std::fs::copy(path, &backup_file)
            .map_err(|e| anyhow::anyhow!("backup {} -> {}: {e}",
                path.display(), backup_file.display()))?;

        // 4. Render new TOML.
        let rendered = toml::to_string_pretty(&doc)
            .map_err(|e| anyhow::anyhow!("serialize {}: {e}", path.display()))?;

        // 5. Atomic write via sibling tempfile + fsync.
        let tmp = path.with_extension(format!("toml.tmp-{}", std::process::id()));
        std::fs::write(&tmp, &rendered)
            .map_err(|e| anyhow::anyhow!("write {}: {e}", tmp.display()))?;
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }

        // 6. Atomic rename.
        std::fs::rename(&tmp, path)
            .map_err(|e| anyhow::anyhow!("rename {} -> {}: {e}",
                tmp.display(), path.display()))?;

        // 7. Fsync parent directory (POSIX only — NTFS doesn't support it this way).
        #[cfg(unix)]
        if let Some(parent) = path.parent()
            && let Ok(f) = std::fs::File::open(parent)
        {
            let _ = f.sync_all();
        }

        // 8. Write marker.
        let _ = std::fs::write(&marker, chrono::Utc::now().to_rfc3339());

        tracing::info!(
            agent = %name,
            backup = %backup_file.display(),
            "migrated agent TOML routing to connection references"
        );
        Ok(())
    }

    /// Walk the agent TOML and rewrite every legacy `[[agent.routing]]`
    /// entry to use `connection` + strip legacy inline fields.
    ///
    /// Returns `Ok(true)` if the document was mutated, `Ok(false)` if no
    /// changes were needed. Errors when a legacy route references a
    /// provider that has no matching row in `db_providers`.
    fn rewrite_routes(&self, agent: &str, doc: &mut toml::Value) -> anyhow::Result<bool> {
        // Routing lives under `[agent].routing` (the `AgentConfig` wrapper
        // serializes `AgentSettings` as the top-level `[agent]` table).
        let Some(agent_tbl) = doc.get_mut("agent").and_then(|v| v.as_table_mut()) else {
            return Ok(false);
        };
        let Some(routes) = agent_tbl.get_mut("routing").and_then(|v| v.as_array_mut()) else {
            return Ok(false);
        };

        let mut changed = false;

        for (i, route) in routes.iter_mut().enumerate() {
            let Some(tbl) = route.as_table_mut() else { continue };

            // Already migrated — uses `connection` and has no legacy `provider` field.
            let has_connection = tbl.get("connection")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty());
            let has_legacy = LEGACY_ROUTE_KEYS.iter().any(|k| tbl.contains_key(*k));
            if has_connection && !has_legacy {
                continue;
            }

            // Match DB provider by (provider_type, base_url).
            let ptype = tbl
                .get("provider")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let base_url = tbl
                .get("base_url")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            // If the route already has a `connection` field but also has
            // legacy inline keys, trust the existing connection and just
            // strip the noise. Otherwise resolve via DB lookup.
            let connection_name: String = if has_connection {
                tbl.get("connection")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            } else {
                let matched = self.db_providers.iter().find(|r| {
                    ptype.as_deref() == Some(r.provider_type.as_str())
                        && r.base_url.as_deref() == base_url.as_deref()
                });
                match matched {
                    Some(r) => r.name.clone(),
                    None => {
                        anyhow::bail!(
                            "agent={agent} route[{i}] references inline provider \
                             ({:?}, {:?}) that has no matching DB provider row; \
                             operator must create the connection first",
                            ptype,
                            base_url
                        );
                    }
                }
            };

            tbl.insert(
                "connection".into(),
                toml::Value::String(connection_name),
            );

            // Strip legacy inline fields. Inserting `connection` above
            // already counts as a change, so we don't track per-key removals.
            for key in LEGACY_ROUTE_KEYS {
                tbl.remove(*key);
            }
            changed = true;
        }

        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fake_row(name: &str, ptype: &str, base_url: &str) -> ProviderRow {
        ProviderRow {
            id: uuid::Uuid::new_v4(),
            name: name.into(),
            category: "llm".into(),
            provider_type: ptype.into(),
            base_url: Some(base_url.into()),
            default_model: None,
            enabled: true,
            options: serde_json::json!({"timeouts": {}}),
            notes: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn rewrites_inline_route_to_connection_reference() {
        let dir = tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let file = agents.join("Arty.toml");
        std::fs::write(
            &file,
            r#"
[agent]
name = "Arty"
language = "ru"
provider = "ollama"
model = "minimax-m2.7"

[[agent.routing]]
condition = "default"
provider = "ollama"
model = "minimax-m2.7"
base_url = "http://localhost:11434"
cooldown_secs = 60
"#,
        )
        .unwrap();

        let rows = vec![fake_row("ollama-auto", "ollama", "http://localhost:11434")];
        let migrator = TomlMigrator {
            config_dir: &agents,
            db_providers: &rows,
        };
        migrator.migrate_all().await.unwrap();

        let content = std::fs::read_to_string(&file).unwrap();
        assert!(
            content.contains("connection = \"ollama-auto\""),
            "got:\n{content}"
        );
        assert!(
            !content.contains("base_url ="),
            "still has inline base_url:\n{content}"
        );
        // Legacy inline provider / api_key_env must be gone from the route.
        // (The top-level agent.provider field is unrelated and stays.)
        let rendered_routing = content
            .split("[[agent.routing]]")
            .nth(1)
            .unwrap_or_default();
        assert!(
            !rendered_routing.contains("api_key_env"),
            "route retains legacy api_key_env:\n{rendered_routing}"
        );
    }

    #[tokio::test]
    async fn atomic_rename_leaves_no_tmp_files() {
        let dir = tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("A.toml"),
            "[agent]\nname = \"A\"\nprovider = \"openai\"\nmodel = \"m\"\n",
        )
        .unwrap();

        let migrator = TomlMigrator {
            config_dir: &agents,
            db_providers: &[],
        };
        migrator.migrate_all().await.unwrap();

        for entry in std::fs::read_dir(&agents).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!name.contains(".toml.tmp-"), "leftover tmp: {name}");
        }
    }

    #[tokio::test]
    async fn marker_prevents_re_run() {
        let dir = tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let file = agents.join("X.toml");
        std::fs::write(
            &file,
            r#"
[agent]
name = "X"
provider = "openai"
model = "m"

[[agent.routing]]
condition = "default"
provider = "ollama"
base_url = "http://a"
cooldown_secs = 60
"#,
        )
        .unwrap();

        let rows = vec![fake_row("ol", "ollama", "http://a")];
        let migrator = TomlMigrator {
            config_dir: &agents,
            db_providers: &rows,
        };
        migrator.migrate_all().await.unwrap();

        // Break the file deliberately. If migrator runs again, it would rewrite.
        std::fs::write(&file, "garbage that is not valid toml").unwrap();
        migrator.migrate_all().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "garbage that is not valid toml"
        );
    }

    #[tokio::test]
    async fn stale_tmp_files_are_cleaned() {
        let dir = tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("Y.toml.tmp-99999"), "partial").unwrap();
        std::fs::write(
            agents.join("Y.toml"),
            "[agent]\nname = \"Y\"\nprovider = \"openai\"\nmodel = \"m\"\n",
        )
        .unwrap();

        let migrator = TomlMigrator {
            config_dir: &agents,
            db_providers: &[],
        };
        migrator.migrate_all().await.unwrap();
        assert!(
            !agents.join("Y.toml.tmp-99999").exists(),
            "stale tmp should be cleaned"
        );
    }

    #[tokio::test]
    async fn missing_provider_match_fails_loud() {
        let dir = tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("Z.toml"),
            r#"
[agent]
name = "Z"
provider = "openai"
model = "m"

[[agent.routing]]
condition = "default"
provider = "mystery"
base_url = "http://nonexistent"
cooldown_secs = 60
"#,
        )
        .unwrap();

        let migrator = TomlMigrator {
            config_dir: &agents,
            db_providers: &[],
        };
        let err = migrator.migrate_all().await.unwrap_err();
        assert!(
            err.to_string().contains("mystery"),
            "err should name mystery provider: {err}"
        );
    }
}
