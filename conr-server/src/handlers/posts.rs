use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::Utc;
use serde::Deserialize;

use conr_shared::{
    new_id, validate_content, CreatePostRequest, EditPostRequest, Event, FeedResponse,
    LikeResponse, NotificationKind, Post, ThreadResponse,
};

use crate::auth::AuthUser;
use crate::db::{
    fetch_post, notify, notify_mentions, preview_content, rate_limit_posts, row_to_post,
    sync_post_tags, NOT_BLOCKED, POST_SELECT, PostRow,
};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    /// Cursor: fetch posts older than this post id.
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    /// `all` (default) or `following`
    pub filter: Option<String>,
}

pub async fn feed(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<FeedQuery>,
) -> ApiResult<Json<FeedResponse>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let following_only = q
        .filter
        .as_deref()
        .map(|f| f.eq_ignore_ascii_case("following"))
        .unwrap_or(false);

    let following_clause = if following_only {
        r#"
        AND (
            p.author_id = $viewer_user
            OR EXISTS (
                SELECT 1 FROM follows f
                WHERE f.follower_id = $viewer_user AND f.followee_id = p.author_id
            )
        )
        "#
    } else {
        ""
    };

    let blocked = NOT_BLOCKED;

    let rows: Vec<PostRow> = if let Some(cursor) = q.cursor.as_ref() {
        let sql = format!(
            r#"
            {POST_SELECT}
            WHERE p.parent_id IS NULL
              AND (p.created_at, p.id) < (
                    SELECT created_at, id FROM posts WHERE id = $1
                  )
              {following_clause}
              {blocked}
            ORDER BY p.created_at DESC, p.id DESC
            LIMIT $3
            "#
        )
        .replace("$viewer_user", "$2")
        .replace("$viewer", "$2");
        sqlx::query_as(&sql)
            .bind(cursor)
            .bind(&user.id)
            .bind(limit)
            .fetch_all(&state.db)
            .await?
    } else {
        let sql = format!(
            r#"
            {POST_SELECT}
            WHERE p.parent_id IS NULL
              {following_clause}
              {blocked}
            ORDER BY p.created_at DESC, p.id DESC
            LIMIT $2
            "#
        )
        .replace("$viewer_user", "$1")
        .replace("$viewer", "$1");
        sqlx::query_as(&sql)
            .bind(&user.id)
            .bind(limit)
            .fetch_all(&state.db)
            .await?
    };

    let posts: Vec<Post> = rows.into_iter().map(row_to_post).collect();
    let next_cursor = posts.last().map(|p| p.id.clone());
    let count = posts.len();

    tracing::info!(
        "feed      @{}  loaded {} post(s)  filter={}  limit={}  cursor={}",
        user.handle,
        count,
        if following_only { "following" } else { "all" },
        limit,
        q.cursor.as_deref().unwrap_or("-"),
    );

    Ok(Json(FeedResponse {
        posts,
        next_cursor,
    }))
}

pub async fn create_post(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreatePostRequest>,
) -> ApiResult<Json<Post>> {
    let content =
        validate_content(&body.content).map_err(|e| ApiError::bad_request(e.to_string()))?;
    rate_limit_posts(&state.db, &user.id).await?;

    let mut parent_author: Option<String> = None;
    if let Some(ref parent_id) = body.parent_id {
        let parent: Option<(String, String)> =
            sqlx::query_as(r#"SELECT id, author_id FROM posts WHERE id = $1"#)
                .bind(parent_id)
                .fetch_optional(&state.db)
                .await?;
        let Some((pid, author_id)) = parent else {
            return Err(ApiError::not_found("parent post not found"));
        };
        if crate::db::is_blocked_either(&state.db, &user.id, &author_id).await? {
            return Err(ApiError::forbidden("cannot reply: blocked"));
        }
        let _ = pid;
        parent_author = Some(author_id);
    }

    let id = new_id();

    sqlx::query(
        r#"
        INSERT INTO posts (id, author_id, parent_id, content)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&body.parent_id)
    .bind(&content)
    .execute(&state.db)
    .await?;

    sync_post_tags(&state.db, &id, &content).await?;
    notify_mentions(&state, &user.id, &content, &id, None).await?;

    if let Some(parent_author) = parent_author {
        notify(
            &state,
            &parent_author,
            &user.id,
            NotificationKind::Reply,
            Some(&id),
        )
        .await?;
    }

    let post = fetch_post(&state.db, &id, &user.id)
        .await?
        .ok_or_else(|| ApiError::internal("post missing after insert"))?;

    let preview = preview_content(&post.content, 80);
    let chars = post.content.chars().count();
    let kind = if post.parent_id.is_some() {
        "reply"
    } else {
        "post"
    };
    tracing::info!(
        "{kind:6}    @{}  id={}  parent={}  ({} chars)  \"{}\"",
        user.handle,
        post.id,
        post.parent_id.as_deref().unwrap_or("-"),
        chars,
        preview
    );

    let _ = state.broadcast_tx.send(Arc::new(Event::NewPost {
        post: post.clone(),
    }));

    Ok(Json(post))
}

pub async fn edit_post(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(post_id): Path<String>,
    Json(body): Json<EditPostRequest>,
) -> ApiResult<Json<Post>> {
    let content =
        validate_content(&body.content).map_err(|e| ApiError::bad_request(e.to_string()))?;

    let row: Option<(String, String, Option<chrono::DateTime<Utc>>)> =
        sqlx::query_as(r#"SELECT author_id, content, deleted_at FROM posts WHERE id = $1"#)
            .bind(&post_id)
            .fetch_optional(&state.db)
            .await?;
    let (author_id, old_content, deleted_at) =
        row.ok_or_else(|| ApiError::not_found("post not found"))?;
    if author_id != user.id {
        return Err(ApiError::forbidden("only the author can edit this post"));
    }
    if deleted_at.is_some() {
        return Err(ApiError::bad_request("cannot edit a deleted post"));
    }

    sqlx::query(
        r#"
        UPDATE posts SET content = $1, edited_at = NOW()
        WHERE id = $2
        "#,
    )
    .bind(&content)
    .bind(&post_id)
    .execute(&state.db)
    .await?;

    sync_post_tags(&state.db, &post_id, &content).await?;
    notify_mentions(
        &state,
        &user.id,
        &content,
        &post_id,
        Some(old_content.as_str()),
    )
    .await?;

    let post = fetch_post(&state.db, &post_id, &user.id)
        .await?
        .ok_or_else(|| ApiError::internal("post missing after edit"))?;

    tracing::info!("edit      @{}  id={}", user.handle, post.id);
    let _ = state.broadcast_tx.send(Arc::new(Event::PostEdited {
        post: post.clone(),
    }));

    Ok(Json(post))
}

/// Full thread for a post id (walks to root, returns root + all descendants).
pub async fn get_thread(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(post_id): Path<String>,
) -> ApiResult<Json<ThreadResponse>> {
    let root_id: Option<String> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE ancestors AS (
            SELECT id, parent_id FROM posts WHERE id = $1
            UNION ALL
            SELECT p.id, p.parent_id
            FROM posts p
            JOIN ancestors a ON a.parent_id = p.id
        )
        SELECT id FROM ancestors WHERE parent_id IS NULL
        LIMIT 1
        "#,
    )
    .bind(&post_id)
    .fetch_optional(&state.db)
    .await?;

    let root_id = root_id.ok_or_else(|| ApiError::not_found("post not found"))?;

    let sql = format!(
        r#"
        WITH RECURSIVE tree AS (
            SELECT id FROM posts WHERE id = $1
            UNION ALL
            SELECT p.id FROM posts p
            JOIN tree t ON p.parent_id = t.id
        )
        {POST_SELECT}
        WHERE p.id IN (SELECT id FROM tree)
        ORDER BY p.created_at ASC, p.id ASC
        "#
    )
    .replace("$viewer", "$2");

    let rows: Vec<PostRow> = sqlx::query_as(&sql)
        .bind(&root_id)
        .bind(&user.id)
        .fetch_all(&state.db)
        .await?;

    let posts: Vec<Post> = rows.into_iter().map(row_to_post).collect();
    let root = posts
        .iter()
        .find(|p| p.id == root_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("post not found"))?;

    // If root author is blocked, hide thread.
    if crate::db::is_blocked_either(&state.db, &user.id, &root.author_id).await? {
        return Err(ApiError::not_found("post not found"));
    }

    tracing::info!(
        "thread    @{}  root={}  {} post(s)",
        user.handle,
        root_id,
        posts.len()
    );

    Ok(Json(ThreadResponse { root, posts }))
}

/// Toggle like on a post. Returns updated counts.
pub async fn toggle_like(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(post_id): Path<String>,
) -> ApiResult<Json<LikeResponse>> {
    let author: Option<(String,)> =
        sqlx::query_as(r#"SELECT author_id FROM posts WHERE id = $1"#)
            .bind(&post_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((author_id,)) = author else {
        return Err(ApiError::not_found("post not found"));
    };
    if crate::db::is_blocked_either(&state.db, &user.id, &author_id).await? {
        return Err(ApiError::forbidden("cannot like: blocked"));
    }

    let existing: Option<(String,)> =
        sqlx::query_as(r#"SELECT post_id FROM likes WHERE post_id = $1 AND user_id = $2"#)
            .bind(&post_id)
            .bind(&user.id)
            .fetch_optional(&state.db)
            .await?;

    let liked_by_me = if existing.is_some() {
        sqlx::query(r#"DELETE FROM likes WHERE post_id = $1 AND user_id = $2"#)
            .bind(&post_id)
            .bind(&user.id)
            .execute(&state.db)
            .await?;
        false
    } else {
        let inserted = sqlx::query(
            r#"INSERT INTO likes (post_id, user_id) VALUES ($1, $2)
               ON CONFLICT DO NOTHING"#,
        )
        .bind(&post_id)
        .bind(&user.id)
        .execute(&state.db)
        .await?
        .rows_affected();
        // Only notify on a newly created like (not races / re-likes after concurrent insert).
        if inserted > 0 {
            notify(
                &state,
                &author_id,
                &user.id,
                NotificationKind::Like,
                Some(&post_id),
            )
            .await?;
        }
        true
    };

    let like_count: i64 =
        sqlx::query_scalar(r#"SELECT COUNT(*)::bigint FROM likes WHERE post_id = $1"#)
            .bind(&post_id)
            .fetch_one(&state.db)
            .await?;

    tracing::info!(
        "like      @{}  post={}  liked={}  count={}",
        user.handle,
        post_id,
        liked_by_me,
        like_count
    );

    let _ = state.broadcast_tx.send(Arc::new(Event::LikeUpdated {
        post_id: post_id.clone(),
        like_count,
    }));

    Ok(Json(LikeResponse {
        post_id,
        like_count,
        liked_by_me,
    }))
}

/// Tombstone a post (author only). Keeps the row and children.
pub async fn delete_post(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(post_id): Path<String>,
) -> ApiResult<Json<Post>> {
    let row: Option<(String, String, Option<chrono::DateTime<Utc>>)> = sqlx::query_as(
        r#"SELECT id, author_id, deleted_at FROM posts WHERE id = $1"#,
    )
    .bind(&post_id)
    .fetch_optional(&state.db)
    .await?;

    let (id, author_id, deleted_at) =
        row.ok_or_else(|| ApiError::not_found("post not found"))?;

    if author_id != user.id {
        return Err(ApiError::forbidden("only the author can delete this post"));
    }
    if deleted_at.is_some() {
        return Err(ApiError::bad_request("post already deleted"));
    }

    sqlx::query(
        r#"
        UPDATE posts
        SET deleted_at = NOW(), content = ''
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .execute(&state.db)
    .await?;

    sqlx::query(r#"DELETE FROM post_tags WHERE post_id = $1"#)
        .bind(&id)
        .execute(&state.db)
        .await?;

    let post = fetch_post(&state.db, &id, &user.id)
        .await?
        .ok_or_else(|| ApiError::internal("post missing after delete"))?;

    tracing::info!("delete    @{}  id={}", user.handle, post.id);

    let _ = state.broadcast_tx.send(Arc::new(Event::PostDeleted {
        post: post.clone(),
    }));

    Ok(Json(post))
}
