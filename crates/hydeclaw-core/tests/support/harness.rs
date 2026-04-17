//! Ephemeral PostgreSQL test harness.
//!
//! Spawns a fresh `pgvector/pgvector:pg17` container per `TestHarness::new()` call
//! (matches production image — see CONTEXT.md decision), applies every migration
//! in `migrations/`, and exposes a connected pool.
//!
//! On Drop the underlying `ContainerAsync` is dropped, which removes the container.
//!
//! Override the image via `HYDECLAW_PG_TEST_IMAGE=<repo>:<tag>` (split on the LAST
//! `:` so registry hosts with explicit ports still parse). Examples:
//!   - HYDECLAW_PG_TEST_IMAGE=postgres:17                 (vanilla PG, no pgvector)
//!   - HYDECLAW_PG_TEST_IMAGE=ghcr.io/foo/pg:17-age       (custom registry)

use anyhow::{Context, Result};
use sqlx::PgPool;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// Env var: full `image:tag` override. CONTEXT.md decision-locked name.
const PG_IMAGE_ENV: &str = "HYDECLAW_PG_TEST_IMAGE";
/// Default image — matches production deployment. CONTEXT.md decision-locked value.
const DEFAULT_PG_IMAGE: &str = "pgvector/pgvector:pg17";

pub struct TestHarness {
    // Order matters: `pool` must drop before `_container` so connections
    // close cleanly before the container is torn down.
    pool: PgPool,
    pg_url: String,
    _container: ContainerAsync<GenericImage>,
}

impl TestHarness {
    /// Spin up a fresh PG container, run all migrations, return a connected harness.
    pub async fn new() -> Result<Self> {
        let image_spec = std::env::var(PG_IMAGE_ENV)
            .unwrap_or_else(|_| DEFAULT_PG_IMAGE.to_string());

        // Split on the LAST ':' so e.g. `registry.example.com:5000/pg:17` parses correctly.
        let (repo, tag) = match image_spec.rsplit_once(':') {
            Some((r, t)) if !r.is_empty() && !t.is_empty() => (r.to_string(), t.to_string()),
            _ => anyhow::bail!(
                "{} must be of the form '<image>:<tag>', got: {:?}",
                PG_IMAGE_ENV,
                image_spec
            ),
        };

        let image = GenericImage::new(&repo, &tag)
            .with_exposed_port(5432.tcp())
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ));

        let container = image
            .with_env_var("POSTGRES_PASSWORD", "postgres")
            .with_env_var("POSTGRES_USER", "postgres")
            .with_env_var("POSTGRES_DB", "postgres")
            .start()
            .await
            .with_context(|| format!("starting ephemeral PostgreSQL container ({image_spec})"))?;

        let host_port = container
            .get_host_port_ipv4(5432)
            .await
            .context("resolving container host port")?;
        let pg_url = format!(
            "postgres://postgres:postgres@127.0.0.1:{host_port}/postgres"
        );

        let pool = PgPool::connect(&pg_url)
            .await
            .context("connecting to ephemeral PG")?;

        super::migrations::apply_all(&pool)
            .await
            .context("applying migrations to ephemeral PG")?;

        Ok(Self {
            pool,
            pg_url,
            _container: container,
        })
    }

    /// Borrow the connected pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// PostgreSQL URL of the ephemeral container.
    pub fn pg_url(&self) -> &str {
        &self.pg_url
    }
}
