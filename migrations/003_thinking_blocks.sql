-- migrations/003_thinking_blocks.sql
ALTER TABLE messages ADD COLUMN IF NOT EXISTS thinking_blocks JSONB DEFAULT NULL;
