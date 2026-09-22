-- Phase C: follows, blocks, notifications, DMs, tags, edits, FTS

ALTER TABLE posts ADD COLUMN IF NOT EXISTS edited_at TIMESTAMPTZ;

ALTER TABLE posts ADD COLUMN IF NOT EXISTS content_tsv tsvector;

UPDATE posts SET content_tsv = to_tsvector('english', coalesce(content, ''))
WHERE content_tsv IS NULL;

CREATE INDEX IF NOT EXISTS idx_posts_fts ON posts USING GIN (content_tsv);

CREATE OR REPLACE FUNCTION posts_content_tsv_update() RETURNS trigger AS $$
BEGIN
    NEW.content_tsv := to_tsvector('english', coalesce(NEW.content, ''));
    RETURN NEW;
END
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS posts_tsv_trigger ON posts;
CREATE TRIGGER posts_tsv_trigger
    BEFORE INSERT OR UPDATE OF content ON posts
    FOR EACH ROW EXECUTE FUNCTION posts_content_tsv_update();

CREATE TABLE IF NOT EXISTS follows (
    follower_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    followee_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (follower_id, followee_id),
    CHECK (follower_id <> followee_id)
);
CREATE INDEX IF NOT EXISTS idx_follows_followee ON follows (followee_id);

CREATE TABLE IF NOT EXISTS blocks (
    blocker_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    blocked_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (blocker_id, blocked_id),
    CHECK (blocker_id <> blocked_id)
);
CREATE INDEX IF NOT EXISTS idx_blocks_blocked ON blocks (blocked_id);

CREATE TABLE IF NOT EXISTS notifications (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    actor_id   TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    post_id    TEXT REFERENCES posts(id) ON DELETE CASCADE,
    read_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_notifications_user_time
    ON notifications (user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS tags (
    name TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS post_tags (
    post_id TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    tag     TEXT NOT NULL REFERENCES tags(name) ON DELETE CASCADE,
    PRIMARY KEY (post_id, tag)
);
CREATE INDEX IF NOT EXISTS idx_post_tags_tag ON post_tags (tag);

CREATE TABLE IF NOT EXISTS dm_conversations (
    id         TEXT PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS dm_participants (
    conversation_id TEXT NOT NULL REFERENCES dm_conversations(id) ON DELETE CASCADE,
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (conversation_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_dm_participants_user ON dm_participants (user_id);

CREATE TABLE IF NOT EXISTS dm_messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES dm_conversations(id) ON DELETE CASCADE,
    sender_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    content         TEXT NOT NULL CHECK (char_length(content) <= 512),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_dm_messages_conv_time
    ON dm_messages (conversation_id, created_at ASC);
