ALTER TABLE posts
    ADD COLUMN quote_id TEXT REFERENCES posts(id) ON DELETE RESTRICT,
    ADD CONSTRAINT posts_parent_or_quote CHECK (parent_id IS NULL OR quote_id IS NULL);

CREATE INDEX idx_posts_quote ON posts (quote_id) WHERE quote_id IS NOT NULL;
