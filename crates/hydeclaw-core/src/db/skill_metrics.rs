use sqlx::PgPool;

/// Records that a skill-assisted task completed (success or failure).
/// Updates `times_applied`, `times_success/times_fail`, and recalculates
/// `effectiveness_score` = `times_success` / `times_applied`.
pub async fn record_outcome(db: &PgPool, skill_name: &str, success: bool) -> sqlx::Result<()> {
    if success {
        sqlx::query(
            r"
            INSERT INTO skill_metrics (skill_name, times_applied, times_success, effectiveness_score)
            VALUES ($1, 1, 1, 1.0)
            ON CONFLICT (skill_name) DO UPDATE
                SET times_applied       = skill_metrics.times_applied + 1,
                    times_success       = skill_metrics.times_success + 1,
                    effectiveness_score = (skill_metrics.times_success + 1)::REAL
                                          / (skill_metrics.times_applied + 1)::REAL,
                    updated_at          = NOW()
            ",
        )
        .bind(skill_name)
        .execute(db)
        .await?;
    } else {
        sqlx::query(
            r"
            INSERT INTO skill_metrics (skill_name, times_applied, times_fail, effectiveness_score)
            VALUES ($1, 1, 1, 0.0)
            ON CONFLICT (skill_name) DO UPDATE
                SET times_applied       = skill_metrics.times_applied + 1,
                    times_fail          = skill_metrics.times_fail + 1,
                    effectiveness_score = skill_metrics.times_success::REAL
                                          / (skill_metrics.times_applied + 1)::REAL,
                    updated_at          = NOW()
            ",
        )
        .bind(skill_name)
        .execute(db)
        .await?;
    }
    Ok(())
}
