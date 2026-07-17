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
                deleted_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_cache_time ON posts(created_at DESC);
            "#,
        )?;
        Ok(Self { conn })
    }

    pub fn upsert_posts(&self, posts: &[Post]) -> Result<()> {
        let mut stmt = self.conn.prepare(
            r#"INSERT INTO posts (id, author_id, author_handle, parent_id, content, created_at, deleted_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(id) DO UPDATE SET
                 content=excluded.content,
                 deleted_at=excluded.deleted_at"#,
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
            ])?;
        }
        // keep last 500
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
            r#"SELECT id, author_id, author_handle, parent_id, content, created_at, deleted_at
               FROM posts
               WHERE parent_id IS NULL
               ORDER BY created_at DESC
               LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let created: String = row.get(5)?;
            let deleted: Option<String> = row.get(6)?;
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
