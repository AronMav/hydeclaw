use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryWorkerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_5")]
    pub poll_interval_secs: u64,
}

impl Default for MemoryWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 5,
        }
    }
}

fn default_true() -> bool { true }
fn default_5() -> u64 { 5 }

#[derive(Debug, Deserialize)]
struct AppConfigPartial {
    #[serde(default)]
    pub memory_worker: MemoryWorkerConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MemoryConfig {
    pub workspace_dir: Option<String>,
}

pub struct WorkerConfig {
    pub worker: MemoryWorkerConfig,
    pub database_url: String,
    pub toolgate_url: String,
    pub workspace_dir: String,
    pub fts_language: String,
}

/// Detect FTS language from agent language code.
fn detect_fts_language(lang: &str) -> &'static str {
    match lang {
        "ru" => "russian",
        "en" => "english",
        "es" => "spanish",
        "de" => "german",
        "fr" => "french",
        "pt" => "portuguese",
        "it" => "italian",
        "nl" => "dutch",
        "sv" => "swedish",
        "no" | "nb" => "norwegian",
        "da" => "danish",
        "fi" => "finnish",
        "hu" => "hungarian",
        "ro" => "romanian",
        "tr" => "turkish",
        _ => "simple",
    }
}

/// Read language from Hyde agent TOML.
///
/// Hyde is the designated system-configuration agent — its `[agent] language`
/// field sets the deployment locale (e.g. "ru", "en"). The memory worker reads this
/// to select the correct PostgreSQL FTS dictionary (e.g. 'russian', 'english') for
/// `to_tsvector()` so full-text search matches the actual content language.
fn read_base_agent_language(config_path: &str) -> String {
    let config_dir = std::path::Path::new(config_path)
        .parent()
        .unwrap_or(std::path::Path::new("config"));
    let agent_path = config_dir.join("agents/Hyde.toml");
    if let Ok(text) = std::fs::read_to_string(&agent_path) {
        #[derive(Deserialize, Default)]
        struct AgentSection {
            #[serde(default)]
            language: String,
        }
        #[derive(Deserialize)]
        struct AgentPartial {
            #[serde(default)]
            agent: AgentSection,
        }
        if let Ok(a) = toml::from_str::<AgentPartial>(&text)
            && !a.agent.language.is_empty()
        {
            return detect_fts_language(&a.agent.language).to_string();
        }
    }
    "simple".to_string()
}

pub fn load_config(path: &str) -> anyhow::Result<WorkerConfig> {
    let text = std::fs::read_to_string(path)?;
    let cfg: AppConfigPartial = toml::from_str(&text)?;
    let db_url = std::env::var("DATABASE_URL").unwrap_or(cfg.database.url);
    let fts_language = read_base_agent_language(path);
    let toolgate_url = std::env::var("TOOLGATE_URL")
        .unwrap_or_else(|_| "http://localhost:9011".to_string());

    Ok(WorkerConfig {
        worker: cfg.memory_worker,
        database_url: db_url,
        toolgate_url,
        workspace_dir: cfg.memory.workspace_dir.unwrap_or_else(|| "workspace".to_string()),
        fts_language,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_fts_language_russian() {
        assert_eq!(detect_fts_language("ru"), "russian");
    }

    #[test]
    fn test_detect_fts_language_english() {
        assert_eq!(detect_fts_language("en"), "english");
    }

    #[test]
    fn test_detect_fts_language_unknown() {
        assert_eq!(detect_fts_language("xx"), "simple");
    }

    #[test]
    fn test_default_config() {
        let cfg = MemoryWorkerConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.poll_interval_secs, 5);
    }
}
