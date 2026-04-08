-- Migration 011: Switch messages FTS from 'simple' to 'russian' dictionary
-- Purpose: Enable morphological stemming for Russian-language content.
-- The existing tsv column is a GENERATED ALWAYS AS column (simple dictionary).
-- PostgreSQL cannot ALTER a generated column's expression, so we must drop
-- and re-add it as a plain column, then maintain it via trigger.

-- Step 1: Drop the existing GIN index
DROP INDEX IF EXISTS idx_messages_tsv;

-- Step 2: Drop the GENERATED ALWAYS AS column
ALTER TABLE messages DROP COLUMN tsv;

-- Step 3: Add tsv back as a plain tsvector column
ALTER TABLE messages ADD COLUMN tsv tsvector;

-- Step 4: Create the trigger function for auto-maintaining tsv
CREATE OR REPLACE FUNCTION trg_messages_tsv()
RETURNS TRIGGER AS $$
BEGIN
    NEW.tsv := to_tsvector('russian', COALESCE(NEW.content, ''));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Step 5: Create the trigger on messages table
CREATE TRIGGER trg_messages_tsv_update
    BEFORE INSERT OR UPDATE OF content ON messages
    FOR EACH ROW EXECUTE FUNCTION trg_messages_tsv();

-- Step 6: Backfill all existing rows
UPDATE messages SET tsv = to_tsvector('russian', COALESCE(content, ''));

-- Step 7: Recreate the GIN index
CREATE INDEX idx_messages_tsv ON messages USING gin(tsv);
