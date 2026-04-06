-- 007_system_flags: persistent boolean flags for system state
-- setup_complete: true = wizard finished, guards POST /api/setup/* routes

CREATE TABLE IF NOT EXISTS system_flags (
    key TEXT PRIMARY KEY,
    value JSONB NOT NULL DEFAULT 'null'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- For existing installs (providers table already populated), mark setup as complete.
-- Providers are created during the setup wizard and are a more reliable signal than
-- sessions — an install may have agents but no sessions yet if no chat has occurred.
-- For fresh installs, setup_complete starts false so the wizard is required.
INSERT INTO system_flags (key, value)
SELECT
    'setup_complete',
    CASE
        WHEN EXISTS (SELECT 1 FROM providers LIMIT 1) THEN 'true'::jsonb
        ELSE 'false'::jsonb
    END
ON CONFLICT DO NOTHING;
