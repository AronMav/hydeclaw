-- Migration 016: Tool Execution Cache
-- Adds semantic caching for expensive tool results

CREATE TABLE IF NOT EXISTS tool_execution_cache (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tool_name TEXT NOT NULL,
    query_text TEXT NOT NULL,
    query_embedding VECTOR NOT NULL, -- dimension auto-detected at startup, matches embed_dim
    result_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

-- Semantic index created at startup after embed_dim is known (like memory_chunks)

CREATE INDEX IF NOT EXISTS idx_tool_cache_expiry ON tool_execution_cache (expires_at);
