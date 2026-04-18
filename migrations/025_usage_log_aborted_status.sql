-- usage_log gains a nullable `status` column to persist call termination
-- reasons. Column is added BEFORE the COMMENT/INDEX so the migration is
-- self-sufficient (previous version required a separate follow-up migration
-- that would only run after 025 had already attempted to reference a
-- non-existent column — that failure mode is fixed here).
--
-- Valid values (pinned in `db::usage::STATUS_*` constants):
--   NULL               — legacy / completed call (default for rows inserted
--                        via the old `record_usage` helper)
--   'completed'        — explicit success marker (optional)
--   'aborted'          — max_duration / user_cancelled / shutdown_drain
--   'aborted_failover' — partial content produced before failover to a
--                        sibling route

ALTER TABLE usage_log
    ADD COLUMN IF NOT EXISTS status TEXT;

COMMENT ON COLUMN usage_log.status IS
    'Call status: NULL (legacy/completed), ''completed'', ''error'', ''aborted'' (max_duration / user_cancelled / shutdown_drain), ''aborted_failover'' (partial content produced before failover).';

-- Partial index accelerates "recent aborts" dashboards without full-table
-- scans. Covers both 'aborted' and 'aborted_failover'.
CREATE INDEX IF NOT EXISTS idx_usage_log_status_aborted
    ON usage_log (created_at DESC)
    WHERE status LIKE 'aborted%';
