-- Persist pairing codes in DB so they survive core restarts.
CREATE TABLE IF NOT EXISTS pairing_codes (
    code TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    channel_user_id TEXT NOT NULL,
    display_name TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_id, code)
);

-- Auto-expire old codes (cleanup query, not automatic — run periodically or on access)
CREATE INDEX IF NOT EXISTS idx_pairing_codes_agent ON pairing_codes(agent_id);
