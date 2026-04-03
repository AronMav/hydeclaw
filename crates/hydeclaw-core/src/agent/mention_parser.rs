/// Parse @AgentName mentions from message text.
/// Returns all mentioned agent names (in order of appearance).
///
/// Word boundary rule: `@` must be preceded by whitespace, start of string,
/// or a punctuation character (not alphanumeric or `.`). This prevents
/// matching inside email addresses like `user@Arty.com`.
pub fn parse_mentions(text: &str, known_agents: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    let lower_text = text.to_lowercase();

    for agent in known_agents {
        let lower_pattern = format!("@{}", agent.to_lowercase());

        // Find all occurrences with word boundary check
        let mut search_from = 0;
        while let Some(pos) = lower_text[search_from..].find(&lower_pattern) {
            let abs_pos = search_from + pos;

            // Check word boundary BEFORE the @
            let valid_start = if abs_pos == 0 {
                true
            } else {
                let prev_char = text.as_bytes()[abs_pos - 1];
                // Previous char must be whitespace or certain punctuation, NOT alphanumeric or dot
                prev_char.is_ascii_whitespace() || matches!(prev_char, b'(' | b'[' | b'{' | b',' | b';' | b':' | b'"' | b'\'' | b'\n')
            };

            // Check word boundary AFTER the mention
            let end_pos = abs_pos + lower_pattern.len();
            let valid_end = if end_pos >= text.len() {
                true
            } else {
                let next_char = text.as_bytes()[end_pos];
                !next_char.is_ascii_alphanumeric() && next_char != b'_'
            };

            if valid_start && valid_end && !found.contains(&agent.clone()) {
                found.push(agent.clone());
            }

            search_from = abs_pos + 1;
        }
    }
    found
}

/// Return the first mentioned agent, or None.
pub fn parse_first_mention(text: &str, known_agents: &[String]) -> Option<String> {
    parse_mentions(text, known_agents).into_iter().next()
}

/// Strip @AgentName mention from text, returning cleaned text.
/// Case-insensitive replacement.
pub fn strip_mention(text: &str, agent_name: &str) -> String {
    let pattern = format!("@{}", agent_name);
    let lower_text = text.to_lowercase();
    let lower_pattern = pattern.to_lowercase();
    match lower_text.find(&lower_pattern) {
        Some(pos) => {
            let end = pos + pattern.len();
            // Also strip trailing punctuation/whitespace after the mention (e.g. "@Agent, " → "")
            let after = &text[end..];
            let after_trimmed = after.trim_start_matches([',', ':', ';', ' ']);
            let mut result = String::with_capacity(text.len());
            result.push_str(&text[..pos]);
            result.push_str(after_trimmed);
            result.trim().to_string()
        }
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agents() -> Vec<String> {
        vec!["Arty".to_string(), "Hyde".to_string(), "Alma".to_string()]
    }

    #[test]
    fn parse_mention_found() {
        assert_eq!(parse_first_mention("@Arty check portfolio", &agents()), Some("Arty".to_string()));
    }

    #[test]
    fn parse_mention_not_found() {
        assert_eq!(parse_first_mention("hello world", &agents()), None);
    }

    #[test]
    fn parse_mention_case_insensitive() {
        assert_eq!(parse_first_mention("@arty do something", &agents()), Some("Arty".to_string()));
    }

    #[test]
    fn no_match_in_email() {
        // Must NOT match @Arty inside an email address
        assert_eq!(parse_first_mention("email@Arty.com", &agents()), None);
    }

    #[test]
    fn no_match_in_email_with_dot() {
        assert_eq!(parse_first_mention("user.name@Hyde.org", &agents()), None);
    }

    #[test]
    fn match_after_newline() {
        assert_eq!(parse_first_mention("hello\n@Arty check this", &agents()), Some("Arty".to_string()));
    }

    #[test]
    fn match_at_start() {
        assert_eq!(parse_first_mention("@Hyde review this", &agents()), Some("Hyde".to_string()));
    }

    #[test]
    fn no_match_partial_name() {
        // @Art should not match if followed by more alphanumeric chars (e.g. "Arty")
        let agents = vec!["Art".to_string()];
        assert_eq!(parse_first_mention("@Arty check", &agents), None);
    }

    #[test]
    fn strip_mention_cleans_text() {
        assert_eq!(strip_mention("@Arty check portfolio", "Arty"), "check portfolio");
    }

    #[test]
    fn strip_mention_case_insensitive() {
        assert_eq!(strip_mention("@arty check portfolio", "Arty"), "check portfolio");
    }

    #[test]
    fn multiple_mentions() {
        let result = parse_mentions("@Arty and @Hyde review this", &agents());
        assert_eq!(result, vec!["Arty".to_string(), "Hyde".to_string()]);
    }

    #[test]
    fn self_mention_filtered_finds_other() {
        // D-11: "I, @Arty, will ask @Hyde" -> filter out self (Arty) -> find Hyde
        let mentions = parse_mentions("I, @Arty, will ask @Hyde to review", &agents());
        let non_self: Vec<_> = mentions.into_iter().filter(|n| n != "Arty").collect();
        assert_eq!(non_self, vec!["Hyde".to_string()]);
    }

    #[test]
    fn self_mention_only_returns_empty() {
        // D-09: "I, @Arty, will help you" -> filter out self -> empty -> no routing
        let mentions = parse_mentions("I, @Arty, will help you with that", &agents());
        let non_self: Vec<_> = mentions.into_iter().filter(|n| n != "Arty").collect();
        assert!(non_self.is_empty());
    }

    #[test]
    fn self_mention_preserves_order() {
        // parse_mentions returns in agent-list order (Arty, Hyde, Alma), not text order
        let mentions = parse_mentions("@Arty says ask @Alma then @Hyde", &agents());
        let non_self: Vec<_> = mentions.into_iter().filter(|n| n != "Arty").collect();
        assert_eq!(non_self, vec!["Hyde".to_string(), "Alma".to_string()]);
    }
}
