-- Per-user last-read watermark for DM conversations.
CREATE TABLE IF NOT EXISTS dm_reads (
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES dm_conversations(id) ON DELETE CASCADE,
    last_read_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, conversation_id)
);
CREATE INDEX IF NOT EXISTS idx_dm_reads_user ON dm_reads (user_id);

-- Existing threads are treated as already read so a deploy does not
-- light up every old conversation.
INSERT INTO dm_reads (user_id, conversation_id, last_read_at)
SELECT p.user_id, p.conversation_id, NOW()
FROM dm_participants p
ON CONFLICT DO NOTHING;
