-- Image uploads attached to posts

CREATE TABLE IF NOT EXISTS media (
    id           TEXT PRIMARY KEY,
    uploader_id  TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    content_type TEXT NOT NULL,
    size_bytes   BIGINT NOT NULL,
    width        INT,
    height       INT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_media_uploader ON media (uploader_id, created_at DESC);

CREATE TABLE IF NOT EXISTS post_media (
    post_id  TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    media_id TEXT NOT NULL REFERENCES media(id) ON DELETE RESTRICT,
    position SMALLINT NOT NULL DEFAULT 0,
    PRIMARY KEY (post_id, media_id)
);

CREATE INDEX IF NOT EXISTS idx_post_media_media ON post_media (media_id);
