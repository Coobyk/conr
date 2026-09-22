use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::time::interval;

use crate::shared::Event;

use crate::server::db::is_blocked_either;
use crate::server::error::ApiError;
use crate::server::state::AppState;

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    /// Access token via query (discouraged — prefer Authorization header).
    pub token: Option<String>,
}

pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(q): Query<WsQuery>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let token = q
        .token
        .or_else(|| {
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer ").map(|t| t.to_string()))
        })
        .ok_or_else(|| {
            ApiError::unauthorized("missing token (Authorization: Bearer or ?token=)")
        })?;

    let (user_id, handle) = state.signing_key.decode_access(&token)?;
    tracing::info!("ws        @{}  connected  id={}", handle, user_id);

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state, handle, user_id)))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>, handle: String, user_id: String) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.broadcast_tx.subscribe();
    let mut ping = interval(Duration::from_secs(30));
    let mut events_sent: u64 = 0;

    loop {
        tokio::select! {
            evt = rx.recv() => {
                match evt {
                    Ok(event) => {
                        let Some(event) = event_for_viewer(&state, &user_id, event.as_ref()).await else {
                            continue;
                        };
                        let Ok(text) = serde_json::to_string(&event) else { continue };
                        if sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                        events_sent += 1;
                        match &event {
                            Event::NewPost { post } => {
                                tracing::debug!(
                                    "ws        @{}  ← NewPost from @{} ({})",
                                    handle,
                                    post.author_handle,
                                    post.id
                                );
                            }
                            Event::PostDeleted { post } => {
                                tracing::debug!(
                                    "ws        @{}  ← PostDeleted ({})",
                                    handle,
                                    post.id
                                );
                            }
                            Event::PostEdited { post } => {
                                tracing::debug!(
                                    "ws        @{}  ← PostEdited ({})",
                                    handle,
                                    post.id
                                );
                            }
                            Event::LikeUpdated { post_id, like_count } => {
                                tracing::debug!(
                                    "ws        @{}  ← LikeUpdated {} count={}",
                                    handle,
                                    post_id,
                                    like_count
                                );
                            }
                            Event::Notification { notification } => {
                                tracing::debug!(
                                    "ws        @{}  ← Notification kind={} from @{}",
                                    handle,
                                    notification.kind,
                                    notification.actor_handle
                                );
                            }
                            Event::DmMessage { message } => {
                                tracing::debug!(
                                    "ws        @{}  ← DmMessage conv={}",
                                    handle,
                                    message.conversation_id
                                );
                            }
                            _ => {}
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("ws        @{}  lagged {} event(s)", handle, n);
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Ping(d))) => {
                        let _ = sender.send(Message::Pong(d)).await;
                    }
                    Some(Ok(Message::Text(t))) => {
                        if t == "ping" {
                            let pong = serde_json::to_string(&Event::Pong).unwrap_or_default();
                            if sender.send(Message::Text(pong.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            _ = ping.tick() => {
                if sender.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
        }
    }

    tracing::info!(
        "ws        @{}  disconnected  id={}  pushed {} event(s)",
        handle,
        user_id,
        events_sent
    );
}

/// Server-side fan-out filter: private events only to recipients; hide blocked authors.
async fn event_visible_to(state: &AppState, user_id: &str, event: &Event) -> bool {
    match event {
        Event::Notification { notification } => notification.user_id == user_id,
        Event::DmMessage { message } => {
            is_dm_participant(state, user_id, &message.conversation_id).await
        }
        Event::NewPost { post } | Event::PostEdited { post } | Event::PostDeleted { post } => {
            if post.author_id == user_id {
                return true;
            }
            match is_blocked_either(&state.db, user_id, &post.author_id).await {
                Ok(blocked) => !blocked,
                Err(e) => {
                    tracing::warn!(?e, "ws block check failed — dropping event");
                    false
                }
            }
        }
        // Like counts are public engagement stats (no private content).
        Event::LikeUpdated { .. } | Event::Pong | Event::Error { .. } => true,
    }
}

pub(crate) async fn event_for_viewer(
    state: &AppState,
    user_id: &str,
    event: &Event,
) -> Option<Event> {
    if !event_visible_to(state, user_id, event).await {
        return None;
    }
    let mut event = event.clone();
    match &mut event {
        Event::NewPost { post } | Event::PostEdited { post } | Event::PostDeleted { post } => {
            *post = match crate::server::db::fetch_post(&state.db, &post.id, user_id).await {
                Ok(Some(post)) => post,
                Ok(None) => return None,
                Err(e) => {
                    tracing::warn!(?e, "ws post hydration failed — dropping event");
                    return None;
                }
            };
        }
        _ => {}
    }
    Some(event)
}

async fn is_dm_participant(state: &AppState, user_id: &str, conv_id: &str) -> bool {
    match sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(
            SELECT 1 FROM dm_participants
            WHERE conversation_id = $1 AND user_id = $2
        )"#,
    )
    .bind(conv_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(ok) => ok,
        Err(e) => {
            tracing::warn!(?e, "ws dm participant check failed — dropping event");
            false
        }
    }
}
