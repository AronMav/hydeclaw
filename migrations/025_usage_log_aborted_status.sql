-- usage_log.status gains 'aborted' and 'aborted_failover' as valid values.
-- Table uses TEXT column (no enum) so the change is a pure convention
-- update — we pin it here so readers know what to expect.

COMMENT ON COLUMN usage_log.status IS
    'Call status: ''completed'', ''error'', ''aborted'' (max_duration / user_cancelled / shutdown_drain), ''aborted_failover'' (partial content produced before failover).';

-- Index to accelerate "recent aborts" dashboards without full-table scans.
CREATE INDEX IF NOT EXISTS idx_usage_log_status_aborted
    ON usage_log (created_at DESC)
    WHERE status LIKE 'aborted%';
