-- Spec §6.1. Moves flat `timeout_secs` into nested `timeouts.request_secs`
-- and provisions the other three tiers with defaults. Idempotent: a
-- second run finds both the legacy key absent AND the `timeouts` object
-- present, so the WHERE clause evaluates to false.
--
-- `timeout_secs: 0` (legacy "no limit") becomes `request_secs: 0`.
-- Flagged in `system_flags` for operator follow-up.

UPDATE providers
SET options = jsonb_set(
    options - 'timeout_secs',
    '{timeouts}',
    jsonb_build_object(
        'connect_secs',              10,
        'request_secs',              COALESCE((options->>'timeout_secs')::int, 120),
        'stream_inactivity_secs',    60,
        'stream_max_duration_secs',  600
    ),
    true
)
WHERE
    (options ? 'timeout_secs')
    OR NOT (options ? 'timeouts');

-- Record providers that ended up with "no request limit" so operators
-- see them in the v0.20 release-notes follow-up checklist.
INSERT INTO system_flags (key, value, updated_at)
SELECT
    'v020_providers_with_no_request_limit',
    COALESCE(jsonb_agg(name ORDER BY name), '[]'::jsonb),
    NOW()
FROM providers
WHERE (options->'timeouts'->>'request_secs')::int = 0
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW();
