CREATE TABLE users (
    id          TEXT PRIMARY KEY,
    handle      TEXT UNIQUE NOT NULL,
    token_hash  BYTEA NOT NULL UNIQUE,
    bio         TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE posts (
    id          TEXT PRIMARY KEY,
    author_id   TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    parent_id   TEXT REFERENCES posts(id) ON DELETE RESTRICT,
    content     TEXT NOT NULL CHECK (char_length(content) <= 512),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at  TIMESTAMPTZ
);

CREATE INDEX idx_posts_time ON posts (created_at DESC);
CREATE INDEX idx_posts_author_time ON posts (author_id, created_at DESC);
CREATE INDEX idx_posts_parent ON posts (parent_id, created_at ASC)
    WHERE parent_id IS NOT NULL;

CREATE TABLE likes (
    post_id    TEXT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (post_id, user_id)
);

CREATE TABLE refresh_tokens (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_refresh_user ON refresh_tokens (user_id);
