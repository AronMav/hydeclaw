-- 014_message_parts.sql
-- Add parts JSONB column to messages for storing finalized MessagePart[] array.
-- NULL during streaming and for pre-migration messages (frontend falls back to convertHistory).
ALTER TABLE messages ADD COLUMN parts JSONB;
