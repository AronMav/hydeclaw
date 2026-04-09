-- Migration 012: Add message branching support (parent tracking + fork points)
-- Purpose: Enable conversation forking — users can edit a past message and create
-- an alternative branch without losing the original conversation path.

-- ROLLBACK:
-- DROP INDEX IF EXISTS idx_messages_branch_from;
-- DROP INDEX IF EXISTS idx_messages_parent;
-- ALTER TABLE messages DROP COLUMN IF EXISTS branch_from_message_id;
-- ALTER TABLE messages DROP COLUMN IF EXISTS parent_message_id;

-- parent_message_id: the message that precedes this one in conversation thread. NULL for trunk messages.
ALTER TABLE messages ADD COLUMN IF NOT EXISTS parent_message_id UUID REFERENCES messages(id) ON DELETE SET NULL;

-- branch_from_message_id: set on fork-point messages, points to the message being replaced/edited
ALTER TABLE messages ADD COLUMN IF NOT EXISTS branch_from_message_id UUID REFERENCES messages(id) ON DELETE SET NULL;

-- Index for finding children of a message (sibling navigation)
CREATE INDEX IF NOT EXISTS idx_messages_parent ON messages(parent_message_id) WHERE parent_message_id IS NOT NULL;

-- Index for finding siblings at fork point
CREATE INDEX IF NOT EXISTS idx_messages_branch_from ON messages(branch_from_message_id) WHERE branch_from_message_id IS NOT NULL;
