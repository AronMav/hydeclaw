-- Drop unused user_id column (always empty string since agent-scoped memory)
DROP INDEX IF EXISTS idx_memory_user;
ALTER TABLE memory_chunks DROP COLUMN IF EXISTS user_id;

-- Composite index for common search patterns: (agent_id, scope, pinned)
CREATE INDEX IF NOT EXISTS idx_memory_agent_scope
    ON memory_chunks(agent_id, scope, pinned, created_at DESC);
