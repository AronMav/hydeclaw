use serde::{Deserialize, Serialize};

/// Four-tier LLM timeout model. Every LLM call is governed by all four.
/// Zero means "no limit" for `request_secs`, `stream_inactivity_secs`,
/// `stream_max_duration_secs`. `connect_secs` must be non-zero (a connect
/// with no upper bound cannot fail over — enforced in `validate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeoutsConfig {
    #[serde(default = "default_connect_secs")]
    pub connect_secs: u64,
    #[serde(default = "default_request_secs")]
    pub request_secs: u64,
    #[serde(default = "default_stream_inactivity_secs")]
    pub stream_inactivity_secs: u64,
    #[serde(default = "default_stream_max_duration_secs")]
    pub stream_max_duration_secs: u64,
}

fn default_connect_secs() -> u64 { 10 }
fn default_request_secs() -> u64 { 120 }
fn default_stream_inactivity_secs() -> u64 { 60 }
fn default_stream_max_duration_secs() -> u64 { 600 }

impl Default for TimeoutsConfig {
    fn default() -> Self {
        Self {
            connect_secs: default_connect_secs(),
            request_secs: default_request_secs(),
            stream_inactivity_secs: default_stream_inactivity_secs(),
            stream_max_duration_secs: default_stream_max_duration_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_connect_secs_is_10() {
        assert_eq!(TimeoutsConfig::default().connect_secs, 10);
    }

    #[test]
    fn default_request_secs_is_120() {
        assert_eq!(TimeoutsConfig::default().request_secs, 120);
    }

    #[test]
    fn default_stream_inactivity_secs_is_60() {
        assert_eq!(TimeoutsConfig::default().stream_inactivity_secs, 60);
    }

    #[test]
    fn default_stream_max_duration_secs_is_600() {
        assert_eq!(TimeoutsConfig::default().stream_max_duration_secs, 600);
    }

    #[test]
    fn json_roundtrip_partial_object_fills_defaults() {
        let input = r#"{"request_secs": 30}"#;
        let cfg: TimeoutsConfig = serde_json::from_str(input).unwrap();
        assert_eq!(cfg.connect_secs, 10);
        assert_eq!(cfg.request_secs, 30);
        assert_eq!(cfg.stream_inactivity_secs, 60);
        assert_eq!(cfg.stream_max_duration_secs, 600);
    }

    #[test]
    fn json_roundtrip_empty_object_fills_all_defaults() {
        let cfg: TimeoutsConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, TimeoutsConfig::default());
    }
}
