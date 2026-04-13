-- 019_memory_scope.sql
-- Add scope column for shared memory between agents.
-- 'private' = agent-only (default), 'shared' = visible to all agents.
ALTER TABLE memory_chunks ADD COLUMN IF NOT EXISTS scope TEXT NOT NULL DEFAULT 'private';
