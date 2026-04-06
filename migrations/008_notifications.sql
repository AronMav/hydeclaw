-- 008_notifications: real-time notification store for Web UI
-- Consumed by GET /api/notifications, PATCH /api/notifications/{id}, POST /api/notifications/read-all
-- and by system triggers (access requests, tool approvals, agent errors, watchdog alerts).

CREATE TABLE IF NOT EXISTS notifications (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    type        TEXT        NOT NULL,
    title       TEXT        NOT NULL,
    body        TEXT        NOT NULL DEFAULT '',
    data        JSONB       NOT NULL DEFAULT '{}'::jsonb,
    read        BOOL        NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS notifications_read_created_at
    ON notifications (read, created_at DESC);
