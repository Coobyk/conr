use serde::{Deserialize, Serialize};

use crate::models::Post;

/// WebSocket envelope: `{ "type": "...", "payload": ... }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum Event {
    NewPost { post: Post },
    Pong,
    Error { message: String },
}
