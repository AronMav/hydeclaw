-- Add recall_count to memory_chunks for tracking access frequency (dreaming feature).
-- Frequently-recalled raw memories get promoted to pinned tier by the dreaming cron job.
ALTER TABLE memory_chunks ADD COLUMN IF NOT EXISTS recall_count INTEGER NOT NULL DEFAULT 0;
