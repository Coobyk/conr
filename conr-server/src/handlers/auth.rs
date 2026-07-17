use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use conr_shared::{
    generate_token, hash_token, new_id, normalize_handle, strip_token, validate_token,
    AuthResponse, LoginRequest, RefreshRequest, RegisterRequest, RegisterResponse, User,
};

use crate::auth::{
    fetch_user_by_handle, issue_access_token, issue_refresh_token, rotate_refresh_token,
    token_matches,
};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterRequest>,
) -> ApiResult<Json<RegisterResponse>> {
    let handle = normalize_handle(&body.handle).map_err(|e| ApiError::bad_request(e.to_string()))?;

    let token = generate_token();
    let token_hash = hash_token(&token);
    let id = new_id();

    let row = sqlx::query_as::<_, (String, String, Option<String>, chrono::DateTime<chrono::Utc>)>(
        r#"INSERT INTO users (id, handle, token_hash)
           VALUES ($1, $2, $3)
           RETURNING id, handle, bio, created_at"#,
    )
    .bind(&id)
    .bind(&handle)
    .bind(&token_hash[..])
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db) = &e {
            if db.constraint() == Some("users_handle_key") {
                return ApiError::conflict("handle already taken");
            }
        }
        ApiError::from(e)
    })?;

    let user = User {
        id: row.0,
        handle: row.1,
        bio: row.2,
        created_at: row.3,
    };

    let access_token = issue_access_token(&state.signing_key, &user.id, &user.handle)?;
    let refresh_token = issue_refresh_token(&state.db, &user.id).await?;

    tracing::info!("register  @{}  id={}", user.handle, user.id);

    Ok(Json(RegisterResponse {
        token,
        access_token,
        refresh_token,
        user,
    }))
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
    let handle = normalize_handle(&body.handle).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let token = strip_token(&body.token);
    validate_token(&token).map_err(|e| ApiError::bad_request(e.to_string()))?;

    let Some((user, stored_hash)) = fetch_user_by_handle(&state.db, &handle).await? else {
        tracing::warn!("login     @{handle}  failed — unknown handle");
        return Err(ApiError::unauthorized("invalid handle or token"));
    };

    if !token_matches(&stored_hash, &token) {
        tracing::warn!("login     @{}  failed — bad token", user.handle);
        return Err(ApiError::unauthorized("invalid handle or token"));
    }

    let access_token = issue_access_token(&state.signing_key, &user.id, &user.handle)?;
    let refresh_token = issue_refresh_token(&state.db, &user.id).await?;

    tracing::info!("login     @{}  ok  id={}", user.handle, user.id);

    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
        user,
    }))
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RefreshRequest>,
) -> ApiResult<Json<AuthResponse>> {
    let (refresh_token, user) = rotate_refresh_token(&state.db, &body.refresh_token).await?;
    let access_token = issue_access_token(&state.signing_key, &user.id, &user.handle)?;
    tracing::info!("refresh   @{}  session rotated", user.handle);
    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
        user,
    }))
}
