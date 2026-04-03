//! PII redaction — strips personal identifiers before LLM calls.

use std::sync::LazyLock;
use regex::Regex;

static PHONE_RU: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\+7|8)[\s\-]?\(?\d{3}\)?[\s\-]?\d{3}[\s\-]?\d{2}[\s\-]?\d{2}").unwrap()
});

static PHONE_INTL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\+\d{1,3}[\s\-]?\(?\d{2,4}\)?[\s\-]?\d{3,4}[\s\-]?\d{2,4}").unwrap()
});

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}").unwrap()
});

static CARD_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{4}[\s\-]?\d{4}[\s\-]?\d{4}[\s\-]?\d{4}\b").unwrap()
});

/// Matches common API key prefixes (sk-..., Bearer long-token).
static API_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:sk-[a-zA-Z0-9_-]{20,}|Bearer\s+[a-zA-Z0-9._-]{30,})").unwrap()
});

/// Matches long hex tokens (64+ hex chars, common for auth tokens).
static HEX_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[0-9a-fA-F]{64,}\b").unwrap()
});

/// Matches environment variable assignments with sensitive key names.
/// Catches KEY=value patterns for tokens, keys, secrets, passwords, URLs with credentials.
static ENV_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^([A-Z_]*(?:TOKEN|KEY|SECRET|PASSWORD|CREDENTIAL|MASTER)[A-Z_]*)=(.+)$").unwrap()
});

/// Matches DATABASE_URL with embedded credentials (user:pass@host).
static DB_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(DATABASE_URL\s*=\s*)(\S+)").unwrap()
});

/// Redact PII from text. Returns (redacted_text, count_of_redactions).
pub fn redact(text: &str) -> (String, usize) {
    let mut result = text.to_string();
    let mut count = 0;

    for (pattern, replacement) in [
        (&*PHONE_RU, "[PHONE]"),
        (&*PHONE_INTL, "[PHONE]"),
        (&*EMAIL, "[EMAIL]"),
        (&*CARD_NUMBER, "[CARD]"),
        (&*API_KEY, "[API_KEY]"),
        (&*HEX_TOKEN, "[TOKEN]"),
    ] {
        let matches = pattern.find_iter(&result).count();
        if matches > 0 {
            count += matches;
            result = pattern.replace_all(&result, replacement).into_owned();
        }
    }

    (result, count)
}

/// Redact secrets from code_exec output. More aggressive than general PII —
/// catches env var assignments (TOKEN=..., KEY=..., SECRET=..., PASSWORD=..., DATABASE_URL=...).
pub fn redact_code_output(text: &str) -> (String, usize) {
    let mut result = text.to_string();
    let mut count = 0;

    // First: env var secret assignments (KEY=value → KEY=[REDACTED])
    let env_matches = ENV_SECRET.find_iter(&result).count();
    if env_matches > 0 {
        count += env_matches;
        result = ENV_SECRET.replace_all(&result, "$1=[REDACTED]").into_owned();
    }

    // DATABASE_URL with credentials
    let db_matches = DB_URL.find_iter(&result).count();
    if db_matches > 0 {
        count += db_matches;
        result = DB_URL.replace_all(&result, "${1}[REDACTED]").into_owned();
    }

    // Then: standard PII (hex tokens, API keys, etc.)
    let (result, pii_count) = redact(&result);
    (result, count + pii_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phone_redaction() {
        let (r, c) = redact("Позвони мне +7 (999) 123-45-67 завтра");
        assert!(r.contains("[PHONE]"));
        assert!(!r.contains("999"));
        assert!(c > 0);
    }

    #[test]
    fn test_email_redaction() {
        let (r, _) = redact("Напиши на user@example.com");
        assert!(r.contains("[EMAIL]"));
        assert!(!r.contains("user@example.com"));
    }

    #[test]
    fn test_card_redaction() {
        let (r, _) = redact("Карта 4276 3801 1234 5678");
        assert!(r.contains("[CARD]"));
    }

    #[test]
    fn test_no_false_positives() {
        let (r, c) = redact("Привет, как дела?");
        assert_eq!(r, "Привет, как дела?");
        assert_eq!(c, 0);
    }

    #[test]
    fn test_multiple_phones_count() {
        let (_, c) = redact("Звони +7 999 111-22-33 или +7 888 444-55-66");
        assert_eq!(c, 2);
    }

    #[test]
    fn test_api_key_redaction() {
        let (r, c) = redact("key: sk-proj-abc123xyz789defghijklmnop");
        assert!(r.contains("[API_KEY]"), "got: {}", r);
        assert!(!r.contains("sk-proj-abc123"));
        assert!(c > 0);
    }

    #[test]
    fn test_long_hex_token_redaction() {
        let (r, _) = redact("token=13c43c3f3db4413003f0da16013764d06de812f4066c210f59a99f610b9c665e");
        assert!(r.contains("[TOKEN]"), "got: {}", r);
    }

    #[test]
    fn test_bearer_redaction() {
        let (r, _) = redact("Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jV");
        assert!(r.contains("[API_KEY]"), "got: {}", r);
    }

    #[test]
    fn test_short_strings_not_redacted() {
        let (r, c) = redact("model=gpt-4 temperature=0.7");
        assert_eq!(r, "model=gpt-4 temperature=0.7");
        assert_eq!(c, 0);
    }

    #[test]
    fn test_env_secret_redaction() {
        let input = "HYDECLAW_AUTH_TOKEN=bd1fed8f21f5851459a8a04e83813ed11c87395a\nHYDECLAW_MASTER_KEY=8af82e59c9c7b827d1aa\nDATABASE_URL=postgresql://user:pass@localhost/db";
        let (r, c) = redact_code_output(input);
        assert!(r.contains("HYDECLAW_AUTH_TOKEN=[REDACTED]"), "got: {}", r);
        assert!(r.contains("HYDECLAW_MASTER_KEY=[REDACTED]"), "got: {}", r);
        assert!(r.contains("DATABASE_URL=[REDACTED]"), "got: {}", r);
        assert!(!r.contains("bd1fed8f"));
        assert!(!r.contains("8af82e59"));
        assert!(!r.contains("user:pass"));
        assert!(c >= 3);
    }

    #[test]
    fn test_env_secret_various_keys() {
        let input = "API_TOKEN=abc123\nSECRET_KEY=xyz789\nDB_PASSWORD=hunter2";
        let (r, _) = redact_code_output(input);
        assert!(r.contains("API_TOKEN=[REDACTED]"), "got: {}", r);
        assert!(r.contains("SECRET_KEY=[REDACTED]"), "got: {}", r);
        assert!(r.contains("DB_PASSWORD=[REDACTED]"), "got: {}", r);
    }

    #[test]
    fn test_non_secret_env_not_redacted() {
        let input = "HOME=/home/user\nPATH=/usr/bin\nLANG=en_US.UTF-8";
        let (r, c) = redact_code_output(input);
        assert_eq!(r, input);
        assert_eq!(c, 0);
    }
}
