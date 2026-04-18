-- Add abort_reason column for LlmCallError tracking.
-- NULL for non-aborted messages. Stable short identifiers:
--   'connect_timeout' | 'inactivity' | 'request_timeout' | 'max_duration'
--   | 'user_cancelled' | 'shutdown_drain'
-- Values pinned in LlmCallError::abort_reason() — changing them breaks history.

ALTER TABLE messages
    ADD COLUMN IF NOT EXISTS abort_reason TEXT;

COMMENT ON COLUMN messages.abort_reason IS
    'LLM call abort reason when status=aborted. NULL otherwise. Stable identifiers (pinned in LlmCallError::abort_reason): connect_timeout | inactivity | request_timeout | max_duration | user_cancelled | shutdown_drain. Changing these strings breaks historical rows.';
