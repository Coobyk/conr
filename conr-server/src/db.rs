//! Shared DB helpers for posts, blocks, tags, notifications.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use conr_shared::{
    extract_hashtags, extract_mentions, new_id, Event, Notification, NotificationKind, Post,
};
use sqlx::PgPool;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Row shape returned by post SELECT queries that include engagement stats.
pub type PostRow = (
    String,                 // id
    String,                 // author_id
    String,                 // handle
    Option<String>,         // parent_id
    String,                 // content
    DateTime<Utc>,          // created_at
    Option<DateTime<Utc>>,  // deleted_at
    Option<DateTime<Utc>>,  // edited_at
    i64,                    // like_count
    bool,                   // liked_by_me
    i64,                    // reply_count
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
        (SELECT COUNT(*)::bigint FROM posts c WHERE c.parent_id = p.id) AS reply_count
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
        content: r.4,
        created_at: r.5,
        deleted_at: r.6,
        edited_at: r.7,
        like_count: r.8,
        liked_by_me: r.9,
        reply_count: r.10,
    }
}

pub async fn fetch_post(
    pool: &PgPool,
    post_id: &str,
    viewer_id: &str,
) -> ApiResult<Option<Post>> {
    let sql = POST_SELECT.replace("$viewer", "$2") + " WHERE p.id = $1";
    let row: Option<PostRow> = sqlx::query_as(&sql)
        .bind(post_id)
        .bind(viewer_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(row_to_post))
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
    let prev: Vec<String> = previous_content
        .map(extract_mentions)
        .unwrap_or_default();
    for handle in extract_mentions(content) {
        if prev.iter().any(|p| p == &handle) {
            continue;
        }
        let uid: Option<String> =
            sqlx::query_scalar(r#"SELECT id FROM users WHERE handle = $1"#)
                .bind(&handle)
                .fetch_optional(&state.db)
                .await?;
        if let Some(uid) = uid {
            notify(state, &uid, actor_id, NotificationKind::Mention, Some(post_id)).await?;
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
        return Err(ApiError::bad_request(
            "rate limit: max 60 posts per hour",
        ));
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
