use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use conr_shared::{new_id, validate_content, CreatePostRequest, Event, FeedResponse, Post};

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    /// Cursor: fetch posts older than this post id (xid ordered by time roughly).
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

pub async fn feed(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<FeedQuery>,
) -> ApiResult<Json<FeedResponse>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let rows: Vec<(
        String,
        String,
        String,
        Option<String>,
        String,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = if let Some(cursor) = q.cursor.as_ref() {
        // posts strictly older than the cursor post
        sqlx::query_as(
            r#"
            SELECT p.id, p.author_id, u.handle, p.parent_id, p.content, p.created_at, p.deleted_at
            FROM posts p
            JOIN users u ON u.id = p.author_id
            WHERE p.parent_id IS NULL
              AND p.deleted_at IS NULL
              AND (p.created_at, p.id) < (
                    SELECT created_at, id FROM posts WHERE id = $1
                  )
            ORDER BY p.created_at DESC, p.id DESC
            LIMIT $2
            "#,
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT p.id, p.author_id, u.handle, p.parent_id, p.content, p.created_at, p.deleted_at
            FROM posts p
            JOIN users u ON u.id = p.author_id
            WHERE p.parent_id IS NULL
              AND p.deleted_at IS NULL
            ORDER BY p.created_at DESC, p.id DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    };

    let posts: Vec<Post> = rows
        .into_iter()
        .map(|r| Post {
            id: r.0,
            author_id: r.1,
            author_handle: r.2,
            parent_id: r.3,
            content: r.4,
            created_at: r.5,
            deleted_at: r.6,
        })
        .collect();

    let next_cursor = posts.last().map(|p| p.id.clone());
    let count = posts.len();

    tracing::info!(
        "feed      @{}  loaded {} post(s)  limit={}  cursor={}",
        user.handle,
        count,
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
    if body.parent_id.is_some() {
        return Err(ApiError::bad_request(
            "replies are not enabled yet (Phase B)",
        ));
    }

    let content =
        validate_content(&body.content).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let id = new_id();

    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            Option<String>,
            String,
            DateTime<Utc>,
            Option<DateTime<Utc>>,
        ),
    >(
        r#"
        INSERT INTO posts (id, author_id, parent_id, content)
        VALUES ($1, $2, NULL, $3)
        RETURNING id, author_id, parent_id, content, created_at, deleted_at
        "#,
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&content)
    .fetch_one(&state.db)
    .await?;

    let post = Post {
        id: row.0,
        author_id: row.1,
        author_handle: user.handle.clone(),
        parent_id: row.2,
        content: row.3,
        created_at: row.4,
        deleted_at: row.5,
    };

    let preview = preview_content(&post.content, 80);
    let chars = post.content.chars().count();
    tracing::info!(
        "post      @{}  id={}  ({} chars)  \"{}\"",
        user.handle,
        post.id,
        chars,
        preview
    );

    let receivers = state.broadcast_tx.receiver_count();
    let _ = state.broadcast_tx.send(Arc::new(Event::NewPost {
        post: post.clone(),
    }));
    tracing::info!(
        "post      broadcast NewPost → {} websocket subscriber(s)",
        receivers
    );

    Ok(Json(post))
}

fn preview_content(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.chars().count() <= max {
        return flat;
    }
    let truncated: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{truncated}…")
}
