-- Temporal graph edges: replace old UNIQUE constraint with partial unique index on active edges.
-- Phase 15: temporal knowledge graph (valid_from/valid_to already added in migration 009).

-- 1. Drop the old table-level UNIQUE constraint on (source_id, target_id, relation_type).
--    PostgreSQL auto-named it using the convention below.
ALTER TABLE graph_edges
    DROP CONSTRAINT IF EXISTS graph_edges_source_id_target_id_relation_type_key;

-- 2. Partial unique index: enforce uniqueness only among ACTIVE edges (valid_to IS NULL).
--    Allows expired edges to coexist with newer active edges for the same triple.
CREATE UNIQUE INDEX IF NOT EXISTS idx_graph_edges_active_unique
    ON graph_edges (source_id, target_id, relation_type)
    WHERE valid_to IS NULL;

-- 3. Index on valid_to for efficient temporal filtering in queries.
CREATE INDEX IF NOT EXISTS idx_graph_edges_valid_to
    ON graph_edges (valid_to)
    WHERE valid_to IS NULL;
