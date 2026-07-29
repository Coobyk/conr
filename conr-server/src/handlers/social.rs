use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use conr_shared::{
    normalize_handle, validate_bio, validate_block_reason, BlockRequest, FeedResponse,
    NotificationKind, Post, UpdateBioRequest, UserProfile,
};

use crate::auth::AuthUser;
use crate::db::{
    is_blocked_either, notify, row_to_post, NOT_BLOCKED, POST_SELECT, PostRow,
};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn get_profile(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(handle): Path<String>,
) -> ApiResult<Json<UserProfile>> {
    let handle =
        normalize_handle(&handle).map_err(|e| ApiError::bad_request(e.to_string()))?;

    let row: Option<(String, String, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        r#"SELECT id, handle, bio, created_at FROM users WHERE handle = $1"#,
    )
    .bind(&handle)
    .fetch_optional(&state.db)
    .await?;

    let (id, handle, bio, created_at) =
        row.ok_or_else(|| ApiError::not_found("user not found"))?;

    if is_blocked_either(&state.db, &user.id, &id).await? && id != user.id {
        return Err(ApiError::not_found("user not found"));
    }

    let followers: i64 =
        sqlx::query_scalar(r#"SELECT COUNT(*)::bigint FROM follows WHERE followee_id = $1"#)
            .bind(&id)
            .fetch_one(&state.db)
            .await?;
    let following: i64 =
        sqlx::query_scalar(r#"SELECT COUNT(*)::bigint FROM follows WHERE follower_id = $1"#)
            .bind(&id)
            .fetch_one(&state.db)
            .await?;
    let posts: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)::bigint FROM posts WHERE author_id = $1 AND deleted_at IS NULL"#,
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await?;

    let followed_by_me: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM follows WHERE follower_id = $1 AND followee_id = $2)"#,
    )
    .bind(&user.id)
    .bind(&id)
    .fetch_one(&state.db)
    .await?;
    let follows_me: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM follows WHERE follower_id = $1 AND followee_id = $2)"#,
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_one(&state.db)
    .await?;
    let blocked_by_me: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM blocks WHERE blocker_id = $1 AND blocked_id = $2)"#,
    )
    .bind(&user.id)
    .bind(&id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(UserProfile {
        id: id.clone(),
        handle,
        bio,
        created_at,
        followers,
        following,
        posts,
        followed_by_me,
        follows_me,
        blocked_by_me,
        is_me: id == user.id,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ProfilePostsQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

pub async fn profile_posts(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(handle): Path<String>,
    Query(q): Query<ProfilePostsQuery>,
) -> ApiResult<Json<FeedResponse>> {
    let handle =
        normalize_handle(&handle).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let author_id: Option<String> =
        sqlx::query_scalar(r#"SELECT id FROM users WHERE handle = $1"#)
            .bind(&handle)
            .fetch_optional(&state.db)
            .await?;
    let author_id = author_id.ok_or_else(|| ApiError::not_found("user not found"))?;
    if is_blocked_either(&state.db, &user.id, &author_id).await? {
        return Err(ApiError::not_found("user not found"));
    }

    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let blocked = NOT_BLOCKED;

    let rows: Vec<PostRow> = if let Some(cursor) = q.cursor.as_ref() {
        let sql = format!(
            r#"
            {POST_SELECT}
            WHERE p.author_id = $3
              AND p.parent_id IS NULL
              AND (p.created_at, p.id) < (
                    SELECT created_at, id FROM posts WHERE id = $1
                  )
              {blocked}
            ORDER BY p.created_at DESC, p.id DESC
            LIMIT $4
            "#
        )
        .replace("$viewer_user", "$2")
        .replace("$viewer", "$2");
        sqlx::query_as(&sql)
            .bind(cursor)
            .bind(&user.id)
            .bind(&author_id)
            .bind(limit)
            .fetch_all(&state.db)
            .await?
    } else {
        let sql = format!(
            r#"
            {POST_SELECT}
            WHERE p.author_id = $2
              AND p.parent_id IS NULL
              {blocked}
            ORDER BY p.created_at DESC, p.id DESC
            LIMIT $3
            "#
        )
        .replace("$viewer_user", "$1")
        .replace("$viewer", "$1");
        sqlx::query_as(&sql)
            .bind(&user.id)
            .bind(&author_id)
            .bind(limit)
            .fetch_all(&state.db)
            .await?
    };

    let posts: Vec<Post> = rows.into_iter().map(row_to_post).collect();
    let next_cursor = posts.last().map(|p| p.id.clone());
    Ok(Json(FeedResponse {
        posts,
        next_cursor,
    }))
}

pub async fn follow(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(handle): Path<String>,
) -> ApiResult<Json<UserProfile>> {
    let handle =
        normalize_handle(&handle).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let target: Option<String> = sqlx::query_scalar(r#"SELECT id FROM users WHERE handle = $1"#)
        .bind(&handle)
        .fetch_optional(&state.db)
        .await?;
    let target = target.ok_or_else(|| ApiError::not_found("user not found"))?;
    if target == user.id {
        return Err(ApiError::bad_request("cannot follow yourself"));
    }
    if is_blocked_either(&state.db, &user.id, &target).await? {
        return Err(ApiError::forbidden("cannot follow: blocked"));
    }

    let inserted = sqlx::query(
        r#"INSERT INTO follows (follower_id, followee_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"#,
    )
    .bind(&user.id)
    .bind(&target)
    .execute(&state.db)
    .await?
    .rows_affected();

    // Only notify on a new follow edge (not repeat follow clicks).
    if inserted > 0 {
        notify(&state, &target, &user.id, NotificationKind::Follow, None).await?;
    }
    tracing::info!("follow    @{} → @{}", user.handle, handle);

    get_profile(State(state), user, Path(handle)).await
}

pub async fn unfollow(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(handle): Path<String>,
) -> ApiResult<Json<UserProfile>> {
    let handle =
        normalize_handle(&handle).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let target: Option<String> = sqlx::query_scalar(r#"SELECT id FROM users WHERE handle = $1"#)
        .bind(&handle)
        .fetch_optional(&state.db)
        .await?;
    let target = target.ok_or_else(|| ApiError::not_found("user not found"))?;

    sqlx::query(r#"DELETE FROM follows WHERE follower_id = $1 AND followee_id = $2"#)
        .bind(&user.id)
        .bind(&target)
        .execute(&state.db)
        .await?;

    tracing::info!("unfollow  @{} ↛ @{}", user.handle, handle);
    get_profile(State(state), user, Path(handle)).await
}

pub async fn block(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(handle): Path<String>,
    Json(body): Json<BlockRequest>,
) -> ApiResult<Json<UserProfile>> {
    let handle =
        normalize_handle(&handle).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let reason = validate_block_reason(&body.reason)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let target: Option<String> = sqlx::query_scalar(r#"SELECT id FROM users WHERE handle = $1"#)
        .bind(&handle)
        .fetch_optional(&state.db)
        .await?;
    let target = target.ok_or_else(|| ApiError::not_found("user not found"))?;
    if target == user.id {
        return Err(ApiError::bad_request("cannot block yourself"));
    }

    // Remove follows both ways.
    sqlx::query(
        r#"DELETE FROM follows WHERE (follower_id = $1 AND followee_id = $2)
           OR (follower_id = $2 AND followee_id = $1)"#,
    )
    .bind(&user.id)
    .bind(&target)
    .execute(&state.db)
    .await?;

    sqlx::query(
        r#"INSERT INTO blocks (blocker_id, blocked_id, reason) VALUES ($1, $2, $3)
           ON CONFLICT (blocker_id, blocked_id) DO UPDATE SET reason = EXCLUDED.reason"#,
    )
    .bind(&user.id)
    .bind(&target)
    .bind(&reason)
    .execute(&state.db)
    .await?;

    tracing::info!("block     @{} ⊣ @{}  reason={}", user.handle, handle, reason);
    // Profile may return not found due to block — return minimal ok profile.
    Ok(Json(UserProfile {
        id: target,
        handle,
        bio: None,
        created_at: Utc::now(),
        followers: 0,
        following: 0,
        posts: 0,
        followed_by_me: false,
        follows_me: false,
        blocked_by_me: true,
        is_me: false,
    }))
}

pub async fn unblock(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(handle): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let handle =
        normalize_handle(&handle).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let target: Option<String> = sqlx::query_scalar(r#"SELECT id FROM users WHERE handle = $1"#)
        .bind(&handle)
        .fetch_optional(&state.db)
        .await?;
    let target = target.ok_or_else(|| ApiError::not_found("user not found"))?;

    sqlx::query(r#"DELETE FROM blocks WHERE blocker_id = $1 AND blocked_id = $2"#)
        .bind(&user.id)
        .bind(&target)
        .execute(&state.db)
        .await?;

    tracing::info!("unblock   @{} ⊤ @{}", user.handle, handle);
    Ok(Json(serde_json::json!({ "ok": true, "handle": handle })))
}

pub async fn update_bio(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<UpdateBioRequest>,
) -> ApiResult<Json<UserProfile>> {
    let bio = validate_bio(body.bio.as_deref())
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    sqlx::query(r#"UPDATE users SET bio = $1 WHERE id = $2"#)
        .bind(&bio)
        .bind(&user.id)
        .execute(&state.db)
        .await?;
    get_profile(State(state), user.clone(), Path(user.handle)).await
}

/// Handles the current user follows (for client live-feed filters).
pub async fn list_following(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let handles: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT u.handle FROM follows f
        JOIN users u ON u.id = f.followee_id
        WHERE f.follower_id = $1
        ORDER BY u.handle
        "#,
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "handles": handles })))
}

/// Handles the current user has blocked (for client live-feed filters).
pub async fn list_blocks(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let handles: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT u.handle FROM blocks b
        JOIN users u ON u.id = b.blocked_id
        WHERE b.blocker_id = $1
        ORDER BY u.handle
        "#,
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "handles": handles })))
}
