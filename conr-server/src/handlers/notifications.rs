use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use conr_shared::{Notification, NotificationsResponse};

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct NotifQuery {
    pub limit: Option<i64>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<NotifQuery>,
) -> ApiResult<Json<NotificationsResponse>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    type NotifRow = (
        String,
        String,
        String,
        Option<String>,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
    );
    let rows: Vec<NotifRow> = sqlx::query_as(
        r#"
        SELECT n.id, n.kind, u.handle, n.post_id, n.read_at, n.created_at
        FROM notifications n
        JOIN users u ON u.id = n.actor_id
        WHERE n.user_id = $1
        ORDER BY n.created_at DESC
        LIMIT $2
        "#,
    )
    .bind(&user.id)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let unread: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)::bigint FROM notifications WHERE user_id = $1 AND read_at IS NULL"#,
    )
    .bind(&user.id)
    .fetch_one(&state.db)
    .await?;

    let notifications = rows
        .into_iter()
        .map(|r| Notification {
            id: r.0,
            user_id: user.id.clone(),
            kind: r.1,
            actor_handle: r.2,
            post_id: r.3,
            read: r.4.is_some(),
            created_at: r.5,
        })
        .collect();

    Ok(Json(NotificationsResponse {
        notifications,
        unread,
    }))
}

pub async fn mark_read(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let n = sqlx::query(
        r#"UPDATE notifications SET read_at = NOW() WHERE user_id = $1 AND read_at IS NULL"#,
    )
    .bind(&user.id)
    .execute(&state.db)
    .await?
    .rows_affected();

    Ok(Json(serde_json::json!({ "marked": n })))
}
