use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::time::interval;

use conr_shared::Event;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    /// Access token (TUI clients pass via query; Authorization also works if set).
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
        .ok_or_else(|| ApiError::unauthorized("missing token (query ?token= or Authorization)"))?;

    let (user_id, handle) = state.signing_key.decode_access(&token)?;
    tracing::info!("ws        @{}  connected  id={}", handle, user_id);

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state, handle, user_id)))
}

async fn handle_socket(
    socket: WebSocket,
    state: Arc<AppState>,
    handle: String,
    user_id: String,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.broadcast_tx.subscribe();
    let mut ping = interval(Duration::from_secs(30));
    let mut events_sent: u64 = 0;

    loop {
        tokio::select! {
            evt = rx.recv() => {
                match evt {
                    Ok(event) => {
                        let Ok(text) = serde_json::to_string(event.as_ref()) else { continue };
                        if sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                        events_sent += 1;
                        if let Event::NewPost { post } = event.as_ref() {
                            tracing::debug!(
                                "ws        @{}  ← NewPost from @{} ({})",
                                handle,
                                post.author_handle,
                                post.id
                            );
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
