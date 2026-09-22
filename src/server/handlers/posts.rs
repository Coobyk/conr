use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::Utc;
use serde::Deserialize;

use crate::shared::{
    new_id, validate_content, validate_post_body, CreatePostRequest, EditPostRequest, Event,
    FeedResponse, LikeResponse, NotificationKind, Post, ThreadResponse,
};

use crate::server::auth::AuthUser;
use crate::server::db::{
    fetch_post, hydrate_posts, link_post_media, notify, notify_mentions, preview_content,
    rate_limit_posts, row_to_post, sync_post_tags, PostRow, NOT_BLOCKED, POST_SELECT,
};
use crate::server::error::{ApiError, ApiResult};
use crate::server::state::AppState;

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
        sqlx::query_as(sqlx::AssertSqlSafe(&*sql))
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
        sqlx::query_as(sqlx::AssertSqlSafe(&*sql))
            .bind(&user.id)
            .bind(limit)
            .fetch_all(&state.db)
            .await?
    };

    let mut posts: Vec<Post> = rows.into_iter().map(row_to_post).collect();
    hydrate_posts(&state.db, &mut posts, &user.id).await?;
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

    Ok(Json(FeedResponse { posts, next_cursor }))
}

fn validate_create_post(body: &CreatePostRequest) -> ApiResult<String> {
    if body.quote_id.is_some() && body.parent_id.is_some() {
        return Err(ApiError::bad_request("a quote cannot also be a reply"));
    }
    let content = validate_post_body(&body.content, body.image_ids.len())
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if let Some(quote_id) = &body.quote_id {
        if quote_id.trim().is_empty() {
            return Err(ApiError::bad_request("quote_id must not be empty"));
        }
        if content.is_empty() {
            return Err(ApiError::bad_request("a quote requires commentary"));
        }
    }
    Ok(content)
}

pub async fn create_post(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreatePostRequest>,
) -> ApiResult<Json<Post>> {
    let content = validate_create_post(&body)?;
    if let Some(quote_id) = &body.quote_id {
        let available: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM posts p
                WHERE p.id = $1 AND p.deleted_at IS NULL
                  AND NOT EXISTS (
                    SELECT 1 FROM blocks b
                    WHERE (b.blocker_id = $2 AND b.blocked_id = p.author_id)
                       OR (b.blocker_id = p.author_id AND b.blocked_id = $2)
                  )
            )
            "#,
        )
        .bind(quote_id)
        .bind(&user.id)
        .fetch_one(&state.db)
        .await?;
        if !available {
            return Err(ApiError::not_found("quoted post not found"));
        }
    }
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
        if crate::server::db::is_blocked_either(&state.db, &user.id, &author_id).await? {
            return Err(ApiError::forbidden("cannot reply: blocked"));
        }
        let _ = pid;
        parent_author = Some(author_id);
    }

    let id = new_id();

    sqlx::query(
        r#"
        INSERT INTO posts (id, author_id, parent_id, content, quote_id)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&body.parent_id)
    .bind(&content)
    .bind(&body.quote_id)
    .execute(&state.db)
    .await?;

    if let Err(e) = link_post_media(&state.db, &id, &user.id, &body.image_ids).await {
        // Roll back the post if media linking fails.
        let _ = sqlx::query(r#"DELETE FROM posts WHERE id = $1"#)
            .bind(&id)
            .execute(&state.db)
            .await;
        return Err(e);
    }

    if !content.is_empty() {
        sync_post_tags(&state.db, &id, &content).await?;
        notify_mentions(&state, &user.id, &content, &id, None).await?;
    }

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

    let preview = if post.content.is_empty() {
        format!("[{} image(s)]", post.images.len())
    } else {
        preview_content(&post.content, 80)
    };
    let chars = post.content.chars().count();
    let kind = if post.parent_id.is_some() {
        "reply"
    } else {
        "post"
    };
    tracing::info!(
        "{kind:6}    @{}  id={}  parent={}  ({} chars, {} img)  \"{}\"",
        user.handle,
        post.id,
        post.parent_id.as_deref().unwrap_or("-"),
        chars,
        post.images.len(),
        preview
    );

    let _ = state
        .broadcast_tx
        .send(Arc::new(Event::NewPost { post: post.clone() }));

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
    let _ = state
        .broadcast_tx
        .send(Arc::new(Event::PostEdited { post: post.clone() }));

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
          {NOT_BLOCKED}
        ORDER BY p.created_at ASC, p.id ASC
        "#
    )
    .replace("$viewer_user", "$2")
    .replace("$viewer", "$2");

    let rows: Vec<PostRow> = sqlx::query_as(sqlx::AssertSqlSafe(&*sql))
        .bind(&root_id)
        .bind(&user.id)
        .fetch_all(&state.db)
        .await?;

    let mut posts: Vec<Post> = rows.into_iter().map(row_to_post).collect();
    hydrate_posts(&state.db, &mut posts, &user.id).await?;
    let root = posts
        .iter()
        .find(|p| p.id == root_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("post not found"))?;

    // If root author is blocked, hide thread.
    if crate::server::db::is_blocked_either(&state.db, &user.id, &root.author_id).await? {
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
    let author: Option<(String,)> = sqlx::query_as(r#"SELECT author_id FROM posts WHERE id = $1"#)
        .bind(&post_id)
        .fetch_optional(&state.db)
        .await?;
    let Some((author_id,)) = author else {
        return Err(ApiError::not_found("post not found"));
    };
    if crate::server::db::is_blocked_either(&state.db, &user.id, &author_id).await? {
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
    let row: Option<(String, String, Option<chrono::DateTime<Utc>>)> =
        sqlx::query_as(r#"SELECT id, author_id, deleted_at FROM posts WHERE id = $1"#)
            .bind(&post_id)
            .fetch_optional(&state.db)
            .await?;

    let (id, author_id, deleted_at) = row.ok_or_else(|| ApiError::not_found("post not found"))?;

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

    let _ = state
        .broadcast_tx
        .send(Arc::new(Event::PostDeleted { post: post.clone() }));

    Ok(Json(post))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::auth::SigningKey;
    use crate::server::handlers::ws::event_for_viewer;
    use sqlx::PgPool;

    #[test]
    fn quote_validation_requires_commentary_and_excludes_replies() {
        let mut body = CreatePostRequest {
            content: "  commentary  ".into(),
            parent_id: None,
            quote_id: Some("source".into()),
            image_ids: Vec::new(),
        };
        assert_eq!(validate_create_post(&body).unwrap(), "commentary");
        body.parent_id = Some("parent".into());
        assert!(validate_create_post(&body).is_err());
        body.parent_id = None;
        body.quote_id = Some("  ".into());
        assert!(validate_create_post(&body).is_err());
        body.quote_id = Some("source".into());
        body.content = " \n ".into();
        assert!(validate_create_post(&body).is_err());
        body.image_ids.push("image".into());
        assert!(validate_create_post(&body).is_err());
        body.quote_id = None;
        assert_eq!(validate_create_post(&body).unwrap(), "");
        body.quote_id = Some("source".into());
        body.content = "x".repeat(crate::shared::POST_MAX_LEN + 1);
        assert!(validate_create_post(&body).is_err());
        body.content = "commentary".into();
        body.image_ids = vec!["image".into(); crate::shared::MEDIA_MAX_PER_POST + 1];
        assert!(validate_create_post(&body).is_err());
    }

    async fn assert_quote_views(state: Arc<AppState>, post_id: &str, visible: bool) {
        use crate::server::handlers::{search, social};
        let viewer = AuthUser {
            id: "viewer".into(),
            handle: "viewer".into(),
        };
        let Json(feed) = feed(
            State(state.clone()),
            viewer.clone(),
            Query(FeedQuery {
                cursor: None,
                limit: None,
                filter: None,
            }),
        )
        .await
        .unwrap();
        let Json(thread) = get_thread(State(state.clone()), viewer.clone(), Path(post_id.into()))
            .await
            .unwrap();
        let Json(search) = search::search(
            State(state.clone()),
            viewer.clone(),
            Query(search::SearchQuery {
                q: "commentary".into(),
                limit: None,
            }),
        )
        .await
        .unwrap();
        let Json(profile) = social::profile_posts(
            State(state.clone()),
            viewer,
            Path("quoter".into()),
            Query(social::ProfilePostsQuery {
                cursor: None,
                limit: None,
            }),
        )
        .await
        .unwrap();
        let fetched = fetch_post(&state.db, post_id, "viewer")
            .await
            .unwrap()
            .unwrap();
        for post in [
            &fetched,
            feed.posts.iter().find(|p| p.id == post_id).unwrap(),
            &thread.root,
            thread.posts.iter().find(|p| p.id == post_id).unwrap(),
            search.posts.iter().find(|p| p.id == post_id).unwrap(),
            profile.posts.iter().find(|p| p.id == post_id).unwrap(),
        ] {
            assert_eq!(post.quote_id.as_deref(), Some("source"));
            assert_eq!(post.quote.is_some(), visible);
            if let Some(quote) = &post.quote {
                assert_eq!(quote.content, "source text");
                assert_eq!(quote.author_handle, "original");
                assert_eq!(quote.images[0].id, "image");
            }
            for event in [
                Event::NewPost { post: post.clone() },
                Event::PostEdited { post: post.clone() },
            ] {
                let event = event_for_viewer(&state, "viewer", &event).await.unwrap();
                match event {
                    Event::NewPost { post } | Event::PostEdited { post } => {
                        assert_eq!(post.quote.is_some(), visible)
                    }
                    _ => panic!("unexpected event"),
                }
            }
        }
    }

    #[sqlx::test]
    #[ignore = "requires PostgreSQL via DATABASE_URL"]
    async fn quotes_are_persisted_and_filtered_for_ws_viewers(pool: PgPool) {
        let (broadcast_tx, mut events) = tokio::sync::broadcast::channel(32);
        let state = Arc::new(AppState {
            db: pool.clone(),
            broadcast_tx,
            signing_key: SigningKey::from_seed(&[0; 32]).unwrap(),
            media_dir: std::env::temp_dir(),
            login_limiter: crate::server::ratelimit::RateLimiter::new(
                std::time::Duration::from_secs(900),
                20,
            ),
            register_limiter: crate::server::ratelimit::RateLimiter::new(
                std::time::Duration::from_secs(3600),
                10,
            ),
        });
        for id in ["original", "quoter", "viewer"] {
            sqlx::query("INSERT INTO users (id, handle, token_hash) VALUES ($1, $1, $2)")
                .bind(id)
                .bind(id.as_bytes())
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO posts (id, author_id, content) VALUES ('source', 'original', 'source text')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO media (id, uploader_id, content_type, size_bytes) VALUES ('image', 'original', 'image/png', 42)")
            .execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO post_media (post_id, media_id, position) VALUES ('source', 'image', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let Json(post) = create_post(
            State(state.clone()),
            AuthUser {
                id: "quoter".into(),
                handle: "quoter".into(),
            },
            Json(CreatePostRequest {
                content: "commentary".into(),
                parent_id: None,
                quote_id: Some("source".into()),
                image_ids: Vec::new(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(post.quote_id.as_deref(), Some("source"));
        assert_eq!(post.quote.as_ref().unwrap().content, "source text");
        let quote_post_id = post.id.clone();
        assert_quote_views(state.clone(), &quote_post_id, true).await;
        let event = events.recv().await.unwrap();
        let Some(Event::NewPost { post }) = event_for_viewer(&state, "viewer", &event).await else {
            panic!("expected NewPost");
        };
        assert_eq!(post.quote.as_ref().unwrap().content, "source text");
        sqlx::query("INSERT INTO blocks (blocker_id, blocked_id, reason) VALUES ('original', 'viewer', 'test')")
            .execute(&pool)
            .await
            .unwrap();
        let Some(Event::NewPost { post }) = event_for_viewer(&state, "viewer", &event).await else {
            panic!("expected NewPost with hidden quote");
        };
        assert_eq!(post.quote_id.as_deref(), Some("source"));
        assert!(post.quote.is_none());
        assert_quote_views(state.clone(), &quote_post_id, false).await;
        sqlx::query("DELETE FROM blocks")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO blocks (blocker_id, blocked_id, reason) VALUES ('viewer', 'original', 'test')")
            .execute(&pool).await.unwrap();
        assert_quote_views(state.clone(), &quote_post_id, false).await;
        for (blocker, blocked) in [("original", "quoter"), ("quoter", "original")] {
            sqlx::query("DELETE FROM blocks")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO blocks (blocker_id, blocked_id, reason) VALUES ($1, $2, 'test')",
            )
            .bind(blocker)
            .bind(blocked)
            .execute(&pool)
            .await
            .unwrap();
            let result = create_post(
                State(state.clone()),
                AuthUser {
                    id: "quoter".into(),
                    handle: "quoter".into(),
                },
                Json(CreatePostRequest {
                    content: "blocked commentary".into(),
                    parent_id: None,
                    quote_id: Some("source".into()),
                    image_ids: Vec::new(),
                }),
            )
            .await;
            assert_eq!(
                result.unwrap_err().status,
                axum::http::StatusCode::NOT_FOUND
            );
        }
        sqlx::query("DELETE FROM blocks")
            .execute(&pool)
            .await
            .unwrap();
        let _ = delete_post(
            State(state.clone()),
            AuthUser {
                id: "original".into(),
                handle: "original".into(),
            },
            Path("source".into()),
        )
        .await
        .unwrap();
        assert_quote_views(state.clone(), &quote_post_id, false).await;
        let Some(Event::NewPost { post }) = event_for_viewer(&state, "viewer", &event).await else {
            panic!("expected NewPost with deleted original hidden");
        };
        assert!(post.quote.is_none());
        for quote_id in ["source", "missing"] {
            let result = create_post(
                State(state.clone()),
                AuthUser {
                    id: "quoter".into(),
                    handle: "quoter".into(),
                },
                Json(CreatePostRequest {
                    content: "unavailable commentary".into(),
                    parent_id: None,
                    quote_id: Some(quote_id.into()),
                    image_ids: Vec::new(),
                }),
            )
            .await;
            assert_eq!(
                result.unwrap_err().status,
                axum::http::StatusCode::NOT_FOUND
            );
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE author_id = 'quoter'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        let Json(deleted) = delete_post(
            State(state.clone()),
            AuthUser {
                id: "quoter".into(),
                handle: "quoter".into(),
            },
            Path(quote_post_id),
        )
        .await
        .unwrap();
        assert_eq!(deleted.quote_id.as_deref(), Some("source"));
        assert!(deleted.quote.is_none());
        assert!(deleted.images.is_empty());
    }
}
