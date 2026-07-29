use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use conr_shared::{SearchResponse, TagInfo, UserProfile};

use crate::auth::AuthUser;
use crate::db::{row_to_post, NOT_BLOCKED, POST_SELECT, PostRow};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub limit: Option<i64>,
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<SearchResponse>> {
    let query = q.q.trim();
    if query.is_empty() {
        return Err(ApiError::bad_request("empty search query"));
    }
    let limit = q.limit.unwrap_or(25).clamp(1, 100);
    let blocked = NOT_BLOCKED;

    // Posts: FTS if not starting with # or @, else specialized.
    let mut posts = Vec::new();
    let mut users = Vec::new();
    let mut tags = Vec::new();

    if let Some(tag) = query.strip_prefix('#') {
        let tag = tag.to_ascii_lowercase();
        let sql = format!(
            r#"
            {POST_SELECT}
            JOIN post_tags pt ON pt.post_id = p.id
            WHERE pt.tag = $2
              AND p.deleted_at IS NULL
              {blocked}
            ORDER BY p.created_at DESC
            LIMIT $3
            "#
        )
        .replace("$viewer_user", "$1")
        .replace("$viewer", "$1");
        let rows: Vec<PostRow> = sqlx::query_as(&sql)
            .bind(&user.id)
            .bind(&tag)
            .bind(limit)
            .fetch_all(&state.db)
            .await?;
        posts = rows.into_iter().map(row_to_post).collect();
        let count: i64 =
            sqlx::query_scalar(r#"SELECT COUNT(*)::bigint FROM post_tags WHERE tag = $1"#)
                .bind(&tag)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0);
        tags.push(TagInfo {
            name: tag,
            post_count: count,
        });
    } else if let Some(h) = query.strip_prefix('@') {
        users = search_users(&state, &user.id, h, limit).await?;
    } else {
        // FTS posts
        let sql = format!(
            r#"
            {POST_SELECT}
            WHERE p.deleted_at IS NULL
              AND p.content_tsv @@ plainto_tsquery('english', $2)
              {blocked}
            ORDER BY ts_rank(p.content_tsv, plainto_tsquery('english', $2)) DESC, p.created_at DESC
            LIMIT $3
            "#
        )
        .replace("$viewer_user", "$1")
        .replace("$viewer", "$1");
        let rows: Vec<PostRow> = sqlx::query_as(&sql)
            .bind(&user.id)
            .bind(query)
            .bind(limit)
            .fetch_all(&state.db)
            .await?;
        posts = rows.into_iter().map(row_to_post).collect();

        // Also match handles / tags loosely
        users = search_users(&state, &user.id, query, 10).await?;
        let tag_rows: Vec<(String, i64)> = sqlx::query_as(
            r#"
            SELECT t.name, COUNT(pt.post_id)::bigint
            FROM tags t
            LEFT JOIN post_tags pt ON pt.tag = t.name
            WHERE t.name ILIKE $1
            GROUP BY t.name
            ORDER BY COUNT(pt.post_id) DESC
            LIMIT 10
            "#,
        )
        .bind(format!("%{query}%"))
        .fetch_all(&state.db)
        .await?;
        tags = tag_rows
            .into_iter()
            .map(|(name, post_count)| TagInfo { name, post_count })
            .collect();
    }

    tracing::info!(
        "search    @{}  q={:?}  posts={} users={} tags={}",
        user.handle,
        query,
        posts.len(),
        users.len(),
        tags.len()
    );

    Ok(Json(SearchResponse {
        posts,
        users,
        tags,
    }))
}

async fn search_users(
    state: &AppState,
    viewer_id: &str,
    q: &str,
    limit: i64,
) -> ApiResult<Vec<UserProfile>> {
    let pattern = format!("%{}%", q.trim().to_ascii_lowercase());
    let rows: Vec<(String, String, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT id, handle, bio, created_at FROM users
        WHERE handle ILIKE $1
          AND NOT EXISTS (
            SELECT 1 FROM blocks b
            WHERE (b.blocker_id = $2 AND b.blocked_id = users.id)
               OR (b.blocker_id = users.id AND b.blocked_id = $2)
          )
        ORDER BY handle
        LIMIT $3
        "#,
    )
    .bind(&pattern)
    .bind(viewer_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let mut out = Vec::new();
    for (id, handle, bio, created_at) in rows {
        out.push(profile_lite(state, viewer_id, id, handle, bio, created_at).await?);
    }
    Ok(out)
}

async fn profile_lite(
    state: &AppState,
    viewer_id: &str,
    id: String,
    handle: String,
    bio: Option<String>,
    created_at: DateTime<Utc>,
) -> ApiResult<UserProfile> {
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
    .bind(viewer_id)
    .bind(&id)
    .fetch_one(&state.db)
    .await?;
    let follows_me: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM follows WHERE follower_id = $1 AND followee_id = $2)"#,
    )
    .bind(&id)
    .bind(viewer_id)
    .fetch_one(&state.db)
    .await?;
    let blocked_by_me: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM blocks WHERE blocker_id = $1 AND blocked_id = $2)"#,
    )
    .bind(viewer_id)
    .bind(&id)
    .fetch_one(&state.db)
    .await?;

    Ok(UserProfile {
        is_me: id == viewer_id,
        id,
        handle,
        bio,
        created_at,
        followers,
        following,
        posts,
        followed_by_me,
        follows_me,
        blocked_by_me,
    })
}

pub async fn tag_posts(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(tag): Path<String>,
) -> ApiResult<Json<SearchResponse>> {
    let tag = tag.trim().trim_start_matches('#').to_ascii_lowercase();
    if tag.is_empty() {
        return Err(ApiError::bad_request("empty tag"));
    }
    search(
        State(state),
        user,
        Query(SearchQuery {
            q: format!("#{tag}"),
            limit: Some(50),
        }),
    )
    .await
}

