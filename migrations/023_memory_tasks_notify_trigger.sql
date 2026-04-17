-- Phase 66 REF-04: LISTEN/NOTIFY for memory_tasks pickup.
-- AFTER INSERT trigger fires pg_notify on every committed insert so the
-- memory worker can react in sub-100ms instead of waiting up to
-- poll_interval_secs. pg_notify is transaction-scoped and delivered at
-- COMMIT, so rolled-back inserts never produce spurious wake-ups.
--
-- Idempotent by construction (CREATE OR REPLACE FUNCTION + DROP TRIGGER IF
-- EXISTS) so sqlx auto-run migrations on every startup are safe.

CREATE OR REPLACE FUNCTION notify_memory_task_new() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('memory_tasks_new', NEW.id::text);
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS memory_tasks_notify ON memory_tasks;
CREATE TRIGGER memory_tasks_notify
AFTER INSERT ON memory_tasks
FOR EACH ROW EXECUTE FUNCTION notify_memory_task_new();
