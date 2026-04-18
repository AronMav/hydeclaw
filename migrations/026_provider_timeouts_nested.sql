-- Spec §6.1. Moves flat `timeout_secs` into nested `timeouts.request_secs`
-- and provisions the other three tiers with defaults.
--
-- `timeout_secs: 0` (legacy "no limit") becomes `request_secs: 0`.
-- Flagged in `system_flags` for operator follow-up.
--
-- Issue #3: two-step update preserves hand-edited `timeouts` objects.
-- The previous single UPDATE matched any row with EITHER legacy
-- `timeout_secs` OR no `timeouts` key, and then `jsonb_set` unconditionally
-- rebuilt `timeouts` from defaults + `timeout_secs` — clobbering any
-- operator-tuned values on rows where BOTH keys coexisted.

-- Step 1: Add `timeouts` object for rows that lack it. Covers:
--   * fresh rows with neither key (full defaults)
--   * legacy rows with only `timeout_secs` (hoist into request_secs)
-- Also strips the legacy `timeout_secs` key on this path.
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
WHERE NOT (options ? 'timeouts');

-- Step 2: Strip orphan `timeout_secs` from rows that already have
-- `timeouts` (hand-edit between runs, or a previously migrated row that
-- had both keys). The nested `timeouts` object is preserved verbatim.
UPDATE providers
SET options = options - 'timeout_secs'
WHERE (options ? 'timeout_secs') AND (options ? 'timeouts');

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
