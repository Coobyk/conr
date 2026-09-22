use serde::{Deserialize, Serialize};

use crate::shared::models::{DmMessage, Notification, Post};

/// WebSocket envelope: `{ "type": "...", "payload": ... }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum Event {
    NewPost {
        post: Post,
    },
    /// Tombstoned post (content already scrubbed / display as [Deleted]).
    PostDeleted {
        post: Post,
    },
    PostEdited {
        post: Post,
    },
    /// Like count changed on a post.
    LikeUpdated {
        post_id: String,
        like_count: i64,
    },
    Notification {
        notification: Notification,
    },
    DmMessage {
        message: DmMessage,
    },
    Pong,
    Error {
        message: String,
    },
}
