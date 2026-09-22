//! Shared DB helpers for posts, blocks, tags, notifications.

use std::sync::Arc;

use crate::shared::{
    extract_hashtags, extract_mentions, new_id, Event, MediaInfo, Notification, NotificationKind,
    Post, QuotedPost, MEDIA_MAX_PER_POST,
};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::server::error::{ApiError, ApiResult};
use crate::server::state::AppState;

/// Row shape returned by post SELECT queries that include engagement stats.
pub type PostRow = (
    String,                // id
    String,                // author_id
    String,                // handle
    Option<String>,        // parent_id
    String,                // content
    DateTime<Utc>,         // created_at
    Option<DateTime<Utc>>, // deleted_at
    Option<DateTime<Utc>>, // edited_at
    i64,                   // like_count
    bool,                  // liked_by_me
    i64,                   // reply_count
    Option<String>,
);

pub const POST_SELECT: &str = r#"
    SELECT
        p.id,
        p.author_id,
        u.handle,
        p.parent_id,
        CASE WHEN p.deleted_at IS NOT NULL THEN '[Deleted]' ELSE p.content END AS content,
        p.created_at,
        p.deleted_at,
        p.edited_at,
        (SELECT COUNT(*)::bigint FROM likes l WHERE l.post_id = p.id) AS like_count,
        EXISTS(
            SELECT 1 FROM likes l WHERE l.post_id = p.id AND l.user_id = $viewer
        ) AS liked_by_me,
        (SELECT COUNT(*)::bigint FROM posts c WHERE c.parent_id = p.id) AS reply_count,
        p.quote_id
    FROM posts p
    JOIN users u ON u.id = p.author_id
"#;

/// SQL fragment: hide authors that either side has blocked.
pub const NOT_BLOCKED: &str = r#"
    AND NOT EXISTS (
        SELECT 1 FROM blocks b
        WHERE (b.blocker_id = $viewer_user AND b.blocked_id = p.author_id)
           OR (b.blocker_id = p.author_id AND b.blocked_id = $viewer_user)
    )
"#;

pub fn row_to_post(r: PostRow) -> Post {
    Post {
        id: r.0,
        author_id: r.1,
        author_handle: r.2,
        parent_id: r.3,
        quote_id: r.11,
        quote: None,
        content: r.4,
        created_at: r.5,
        deleted_at: r.6,
        edited_at: r.7,
        like_count: r.8,
        liked_by_me: r.9,
        reply_count: r.10,
        images: Vec::new(),
    }
}

/// Attach `images` on each post from `post_media` (ordered). Tombstones get empty images.
pub async fn attach_media(pool: &PgPool, posts: &mut [Post]) -> ApiResult<()> {
    if posts.is_empty() {
        return Ok(());
    }
    let ids: Vec<String> = posts
        .iter()
        .filter(|p| p.deleted_at.is_none())
        .map(|p| p.id.clone())
        .collect();
    if ids.is_empty() {
        return Ok(());
    }

    let rows: Vec<(String, String, String, i64, Option<i32>, Option<i32>, i16)> = sqlx::query_as(
        r#"
            SELECT pm.post_id, m.id, m.content_type, m.size_bytes, m.width, m.height, pm.position
            FROM post_media pm
            JOIN media m ON m.id = pm.media_id
            WHERE pm.post_id = ANY($1)
            ORDER BY pm.post_id, pm.position ASC, m.id ASC
            "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;

    use std::collections::HashMap;
    let mut by_post: HashMap<String, Vec<MediaInfo>> = HashMap::new();
    for (post_id, id, content_type, size_bytes, width, height, _pos) in rows {
        by_post.entry(post_id).or_default().push(MediaInfo {
            id,
            content_type,
            size_bytes,
            width,
            height,
        });
    }
    for p in posts.iter_mut() {
        if p.deleted_at.is_some() {
            p.images.clear();
            continue;
        }
        if let Some(imgs) = by_post.remove(&p.id) {
            p.images = imgs;
        }
    }
    Ok(())
}

pub async fn hydrate_posts(pool: &PgPool, posts: &mut [Post], viewer_id: &str) -> ApiResult<()> {
    attach_media(pool, posts).await?;
    let ids: Vec<String> = posts
        .iter_mut()
        .filter_map(|post| {
            post.quote = None;
            if post.deleted_at.is_none() {
                post.quote_id.clone()
            } else {
                None
            }
        })
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let sql = format!("{POST_SELECT} WHERE p.id = ANY($1) AND p.deleted_at IS NULL {NOT_BLOCKED}")
        .replace("$viewer_user", "$2")
        .replace("$viewer", "$2");
    let rows: Vec<PostRow> = sqlx::query_as(sqlx::AssertSqlSafe(&*sql))
        .bind(&ids)
        .bind(viewer_id)
        .fetch_all(pool)
        .await?;
    let mut originals: Vec<Post> = rows.into_iter().map(row_to_post).collect();
    attach_media(pool, &mut originals).await?;
    let quotes: std::collections::HashMap<String, QuotedPost> = originals
        .into_iter()
        .map(|post| {
            let quote = QuotedPost {
                id: post.id,
                author_handle: post.author_handle,
                content: post.content,
                deleted_at: post.deleted_at,
                images: post.images,
            };
            (quote.id.clone(), quote)
        })
        .collect();
    for post in posts {
        if post.deleted_at.is_none() {
            post.quote = post
                .quote_id
                .as_ref()
                .and_then(|id| quotes.get(id))
                .cloned();
        }
    }
    Ok(())
}

pub async fn fetch_post(pool: &PgPool, post_id: &str, viewer_id: &str) -> ApiResult<Option<Post>> {
    let sql = POST_SELECT.replace("$viewer", "$2") + " WHERE p.id = $1";
    let row: Option<PostRow> = sqlx::query_as(sqlx::AssertSqlSafe(&*sql))
        .bind(post_id)
        .bind(viewer_id)
        .fetch_optional(pool)
        .await?;
    let mut post = match row.map(row_to_post) {
        Some(p) => p,
        None => return Ok(None),
    };
    hydrate_posts(pool, std::slice::from_mut(&mut post), viewer_id).await?;
    Ok(Some(post))
}

/// Bind uploaded media owned by `user_id` to a new post. Validates ownership and caps.
pub async fn link_post_media(
    pool: &PgPool,
    post_id: &str,
    user_id: &str,
    image_ids: &[String],
) -> ApiResult<()> {
    if image_ids.is_empty() {
        return Ok(());
    }
    if image_ids.len() > MEDIA_MAX_PER_POST {
        return Err(ApiError::bad_request(format!(
            "too many images (max {MEDIA_MAX_PER_POST})"
        )));
    }

    // Dedupe while preserving order.
    let mut seen = std::collections::HashSet::new();
    let mut ordered: Vec<String> = Vec::new();
    for id in image_ids {
        let id = id.trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        ordered.push(id.to_string());
    }
    if ordered.len() > MEDIA_MAX_PER_POST {
        return Err(ApiError::bad_request(format!(
            "too many images (max {MEDIA_MAX_PER_POST})"
        )));
    }

    for (pos, media_id) in ordered.iter().enumerate() {
        let owned: Option<(String,)> = sqlx::query_as(
            r#"
            SELECT id FROM media
            WHERE id = $1 AND uploader_id = $2
              AND NOT EXISTS (SELECT 1 FROM post_media WHERE media_id = $1)
            "#,
        )
        .bind(media_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
        if owned.is_none() {
            return Err(ApiError::bad_request(format!(
                "image {media_id} not found, not yours, or already attached"
            )));
        }
        sqlx::query(
            r#"
            INSERT INTO post_media (post_id, media_id, position)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(post_id)
        .bind(media_id)
        .bind(pos as i16)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn is_blocked_either(pool: &PgPool, a: &str, b: &str) -> ApiResult<bool> {
    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint FROM blocks
        WHERE (blocker_id = $1 AND blocked_id = $2)
           OR (blocker_id = $2 AND blocked_id = $1)
        "#,
    )
    .bind(a)
    .bind(b)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

pub async fn sync_post_tags(pool: &PgPool, post_id: &str, content: &str) -> ApiResult<()> {
    sqlx::query(r#"DELETE FROM post_tags WHERE post_id = $1"#)
        .bind(post_id)
        .execute(pool)
        .await?;
    for tag in extract_hashtags(content) {
        sqlx::query(r#"INSERT INTO tags (name) VALUES ($1) ON CONFLICT DO NOTHING"#)
            .bind(&tag)
            .execute(pool)
            .await?;
        sqlx::query(
            r#"INSERT INTO post_tags (post_id, tag) VALUES ($1, $2) ON CONFLICT DO NOTHING"#,
        )
        .bind(post_id)
        .bind(&tag)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn notify(
    state: &AppState,
    recipient_id: &str,
    actor_id: &str,
    kind: NotificationKind,
    post_id: Option<&str>,
) -> ApiResult<()> {
    if recipient_id == actor_id {
        return Ok(());
    }
    if is_blocked_either(&state.db, recipient_id, actor_id).await? {
        return Ok(());
    }

    let id = new_id();
    sqlx::query(
        r#"
        INSERT INTO notifications (id, user_id, actor_id, kind, post_id)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(&id)
    .bind(recipient_id)
    .bind(actor_id)
    .bind(kind.as_str())
    .bind(post_id)
    .execute(&state.db)
    .await?;

    let actor_handle: String = sqlx::query_scalar(r#"SELECT handle FROM users WHERE id = $1"#)
        .bind(actor_id)
        .fetch_one(&state.db)
        .await?;

    let notification = Notification {
        id,
        user_id: recipient_id.to_string(),
        kind: kind.as_str().to_string(),
        actor_handle,
        post_id: post_id.map(|s| s.to_string()),
        read: false,
        created_at: Utc::now(),
    };

    let _ = state
        .broadcast_tx
        .send(Arc::new(Event::Notification { notification }));
    Ok(())
}

/// Notify newly mentioned handles only (skips handles already in `previous_content`).
pub async fn notify_mentions(
    state: &AppState,
    actor_id: &str,
    content: &str,
    post_id: &str,
    previous_content: Option<&str>,
) -> ApiResult<()> {
    let prev: Vec<String> = previous_content.map(extract_mentions).unwrap_or_default();
    for handle in extract_mentions(content) {
        if prev.iter().any(|p| p == &handle) {
            continue;
        }
        let uid: Option<String> = sqlx::query_scalar(r#"SELECT id FROM users WHERE handle = $1"#)
            .bind(&handle)
            .fetch_optional(&state.db)
            .await?;
        if let Some(uid) = uid {
            notify(
                state,
                &uid,
                actor_id,
                NotificationKind::Mention,
                Some(post_id),
            )
            .await?;
        }
    }
    Ok(())
}

pub async fn rate_limit_posts(pool: &PgPool, user_id: &str) -> ApiResult<()> {
    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint FROM posts
        WHERE author_id = $1 AND created_at > NOW() - INTERVAL '1 hour'
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    if n >= 60 {
        return Err(ApiError::bad_request("rate limit: max 60 posts per hour"));
    }
    Ok(())
}

pub fn preview_content(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.chars().count() <= max {
        return flat;
    }
    let truncated: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{truncated}…")
}
