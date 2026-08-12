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
        Self::from_conn(conn)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
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
                reply_count INTEGER NOT NULL DEFAULT 0,
                liked_by_me INTEGER NOT NULL DEFAULT 0
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
        let _ = conn.execute(
            "ALTER TABLE posts ADD COLUMN liked_by_me INTEGER NOT NULL DEFAULT 0",
            [],
        );
        Ok(Self { conn })
    }

    pub fn upsert_posts(&self, posts: &[Post]) -> Result<()> {
        let mut stmt = self.conn.prepare(
            r#"INSERT INTO posts (
                 id, author_id, author_handle, parent_id, content,
                 created_at, deleted_at, edited_at, like_count, reply_count, liked_by_me
               )
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
               ON CONFLICT(id) DO UPDATE SET
                 content=excluded.content,
                 deleted_at=excluded.deleted_at,
                 edited_at=excluded.edited_at,
                 like_count=excluded.like_count,
                 reply_count=excluded.reply_count,
                 liked_by_me=excluded.liked_by_me"#,
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
                if p.liked_by_me { 1 } else { 0 },
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
                      created_at, deleted_at, edited_at, like_count, reply_count, liked_by_me
               FROM posts
               WHERE parent_id IS NULL
               ORDER BY created_at DESC
               LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![limit as i64], map_post)?;
        collect_posts(rows)
    }

    /// Cached posts by a single author (any depth), newest first.
    pub fn by_author(&self, handle: &str, limit: usize) -> Result<Vec<Post>> {
        let handle = handle.to_ascii_lowercase();
        let mut stmt = self.conn.prepare(
            r#"SELECT id, author_id, author_handle, parent_id, content,
                      created_at, deleted_at, edited_at, like_count, reply_count, liked_by_me
               FROM posts
               WHERE lower(author_handle) = ?1
               ORDER BY created_at DESC
               LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![handle, limit as i64], map_post)?;
        collect_posts(rows)
    }

    pub fn get(&self, id: &str) -> Result<Option<Post>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, author_id, author_handle, parent_id, content,
                      created_at, deleted_at, edited_at, like_count, reply_count, liked_by_me
               FROM posts WHERE id = ?1"#,
        )?;
        let mut rows = stmt.query_map(params![id], map_post)?;
        Ok(rows.next().transpose()?)
    }

    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM posts", [])?;
        Ok(())
    }
}

fn map_post(row: &rusqlite::Row<'_>) -> rusqlite::Result<Post> {
    let created: String = row.get(5)?;
    let deleted: Option<String> = row.get(6)?;
    let edited: Option<String> = row.get(7)?;
    let liked: i64 = row.get(10).unwrap_or(0);
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
        liked_by_me: liked != 0,
        reply_count: row.get(9)?,
    })
}

fn collect_posts(
    rows: impl Iterator<Item = rusqlite::Result<Post>>,
) -> Result<Vec<Post>> {
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample(liked: bool) -> Post {
        Post {
            id: "p1".into(),
            author_id: "u1".into(),
            author_handle: "alice".into(),
            parent_id: None,
            content: "hello".into(),
            created_at: Utc::now(),
            deleted_at: None,
            edited_at: None,
            like_count: 3,
            liked_by_me: liked,
            reply_count: 0,
        }
    }

    #[test]
    fn persists_liked_by_me() {
        let cache = Cache::from_conn(Connection::open_in_memory().unwrap()).unwrap();
        cache.upsert_posts(&[sample(true)]).unwrap();
        let got = cache.get("p1").unwrap().unwrap();
        assert!(got.liked_by_me);
        assert_eq!(got.like_count, 3);
        cache.upsert_posts(&[sample(false)]).unwrap();
        let got = cache.get("p1").unwrap().unwrap();
        assert!(!got.liked_by_me);
    }
}
