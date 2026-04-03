ALTER TABLE sessions ADD COLUMN IF NOT EXISTS participants TEXT[] NOT NULL DEFAULT '{}';

-- Backfill existing sessions: set participants = [agent_id]
UPDATE sessions SET participants = ARRAY[agent_id] WHERE participants = '{}';
