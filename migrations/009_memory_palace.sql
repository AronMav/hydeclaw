-- Memory Palace: schema foundation for phases 13-16
-- Adds category/topic/archived to memory_chunks, valid_from/valid_to to graph_edges

-- 1. memory_chunks: new columns
ALTER TABLE memory_chunks ADD COLUMN IF NOT EXISTS category TEXT DEFAULT NULL;
ALTER TABLE memory_chunks ADD COLUMN IF NOT EXISTS topic TEXT DEFAULT NULL;
ALTER TABLE memory_chunks ADD COLUMN IF NOT EXISTS archived BOOLEAN NOT NULL DEFAULT false;

-- 2. graph_edges: temporal TIMESTAMPTZ columns (do NOT reuse existing TEXT valid_at/invalid_at)
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS valid_from TIMESTAMPTZ DEFAULT now();
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS valid_to TIMESTAMPTZ DEFAULT NULL;

-- 3. Composite B-tree index for hierarchical queries (Phase 14)
CREATE INDEX IF NOT EXISTS idx_memory_category_topic ON memory_chunks(agent_id, category, topic);

-- 4. Index for L0 pinned loading: fast fetch of pinned non-archived chunks per agent
CREATE INDEX IF NOT EXISTS idx_memory_pinned_agent ON memory_chunks(agent_id, pinned, archived) WHERE pinned = true AND archived = false;
