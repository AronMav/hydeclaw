use crate::config::ResourceSettings;
use std::sync::atomic::{AtomicI64, Ordering};

#[derive(Clone, serde::Serialize)]
pub struct ResourceStatus {
    pub disk_free_gb: f64,
    pub disk_warning: bool,
    pub disk_critical: bool,
    pub ram_used_percent: f64,
    pub ram_warning: bool,
    pub ram_critical: bool,
    pub cpu_load_percent: f64,
    pub graph_worker_stuck: bool,
}

pub async fn check_resources(
    cfg: &ResourceSettings,
    http: &reqwest::Client,
    core_url: &str,
    auth_token: &str,
) -> ResourceStatus {
    let disk_free_gb = get_disk_free_gb().await;
    let ram_used_percent = get_ram_used_percent().await;
    let cpu_load_percent = get_cpu_load_percent().await;
    let graph_worker_stuck =
        check_graph_worker_stuck(http, core_url, auth_token, cfg.graph_stuck_timeout_secs).await;

    ResourceStatus {
        disk_free_gb,
        disk_warning: disk_free_gb < cfg.disk_warning_gb as f64,
        disk_critical: disk_free_gb < cfg.disk_critical_gb as f64,
        ram_used_percent,
        ram_warning: ram_used_percent > cfg.ram_warning_percent as f64,
        ram_critical: ram_used_percent > cfg.ram_critical_percent as f64,
        cpu_load_percent,
        graph_worker_stuck,
    }
}

async fn get_disk_free_gb() -> f64 {
    let output = tokio::process::Command::new("df")
        .args(["--output=avail", "-BG", "/"])
        .output()
        .await;
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .nth(1)
            .and_then(|l| l.trim().trim_end_matches('G').parse().ok())
            .unwrap_or(0.0),
        Err(_) => 0.0,
    }
}

async fn get_ram_used_percent() -> f64 {
    let output = tokio::process::Command::new("free")
        .args(["-m"])
        .output()
        .await;
    match output {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            if let Some(line) = text.lines().find(|l| l.starts_with("Mem:")) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    let total: f64 = parts[1].parse().unwrap_or(1.0);
                    let used: f64 = parts[2].parse().unwrap_or(0.0);
                    return (used / total) * 100.0;
                }
            }
            0.0
        }
        Err(_) => 0.0,
    }
}

async fn get_cpu_load_percent() -> f64 {
    // Read 1-minute load average from /proc/loadavg, divide by nproc
    let loadavg = tokio::fs::read_to_string("/proc/loadavg").await.unwrap_or_default();
    let load1: f64 = loadavg.split_whitespace().next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let nproc: f64 = tokio::fs::read_to_string("/proc/cpuinfo").await
        .map(|s| s.matches("processor").count() as f64)
        .unwrap_or(1.0)
        .max(1.0);
    (load1 / nproc) * 100.0
}

/// Check if graph extraction worker is stuck.
/// Tracks done count across calls — if processing > 0 but done hasn't increased, it's stuck.
static LAST_DONE: AtomicI64 = AtomicI64::new(-1);
static STUCK_SINCE: AtomicI64 = AtomicI64::new(0);

async fn check_graph_worker_stuck(
    http: &reqwest::Client,
    core_url: &str,
    auth_token: &str,
    timeout_secs: u64,
) -> bool {
    let resp = http
        .get(format!("{}/api/memory/extraction-queue", core_url))
        .header("Authorization", format!("Bearer {}", auth_token))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;

    let (processing, done) = match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await.unwrap_or_default();
            (
                body["processing"].as_i64().unwrap_or(0),
                body["done"].as_i64().unwrap_or(0),
            )
        }
        _ => return false,
    };

    if processing == 0 {
        LAST_DONE.store(done, Ordering::Relaxed);
        STUCK_SINCE.store(0, Ordering::Relaxed);
        return false;
    }

    let prev = LAST_DONE.load(Ordering::Relaxed);
    if prev == -1 || done > prev {
        LAST_DONE.store(done, Ordering::Relaxed);
        STUCK_SINCE.store(0, Ordering::Relaxed);
        false
    } else {
        let now = chrono::Utc::now().timestamp();
        let since = STUCK_SINCE.load(Ordering::Relaxed);
        if since == 0 {
            STUCK_SINCE.store(now, Ordering::Relaxed);
            false
        } else {
            (now - since) > timeout_secs as i64
        }
    }
}
