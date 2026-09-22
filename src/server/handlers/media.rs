//! Image upload and download.

use std::path::PathBuf;
use std::sync::Arc;

use crate::shared::{new_id, validate_media_bytes, MediaInfo, MEDIA_MAX_BYTES};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Json;

use crate::server::auth::AuthUser;
use crate::server::error::{ApiError, ApiResult};
use crate::server::state::AppState;

/// Max body for multipart uploads (file + form overhead).
pub fn upload_body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(MEDIA_MAX_BYTES + 64 * 1024)
}

pub async fn upload(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    mut multipart: Multipart,
) -> ApiResult<Json<MediaInfo>> {
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("multipart: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name != "file" && name != "image" {
            continue;
        }
        let data = field
            .bytes()
            .await
            .map_err(|e| ApiError::bad_request(format!("read file: {e}")))?;
        if data.len() > MEDIA_MAX_BYTES {
            return Err(ApiError::bad_request(format!(
                "image too large (max {} bytes)",
                MEDIA_MAX_BYTES
            )));
        }
        file_bytes = Some(data.to_vec());
        break;
    }

    let bytes = file_bytes.ok_or_else(|| ApiError::bad_request("missing file field"))?;
    let content_type =
        validate_media_bytes(&bytes).map_err(|e| ApiError::bad_request(e.to_string()))?;

    let id = new_id();
    let path = media_path(&state.media_dir, &id);
    tokio::fs::write(&path, &bytes).await.map_err(|e| {
        tracing::error!(?e, path = %path.display(), "media write failed");
        ApiError::internal("failed to store image")
    })?;

    let size = bytes.len() as i64;
    sqlx::query(
        r#"
        INSERT INTO media (id, uploader_id, content_type, size_bytes)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(&id)
    .bind(&user.id)
    .bind(content_type)
    .bind(size)
    .execute(&state.db)
    .await
    .map_err(|e| {
        // Best-effort cleanup if DB insert fails.
        let _ = std::fs::remove_file(&path);
        ApiError::from(e)
    })?;

    tracing::info!(
        "media     @{}  id={}  type={}  {} bytes",
        user.handle,
        id,
        content_type,
        size
    );

    Ok(Json(MediaInfo {
        id,
        content_type: content_type.to_string(),
        size_bytes: size,
        width: None,
        height: None,
    }))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let row: Option<(String, String, i64)> = sqlx::query_as(
        r#"
        SELECT m.id, m.content_type, m.size_bytes
        FROM media m
        WHERE m.id = $1
          AND NOT EXISTS (
              SELECT 1 FROM blocks b
              WHERE (b.blocker_id = $2 AND b.blocked_id = m.uploader_id)
                 OR (b.blocker_id = m.uploader_id AND b.blocked_id = $2)
          )
        "#,
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await?;

    let Some((media_id, content_type, _size)) = row else {
        return Err(ApiError::not_found("media not found"));
    };

    let path = media_path(&state.media_dir, &media_id);
    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found("media file missing")
        } else {
            tracing::error!(?e, path = %path.display(), "media read failed");
            ApiError::internal("failed to read image")
        }
    })?;

    let mut res = Response::new(Body::from(bytes));
    *res.status_mut() = StatusCode::OK;
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    let _ = headers; // CONTENT_LENGTH set by Body

    Ok(res)
}

pub fn media_path(dir: &std::path::Path, id: &str) -> PathBuf {
    dir.join(id)
}
