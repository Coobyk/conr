use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};

use crate::shared::{
    new_id, normalize_handle, validate_content, DmConversation, DmListResponse, DmMessage,
    DmThreadResponse, Event, SendDmRequest,
};

use crate::server::auth::AuthUser;
use crate::server::db::is_blocked_either;
use crate::server::error::{ApiError, ApiResult};
use crate::server::state::AppState;

type ConvRow = (
    String,
    String,
    String,
    Option<String>,
    Option<DateTime<Utc>>,
    i64,
);

pub async fn list_conversations(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> ApiResult<Json<DmListResponse>> {
    let rows: Vec<ConvRow> = sqlx::query_as(
        r#"
            SELECT c.id,
                   peer.id,
                   peer.handle,
                   (
                     SELECT m.content FROM dm_messages m
                     WHERE m.conversation_id = c.id
                     ORDER BY m.created_at DESC LIMIT 1
                   ) AS last_message,
                   (
                     SELECT m.created_at FROM dm_messages m
                     WHERE m.conversation_id = c.id
                     ORDER BY m.created_at DESC LIMIT 1
                   ) AS last_at,
                   (
                     SELECT COUNT(*)::bigint FROM dm_messages m
                     WHERE m.conversation_id = c.id
                       AND m.sender_id <> $1
                       AND m.created_at > COALESCE(
                             (SELECT r.last_read_at FROM dm_reads r
                              WHERE r.user_id = $1 AND r.conversation_id = c.id),
                             '-infinity'::timestamptz
                           )
                   ) AS unread
            FROM dm_conversations c
            JOIN dm_participants me ON me.conversation_id = c.id AND me.user_id = $1
            JOIN dm_participants other ON other.conversation_id = c.id AND other.user_id <> $1
            JOIN users peer ON peer.id = other.user_id
            WHERE NOT EXISTS (
                SELECT 1 FROM blocks b
                WHERE (b.blocker_id = $1 AND b.blocked_id = peer.id)
                   OR (b.blocker_id = peer.id AND b.blocked_id = $1)
            )
            ORDER BY last_at DESC NULLS LAST
            "#,
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;

    let conversations: Vec<DmConversation> = rows
        .into_iter()
        .map(|r| DmConversation {
            id: r.0,
            peer_id: r.1,
            peer_handle: r.2,
            last_message: r.3,
            last_at: r.4,
            unread: r.5,
        })
        .collect();
    let unread = conversations.iter().map(|c| c.unread).sum();

    Ok(Json(DmListResponse {
        conversations,
        unread,
    }))
}

pub async fn get_conversation(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<DmThreadResponse>> {
    ensure_participant(&state, &user.id, &id).await?;

    let peer: Option<(String, String)> = sqlx::query_as(
        r#"
        SELECT u.id, u.handle
        FROM dm_participants p
        JOIN users u ON u.id = p.user_id
        WHERE p.conversation_id = $1 AND p.user_id <> $2
        LIMIT 1
        "#,
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await?;
    let (peer_id, peer_handle) =
        peer.ok_or_else(|| ApiError::not_found("conversation not found"))?;

    if is_blocked_either(&state.db, &user.id, &peer_id).await? {
        return Err(ApiError::forbidden("blocked"));
    }

    let msg_rows: Vec<(String, String, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT m.id, m.conversation_id, m.sender_id, u.handle, m.content, m.created_at
        FROM dm_messages m
        JOIN users u ON u.id = m.sender_id
        WHERE m.conversation_id = $1
        ORDER BY m.created_at ASC
        LIMIT 200
        "#,
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    let messages: Vec<DmMessage> = msg_rows
        .into_iter()
        .map(|r| DmMessage {
            id: r.0,
            conversation_id: r.1,
            sender_id: r.2,
            sender_handle: r.3,
            content: r.4,
            created_at: r.5,
        })
        .collect();

    mark_conversation_read(&state, &user.id, &id).await?;

    let last = messages.last();
    Ok(Json(DmThreadResponse {
        conversation: DmConversation {
            id,
            peer_id,
            peer_handle,
            last_message: last.map(|m| m.content.clone()),
            last_at: last.map(|m| m.created_at),
            unread: 0,
        },
        messages,
    }))
}

async fn mark_conversation_read(state: &AppState, user_id: &str, conv_id: &str) -> ApiResult<()> {
    sqlx::query(
        r#"
        INSERT INTO dm_reads (user_id, conversation_id, last_read_at)
        VALUES ($1, $2, NOW())
        ON CONFLICT (user_id, conversation_id)
        DO UPDATE SET last_read_at = EXCLUDED.last_read_at
        "#,
    )
    .bind(user_id)
    .bind(conv_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Start or continue a DM by peer handle.
pub async fn send_to_handle(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SendDmRequest>,
) -> ApiResult<Json<DmMessage>> {
    let handle = body
        .handle
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("handle required"))?;
    let handle = normalize_handle(handle).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let peer_id: Option<String> = sqlx::query_scalar(r#"SELECT id FROM users WHERE handle = $1"#)
        .bind(&handle)
        .fetch_optional(&state.db)
        .await?;
    let peer_id = peer_id.ok_or_else(|| ApiError::not_found("user not found"))?;
    if peer_id == user.id {
        return Err(ApiError::bad_request("cannot DM yourself"));
    }
    if is_blocked_either(&state.db, &user.id, &peer_id).await? {
        return Err(ApiError::forbidden("cannot DM: blocked"));
    }

    let conv_id = find_or_create_conversation(&state, &user.id, &peer_id).await?;
    Ok(Json(
        insert_message(&state, &user, &conv_id, &body.content).await?,
    ))
}

pub async fn send_in_conversation(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<SendDmRequest>,
) -> ApiResult<Json<DmMessage>> {
    ensure_participant(&state, &user.id, &id).await?;

    let peer_id: Option<String> = sqlx::query_scalar(
        r#"SELECT user_id FROM dm_participants WHERE conversation_id = $1 AND user_id <> $2"#,
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await?;
    let peer_id = peer_id.ok_or_else(|| ApiError::not_found("conversation not found"))?;
    if is_blocked_either(&state.db, &user.id, &peer_id).await? {
        return Err(ApiError::forbidden("cannot DM: blocked"));
    }

    Ok(Json(
        insert_message(&state, &user, &id, &body.content).await?,
    ))
}

async fn ensure_participant(state: &AppState, user_id: &str, conv_id: &str) -> ApiResult<()> {
    let ok: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
            SELECT 1 FROM dm_participants WHERE conversation_id = $1 AND user_id = $2
        )"#,
    )
    .bind(conv_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    if !ok {
        return Err(ApiError::not_found("conversation not found"));
    }
    Ok(())
}

fn pair_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}:{b}")
    } else {
        format!("{b}:{a}")
    }
}

async fn find_or_create_conversation(state: &AppState, a: &str, b: &str) -> ApiResult<String> {
    let key = pair_key(a, b);

    // Fast path: canonical pair key (Phase C migration 003).
    if let Some(id) =
        sqlx::query_scalar::<_, String>(r#"SELECT id FROM dm_conversations WHERE pair_key = $1"#)
            .bind(&key)
            .fetch_optional(&state.db)
            .await?
    {
        return Ok(id);
    }

    // Legacy path: conversations created before pair_key existed.
    if let Some(id) = sqlx::query_scalar::<_, String>(
        r#"
        SELECT c.id FROM dm_conversations c
        JOIN dm_participants p1 ON p1.conversation_id = c.id AND p1.user_id = $1
        JOIN dm_participants p2 ON p2.conversation_id = c.id AND p2.user_id = $2
        LIMIT 1
        "#,
    )
    .bind(a)
    .bind(b)
    .fetch_optional(&state.db)
    .await?
    {
        let _ = sqlx::query(
            r#"UPDATE dm_conversations SET pair_key = $1 WHERE id = $2 AND pair_key IS NULL"#,
        )
        .bind(&key)
        .bind(&id)
        .execute(&state.db)
        .await;
        return Ok(id);
    }

    let mut tx = state.db.begin().await?;
    let id = new_id();
    let insert = sqlx::query(
        r#"INSERT INTO dm_conversations (id, pair_key) VALUES ($1, $2)
           ON CONFLICT (pair_key) DO NOTHING"#,
    )
    .bind(&id)
    .bind(&key)
    .execute(&mut *tx)
    .await?;

    if insert.rows_affected() > 0 {
        sqlx::query(r#"INSERT INTO dm_participants (conversation_id, user_id) VALUES ($1, $2)"#)
            .bind(&id)
            .bind(a)
            .execute(&mut *tx)
            .await?;
        sqlx::query(r#"INSERT INTO dm_participants (conversation_id, user_id) VALUES ($1, $2)"#)
            .bind(&id)
            .bind(b)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(id);
    }

    // Concurrent create won the unique pair_key race — re-select.
    tx.rollback().await.ok();
    sqlx::query_scalar::<_, String>(r#"SELECT id FROM dm_conversations WHERE pair_key = $1"#)
        .bind(&key)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::internal("failed to create DM conversation"))
}

async fn insert_message(
    state: &AppState,
    user: &AuthUser,
    conv_id: &str,
    content: &str,
) -> ApiResult<DmMessage> {
    let content = validate_content(content).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let id = new_id();
    let row: (String, String, String, String, DateTime<Utc>) = sqlx::query_as(
        r#"
        INSERT INTO dm_messages (id, conversation_id, sender_id, content)
        VALUES ($1, $2, $3, $4)
        RETURNING id, conversation_id, sender_id, content, created_at
        "#,
    )
    .bind(&id)
    .bind(conv_id)
    .bind(&user.id)
    .bind(&content)
    .fetch_one(&state.db)
    .await?;

    let message = DmMessage {
        id: row.0,
        conversation_id: row.1,
        sender_id: row.2,
        sender_handle: user.handle.clone(),
        content: row.3,
        created_at: row.4,
    };

    tracing::info!(
        "dm        @{}  conv={}  ({} chars)",
        user.handle,
        conv_id,
        message.content.chars().count()
    );

    let _ = state.broadcast_tx.send(Arc::new(Event::DmMessage {
        message: message.clone(),
    }));

    Ok(message)
}
