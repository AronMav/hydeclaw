-- 018_drop_graph_and_session_docs.sql
-- Remove knowledge graph tables, extraction queue, and session documents.
-- These subsystems don't contribute to agent context building.
DROP TABLE IF EXISTS graph_episodes CASCADE;
DROP TABLE IF EXISTS graph_edges CASCADE;
DROP TABLE IF EXISTS graph_entities CASCADE;
DROP TABLE IF EXISTS graph_extraction_queue CASCADE;
DROP TABLE IF EXISTS session_documents CASCADE;
