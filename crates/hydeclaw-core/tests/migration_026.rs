#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn migration_026_idempotent_on_second_run(pool: PgPool) {
    // Seed a provider with legacy flat timeout.
    sqlx::query(
        "INSERT INTO providers (id, name, type, provider_type, enabled, options)
         VALUES (gen_random_uuid(), 'legacy', 'llm', 'openai', true,
                 '{\"timeout_secs\": 45}'::jsonb)",
    )
    .execute(&pool).await.unwrap();

    // The framework already ran migrations; simulate a second run by
    // re-executing 026 explicitly.
    let sql = std::fs::read_to_string("../../migrations/026_provider_timeouts_nested.sql").unwrap();
    sqlx::raw_sql(&sql).execute(&pool).await.unwrap();

    // Operator edit BETWEEN runs: change request_secs to 30 manually.
    sqlx::query(
        "UPDATE providers SET options = jsonb_set(options, '{timeouts,request_secs}', '30'::jsonb) WHERE name='legacy'"
    ).execute(&pool).await.unwrap();

    // Third run — must NOT overwrite the 30.
    sqlx::raw_sql(&sql).execute(&pool).await.unwrap();

    let v: serde_json::Value = sqlx::query_scalar(
        "SELECT options FROM providers WHERE name='legacy'"
    ).fetch_one(&pool).await.unwrap();

    assert_eq!(v["timeouts"]["request_secs"], serde_json::json!(30));
    assert_eq!(v["timeouts"]["connect_secs"], serde_json::json!(10));
    assert!(v.get("timeout_secs").is_none(), "legacy key must be gone");
}

#[sqlx::test(migrations = "../../migrations")]
async fn migration_026_preserves_timeout_zero(pool: PgPool) {
    sqlx::query(
        "INSERT INTO providers (id, name, type, provider_type, enabled, options)
         VALUES (gen_random_uuid(), 'unlimited', 'llm', 'openai', true,
                 '{\"timeout_secs\": 0}'::jsonb)",
    )
    .execute(&pool).await.unwrap();

    let sql = std::fs::read_to_string("../../migrations/026_provider_timeouts_nested.sql").unwrap();
    sqlx::raw_sql(&sql).execute(&pool).await.unwrap();

    let req: i64 = sqlx::query_scalar(
        "SELECT (options->'timeouts'->>'request_secs')::bigint FROM providers WHERE name='unlimited'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(req, 0);

    let flag: serde_json::Value = sqlx::query_scalar(
        "SELECT value FROM system_flags WHERE key='v020_providers_with_no_request_limit'"
    ).fetch_one(&pool).await.unwrap();
    assert!(flag.as_array().unwrap().iter().any(|n| n == "unlimited"));
}
