mod config;
mod tasks;
mod handlers;

use sqlx::postgres::{PgListener, PgPoolOptions};

/// Wake source for the hybrid LISTEN/poll loop.
///
/// REF-04: LISTEN is primary; poll is the 60-second safety net that reclaims
/// anything the listener missed (e.g. dropped socket during a NOTIFY burst).
/// `ListenerDied` signals that the listener connection errored and must be
/// rebuilt on the next iteration.
enum Wake {
    Notify,
    Poll,
    ListenerDied,
}

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
        notify_mode = ?cfg.worker.notify_mode,
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

    // ── REF-04: LISTEN/NOTIFY primary + poll safety net ─────────────────────
    //
    // Primary wake: PgListener on `memory_tasks_new`. Sub-100ms steady-state
    // pickup under normal operation (migration 023 trigger pg_notify's on every
    // INSERT commit).
    //
    // Safety net: poll every `poll_interval_secs` (HCS-4 preserved). Reclaims
    // anything that slipped through while LISTEN was dead (socket drop, burst
    // coalescing at the PG layer, etc.).

    let mut poll_tick = tokio::time::interval(poll);
    poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // First tick fires immediately — skip it so the initial wake is a real poll
    // after `poll_interval_secs`, not a tight loop at startup.
    poll_tick.tick().await;

    let mut listener: Option<PgListener> = if cfg.worker.notify_mode == config::NotifyMode::Listen {
        connect_listener(&cfg.database_url).await
    } else {
        tracing::info!("notify_mode = poll — skipping LISTEN, polling only");
        None
    };

    loop {
        // Wait for EITHER a NOTIFY or the poll tick (catch-up safety net).
        let wake = match &mut listener {
            Some(l) => {
                tokio::select! {
                    notif = l.recv() => match notif {
                        Ok(_n) => Wake::Notify,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "PgListener recv failed; will reconnect on next iteration"
                            );
                            Wake::ListenerDied
                        }
                    },
                    _ = poll_tick.tick() => Wake::Poll,
                }
            }
            None => {
                poll_tick.tick().await;
                Wake::Poll
            }
        };

        // Reclaim a listener if it died and operators want LISTEN mode.
        // Drop the broken one first, then attempt a fresh connect. If the
        // reconnect fails, `listener` becomes `None` and the next iteration
        // relies on the poll tick until the next attempt.
        if matches!(wake, Wake::ListenerDied)
            && cfg.worker.notify_mode == config::NotifyMode::Listen
        {
            drop(listener.take());
            listener = connect_listener(&cfg.database_url).await;
            // Fall through and drain pending tasks unconditionally — the poll
            // path is still responsible for catch-up.
        }

        // Drain pending work: NOTIFY may coalesce bursts at the PG layer, so
        // one recv() can correspond to N new tasks. Poll ticks use the same
        // drain to catch up anything that slipped through.
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
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(error = %e, "failed to claim task");
                    break;
                }
            }
        }

        #[cfg(target_os = "linux")]
        let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog]);
    }
}

/// Build a `PgListener` subscribed to `memory_tasks_new`.
///
/// Returns `None` on any failure (connect, subscribe) so the caller falls back
/// to pure-polling mode for this iteration and retries on the next poll tick.
/// Failures are logged at WARN so operators can spot persistent LISTEN issues.
async fn connect_listener(database_url: &str) -> Option<PgListener> {
    match PgListener::connect(database_url).await {
        Ok(mut l) => match l.listen("memory_tasks_new").await {
            Ok(()) => {
                tracing::info!("LISTEN memory_tasks_new active");
                Some(l)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "listener.listen(memory_tasks_new) failed; falling back to poll-only this cycle"
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                "PgListener::connect failed; falling back to poll-only this cycle"
            );
            None
        }
    }
}
