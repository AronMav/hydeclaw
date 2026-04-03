mod config;
mod tasks;
mod handlers;

use sqlx::postgres::PgPoolOptions;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // Load .env from binary dir (memory-worker runs as separate binary)
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("hydeclaw_memory_worker=info".parse()?),
        )
        .init();

    let config_path = std::env::args().nth(1).unwrap_or("config/hydeclaw.toml".into());
    let cfg = config::load_config(&config_path)?;

    if !cfg.worker.enabled {
        tracing::info!("memory worker disabled");
        return Ok(());
    }

    tracing::info!(
        toolgate_url = %cfg.toolgate_url,
        workspace_dir = %cfg.workspace_dir,
        fts_language = %cfg.fts_language,
        poll = cfg.worker.poll_interval_secs,
        "memory worker starting"
    );

    let db = PgPoolOptions::new()
        .max_connections(3)
        .connect(&cfg.database_url)
        .await?;
    tracing::info!("database connected");

    // Recover stuck 'processing' tasks from previous crash
    let recovered = tasks::recover_stuck(&db).await?;
    if recovered > 0 {
        tracing::info!(recovered, "recovered stuck tasks from previous crash");
    }

    #[cfg(target_os = "linux")]
    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);

    let poll = std::time::Duration::from_secs(cfg.worker.poll_interval_secs);
    let ctx = handlers::DispatchCtx {
        toolgate_url: &cfg.toolgate_url,
        workspace_dir: &cfg.workspace_dir,
        fts_language: &cfg.fts_language,
    };

    loop {
        match tasks::claim_next(&db).await {
            Ok(Some(task)) => {
                tracing::info!(id = %task.id, task_type = %task.task_type, "processing task");
                match handlers::dispatch(&task, &db, &ctx).await {
                    Ok(result) => {
                        tasks::complete(&db, task.id, result).await?;
                        tracing::info!(id = %task.id, "task completed");
                    }
                    Err(e) => {
                        tasks::fail(&db, task.id, &e.to_string()).await?;
                        tracing::error!(id = %task.id, error = %e, "task failed");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
            Ok(None) => {
                tokio::time::sleep(poll).await;
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to claim task");
                tokio::time::sleep(poll).await;
            }
        }

        #[cfg(target_os = "linux")]
        let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog]);
    }
}

