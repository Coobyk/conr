use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub handle: String,
    pub bio: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub handle: String,
    pub bio: Option<String>,
    pub created_at: DateTime<Utc>,
    pub followers: i64,
    pub following: i64,
    pub posts: i64,
    #[serde(default)]
    pub followed_by_me: bool,
    #[serde(default)]
    pub follows_me: bool,
    #[serde(default)]
    pub blocked_by_me: bool,
    #[serde(default)]
    pub is_me: bool,
}

/// Uploaded image metadata (bytes live at GET /media/:id).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaInfo {
    pub id: String,
    pub content_type: String,
    pub size_bytes: i64,
    #[serde(default)]
    pub width: Option<i32>,
    #[serde(default)]
    pub height: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotedPost {
    pub id: String,
    pub author_handle: String,
    pub content: String,
    pub deleted_at: Option<DateTime<Utc>>,
    pub images: Vec<MediaInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub author_id: String,
    pub author_handle: String,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub quote_id: Option<String>,
    #[serde(default)]
    pub quote: Option<QuotedPost>,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub edited_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub like_count: i64,
    #[serde(default)]
    pub liked_by_me: bool,
    #[serde(default)]
    pub reply_count: i64,
    /// Attached images, ordered. Empty when none or post is tombstoned.
    #[serde(default)]
    pub images: Vec<MediaInfo>,
}

impl Post {
    /// Display content for UI (tombstones always show as [Deleted]).
    pub fn display_content(&self) -> &str {
        if self.deleted_at.is_some() {
            "[Deleted]"
        } else if self.content.is_empty() && !self.images.is_empty() {
            ""
        } else {
            &self.content
        }
    }

    pub fn has_images(&self) -> bool {
        self.deleted_at.is_none() && !self.images.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterResponse {
    /// Raw identity token — shown once, never stored server-side.
    pub token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub user: User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub handle: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user: User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePostRequest {
    pub content: String,
    /// Parent post id for replies; null for top-level.
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub quote_id: Option<String>,
    /// Previously uploaded media ids (max 4). Order is preserved.
    #[serde(default)]
    pub image_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPostRequest {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateBioRequest {
    pub bio: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockRequest {
    /// Required explanation for the block (1–280 chars after trim).
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedResponse {
    pub posts: Vec<Post>,
    /// Cursor for next page: last (oldest) post id in this page, if any.
    pub next_cursor: Option<String>,
}

/// Full thread: root post plus every descendant (flat, chronological).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResponse {
    pub root: Post,
    pub posts: Vec<Post>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LikeResponse {
    pub post_id: String,
    pub like_count: i64,
    pub liked_by_me: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub posts: Vec<Post>,
    pub users: Vec<UserProfile>,
    pub tags: Vec<TagInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagInfo {
    pub name: String,
    pub post_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Like,
    Reply,
    Follow,
    Mention,
}

impl NotificationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Like => "like",
            Self::Reply => "reply",
            Self::Follow => "follow",
            Self::Mention => "mention",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "like" => Some(Self::Like),
            "reply" => Some(Self::Reply),
            "follow" => Some(Self::Follow),
            "mention" => Some(Self::Mention),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    /// Recipient — clients filter WS events to their own user id.
    #[serde(default)]
    pub user_id: String,
    pub kind: String,
    pub actor_handle: String,
    pub post_id: Option<String>,
    pub read: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationsResponse {
    pub notifications: Vec<Notification>,
    pub unread: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmConversation {
    pub id: String,
    pub peer_handle: String,
    pub peer_id: String,
    pub last_message: Option<String>,
    pub last_at: Option<DateTime<Utc>>,
    /// Messages from the peer newer than this viewer's last read.
    #[serde(default)]
    pub unread: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmMessage {
    pub id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub sender_handle: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmListResponse {
    pub conversations: Vec<DmConversation>,
    /// Sum of per-conversation `unread`.
    #[serde(default)]
    pub unread: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmThreadResponse {
    pub conversation: DmConversation,
    pub messages: Vec<DmMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendDmRequest {
    /// Peer handle (for starting a new DM) or ignored if conversation already open via path.
    #[serde(default)]
    pub handle: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_post_and_create_request_default_to_no_quote() {
        let post: Post = serde_json::from_value(json!({
            "id": "post", "author_id": "author", "author_handle": "alice",
            "parent_id": null, "content": "hello", "created_at": "2026-09-17T00:00:00Z",
            "deleted_at": null
        }))
        .unwrap();
        assert!(post.quote_id.is_none());
        assert!(post.quote.is_none());
        assert!(post.images.is_empty());
        let request: CreatePostRequest =
            serde_json::from_value(json!({"content": "hello"})).unwrap();
        assert!(request.quote_id.is_none());
        assert!(request.parent_id.is_none());
        assert!(request.image_ids.is_empty());
    }

    #[test]
    fn quote_payload_round_trips_with_images_and_unavailable_original() {
        let value = json!({
            "id": "post", "author_id": "author", "author_handle": "alice",
            "parent_id": null, "content": "commentary", "created_at": "2026-09-17T00:00:00Z",
            "deleted_at": null, "quote_id": "original",
            "quote": {
                "id": "original", "author_handle": "bob", "content": "original text",
                "deleted_at": null,
                "images": [{"id": "image", "content_type": "image/png", "size_bytes": 42}]
            }
        });
        let post: Post = serde_json::from_value(value).unwrap();
        let mut post: Post = serde_json::from_str(&serde_json::to_string(&post).unwrap()).unwrap();
        assert_eq!(post.quote_id.as_deref(), Some("original"));
        let quote = post.quote.as_ref().unwrap();
        assert_eq!(quote.id, "original");
        assert_eq!(quote.author_handle, "bob");
        assert_eq!(quote.images[0].id, "image");
        post.quote = None;
        let post: Post = serde_json::from_value(serde_json::to_value(post).unwrap()).unwrap();
        assert_eq!(post.quote_id.as_deref(), Some("original"));
        assert!(post.quote.is_none());
    }
}
