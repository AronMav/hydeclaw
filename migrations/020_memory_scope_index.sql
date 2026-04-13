-- 020_memory_scope_index.sql
-- Add index for scope-based memory filtering (Performance boost for CTX-04/05)
CREATE INDEX IF NOT EXISTS idx_memory_chunks_scope_agent ON memory_chunks (scope, agent_id);
CREATE INDEX IF NOT EXISTS idx_memory_chunks_archived ON memory_chunks (archived) WHERE archived = true;
