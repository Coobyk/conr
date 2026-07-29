use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use conr_shared::Post;
use rusqlite::{params, Connection};

use crate::config::data_dir;

pub struct Cache {
    conn: Connection,
}

impl Cache {
    pub fn open() -> Result<Self> {
        let path = data_dir()?.join("cache.db");
        let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS posts (
                id TEXT PRIMARY KEY,
                author_id TEXT NOT NULL,
                author_handle TEXT NOT NULL,
                parent_id TEXT,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                deleted_at TEXT,
                edited_at TEXT,
                like_count INTEGER NOT NULL DEFAULT 0,
                reply_count INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_cache_time ON posts(created_at DESC);
            "#,
        )?;
        // Migrate older caches that lack newer columns.
        let _ = conn.execute(
            "ALTER TABLE posts ADD COLUMN like_count INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE posts ADD COLUMN reply_count INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute("ALTER TABLE posts ADD COLUMN edited_at TEXT", []);
        Ok(Self { conn })
    }

    pub fn upsert_posts(&self, posts: &[Post]) -> Result<()> {
        let mut stmt = self.conn.prepare(
            r#"INSERT INTO posts (
                 id, author_id, author_handle, parent_id, content,
                 created_at, deleted_at, edited_at, like_count, reply_count
               )
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
               ON CONFLICT(id) DO UPDATE SET
                 content=excluded.content,
                 deleted_at=excluded.deleted_at,
                 edited_at=excluded.edited_at,
                 like_count=excluded.like_count,
                 reply_count=excluded.reply_count"#,
        )?;
        for p in posts {
            stmt.execute(params![
                p.id,
                p.author_id,
                p.author_handle,
                p.parent_id,
                p.content,
                p.created_at.to_rfc3339(),
                p.deleted_at.map(|d| d.to_rfc3339()),
                p.edited_at.map(|d| d.to_rfc3339()),
                p.like_count,
                p.reply_count,
            ])?;
        }
        // keep last 500 top-level-ish by recency
        self.conn.execute(
            r#"DELETE FROM posts WHERE id NOT IN (
                SELECT id FROM posts ORDER BY created_at DESC LIMIT 500
            )"#,
            [],
        )?;
        Ok(())
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<Post>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, author_id, author_handle, parent_id, content,
                      created_at, deleted_at, edited_at, like_count, reply_count
               FROM posts
               WHERE parent_id IS NULL
               ORDER BY created_at DESC
               LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let created: String = row.get(5)?;
            let deleted: Option<String> = row.get(6)?;
            let edited: Option<String> = row.get(7)?;
            Ok(Post {
                id: row.get(0)?,
                author_id: row.get(1)?,
                author_handle: row.get(2)?,
                parent_id: row.get(3)?,
                content: row.get(4)?,
                created_at: DateTime::parse_from_rfc3339(&created)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                deleted_at: deleted.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                edited_at: edited.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                like_count: row.get(8)?,
                liked_by_me: false, // not cached
                reply_count: row.get(9)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM posts", [])?;
        Ok(())
    }
}
