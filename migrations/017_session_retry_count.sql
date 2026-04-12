-- 017_session_retry_count.sql
-- Track how many times a stuck session has been auto-retried by watchdog.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS retry_count INTEGER NOT NULL DEFAULT 0;
