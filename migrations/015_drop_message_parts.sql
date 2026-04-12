-- 015_drop_message_parts.sql
-- Remove unused parts JSONB column from messages.
-- Frontend reconstructs parts from content via parseContentParts() — no backend parts needed.
ALTER TABLE messages DROP COLUMN IF EXISTS parts;
