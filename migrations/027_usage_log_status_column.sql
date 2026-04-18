-- usage_log gains a nullable `status` column to persist call termination
-- reasons. Previously migration 025 documented the expected values but the
-- column itself was never created (latent bug — 025's `COMMENT ON COLUMN`
-- only succeeded on systems where the column happened to exist, and failed
-- silently on fresh installs).
--
-- Valid values (see db::usage constants):
--   NULL               — legacy / completed call (default for rows inserted
--                        via the old `record_usage` helper)
--   'completed'        — explicit success marker (optional)
--   'aborted'          — max_duration / user_cancelled / shutdown_drain
--   'aborted_failover' — partial content produced before failover to a
--                        sibling route

ALTER TABLE usage_log
    ADD COLUMN IF NOT EXISTS status TEXT;

-- Re-apply the comment from 025 now that the column actually exists. Using
-- IF EXISTS guard would be nicer but CREATE INDEX IF NOT EXISTS is the only
-- `IF NOT EXISTS` form supported here.
COMMENT ON COLUMN usage_log.status IS
    'Call status: NULL (legacy/completed), ''completed'', ''error'', ''aborted'' (max_duration / user_cancelled / shutdown_drain), ''aborted_failover'' (partial content produced before failover).';

-- 025 also declared this index; recreate defensively in case it failed.
CREATE INDEX IF NOT EXISTS idx_usage_log_status_aborted
    ON usage_log (created_at DESC)
    WHERE status LIKE 'aborted%';
