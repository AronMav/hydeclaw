use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

/// Record a single LLM call's token usage.
pub async fn record_usage(
    db: &PgPool,
    agent_id: &str,
    provider: &str,
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
    session_id: Option<Uuid>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO usage_log (agent_id, provider, model, input_tokens, output_tokens, session_id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(agent_id)
    .bind(provider)
    .bind(model)
    .bind(input_tokens as i32)
    .bind(output_tokens as i32)
    .bind(session_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Get total tokens used by an agent today.
pub async fn get_agent_usage_today(db: &PgPool, agent_id: &str) -> Result<i64> {
    let total: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(input_tokens + output_tokens), 0) FROM usage_log \
         WHERE agent_id = $1 AND created_at > CURRENT_DATE",
    )
    .bind(agent_id)
    .fetch_one(db)
    .await?;
    Ok(total.0)
}

/// Usage summary for an agent over a time period.
#[derive(Debug, serde::Serialize)]
pub struct UsageSummary {
    pub agent_id: String,
    pub provider: String,
    pub model: String,
    pub total_input: i64,
    pub total_output: i64,
    pub call_count: i64,
    pub estimated_cost: Option<f64>,
}

/// Estimate cost in USD based on provider/model pricing (per 1M tokens).
fn estimate_cost(provider: &str, model: &str, input: i64, output: i64) -> Option<f64> {
    let (input_per_m, output_per_m) = match (provider, model) {
        ("minimax", _) if model.contains("M2.5") => (0.50, 1.50),
        ("minimax", _) => (0.50, 1.50),
        ("anthropic", m) if m.contains("opus") => (15.00, 75.00),
        ("anthropic", m) if m.contains("sonnet") => (3.00, 15.00),
        ("anthropic", m) if m.contains("haiku") => (0.25, 1.25),
        ("anthropic", _) => (3.00, 15.00),
        ("openai", m) if m.contains("gpt-4o-mini") => (0.15, 0.60),
        ("openai", m) if m.contains("gpt-4o") => (2.50, 10.00),
        ("openai", _) => (2.50, 10.00),
        ("google", m) if m.contains("flash") => (0.10, 0.40),
        ("google", m) if m.contains("pro") => (1.25, 5.00),
        ("google", _) => (0.10, 0.40),
        ("deepseek", _) => (0.14, 0.28),
        ("groq", _) => (0.05, 0.08),
        ("xai", _) => (2.00, 10.00),
        ("together", _) => (0.20, 0.60),
        ("openrouter", _) => (0.50, 1.50),
        ("mistral", _) => (0.30, 0.90),
        ("perplexity", _) => (1.00, 5.00),
        ("ollama", _) => (0.0, 0.0),
        _ => return None,
    };
    let cost = (input as f64 / 1_000_000.0) * input_per_m
        + (output as f64 / 1_000_000.0) * output_per_m;
    Some((cost * 10000.0).round() / 10000.0) // 4 decimal places
}

/// Daily usage breakdown for charts.
#[derive(Debug, serde::Serialize)]
pub struct DailyUsage {
    pub date: String,
    pub agent_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub call_count: i64,
}

/// Get daily usage breakdown for the last N days.
pub async fn usage_daily(db: &PgPool, days: u32) -> Result<Vec<DailyUsage>> {
    let rows = sqlx::query_as::<_, (chrono::NaiveDate, String, String, String, i64, i64, i64)>(
        "SELECT date_trunc('day', created_at)::date as day, \
         agent_id, provider, COALESCE(model, ''), \
         COALESCE(SUM(input_tokens), 0), \
         COALESCE(SUM(output_tokens), 0), \
         COUNT(*) \
         FROM usage_log \
         WHERE created_at > now() - make_interval(days => $1) \
         GROUP BY day, agent_id, provider, COALESCE(model, '') \
         ORDER BY day",
    )
    .bind(days as i32)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(date, agent_id, provider, model, input_tokens, output_tokens, call_count)| {
            DailyUsage {
                date: date.to_string(),
                agent_id,
                provider,
                model,
                input_tokens,
                output_tokens,
                call_count,
            }
        })
        .collect())
}

/// Per-session usage breakdown.
#[derive(Debug, serde::Serialize)]
pub struct SessionUsage {
    pub session_id: Uuid,
    pub agent_id: String,
    pub total_input: i64,
    pub total_output: i64,
    pub call_count: i64,
    pub estimated_cost: Option<f64>,
}

/// Get usage grouped by session for the last N days.
pub async fn usage_by_session(db: &PgPool, agent_id: Option<&str>, days: u32) -> Result<Vec<SessionUsage>> {
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, i64, i64, i64)>(
        "SELECT session_id, agent_id, provider, COALESCE(model, ''), \
         COALESCE(SUM(input_tokens), 0), \
         COALESCE(SUM(output_tokens), 0), \
         COUNT(*) \
         FROM usage_log \
         WHERE session_id IS NOT NULL \
         AND created_at > now() - make_interval(days => $1) \
         AND ($2::TEXT IS NULL OR agent_id = $2) \
         GROUP BY session_id, agent_id, provider, COALESCE(model, '') \
         ORDER BY SUM(input_tokens) + SUM(output_tokens) DESC \
         LIMIT 200",
    )
    .bind(days as i32)
    .bind(agent_id)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(session_id, agent_id, provider, model, total_input, total_output, call_count)| {
            let estimated_cost = estimate_cost(&provider, &model, total_input, total_output);
            SessionUsage {
                session_id,
                agent_id,
                total_input,
                total_output,
                call_count,
                estimated_cost,
            }
        })
        .collect())
}

/// Get usage summary grouped by agent+provider+model for the last N days.
pub async fn usage_summary(db: &PgPool, days: u32) -> Result<Vec<UsageSummary>> {
    let rows = sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(
        "SELECT agent_id, provider, COALESCE(model, ''), \
         COALESCE(SUM(input_tokens), 0), \
         COALESCE(SUM(output_tokens), 0), \
         COUNT(*) \
         FROM usage_log \
         WHERE created_at > now() - make_interval(days => $1) \
         GROUP BY agent_id, provider, COALESCE(model, '') \
         ORDER BY agent_id, provider, COALESCE(model, '')",
    )
    .bind(days as i32)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(agent_id, provider, model, total_input, total_output, call_count)| {
            let estimated_cost = estimate_cost(&provider, &model, total_input, total_output);
            UsageSummary {
                agent_id,
                provider,
                model,
                total_input,
                total_output,
                call_count,
                estimated_cost,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimax_m25_pricing() {
        // 1M input @ $0.50/M + 1M output @ $1.50/M = $2.00
        let cost = estimate_cost("minimax", "MiniMax-M2.5", 1_000_000, 1_000_000);
        assert_eq!(cost, Some(2.0));
    }

    #[test]
    fn anthropic_opus_pricing() {
        // 1M input @ $15.00/M + 1M output @ $75.00/M = $90.00
        let cost = estimate_cost("anthropic", "claude-opus-3", 1_000_000, 1_000_000);
        assert_eq!(cost, Some(90.0));
    }

    #[test]
    fn anthropic_sonnet_pricing() {
        // 1M input @ $3.00/M + 1M output @ $15.00/M = $18.00
        let cost = estimate_cost("anthropic", "claude-sonnet-4", 1_000_000, 1_000_000);
        assert_eq!(cost, Some(18.0));
    }

    #[test]
    fn openai_gpt4o_mini_pricing() {
        // 1M input @ $0.15/M + 1M output @ $0.60/M = $0.75
        let cost = estimate_cost("openai", "gpt-4o-mini", 1_000_000, 1_000_000);
        assert_eq!(cost, Some(0.75));
    }

    #[test]
    fn openai_gpt4o_pricing() {
        // 1M input @ $2.50/M + 1M output @ $10.00/M = $12.50
        let cost = estimate_cost("openai", "gpt-4o", 1_000_000, 1_000_000);
        assert_eq!(cost, Some(12.5));
    }

    #[test]
    fn unknown_provider_returns_none() {
        let cost = estimate_cost("unknownprovider", "some-model", 1_000_000, 1_000_000);
        assert_eq!(cost, None);
    }

    #[test]
    fn zero_tokens_returns_zero_cost() {
        let cost = estimate_cost("anthropic", "claude-sonnet-4", 0, 0);
        assert_eq!(cost, Some(0.0));
    }

    #[test]
    fn ollama_is_free() {
        let cost = estimate_cost("ollama", "llama3", 1_000_000, 1_000_000);
        assert_eq!(cost, Some(0.0));
    }

    #[test]
    fn one_million_input_one_million_output_deepseek() {
        // deepseek: $0.14/M input + $0.28/M output = $0.42
        let cost = estimate_cost("deepseek", "deepseek-chat", 1_000_000, 1_000_000);
        assert_eq!(cost, Some(0.42));
    }
}
