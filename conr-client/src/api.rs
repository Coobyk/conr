use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use conr_shared::{
    AuthResponse, BlockRequest, CreatePostRequest, DmListResponse, DmMessage, DmThreadResponse,
    EditPostRequest, FeedResponse, LikeResponse, LoginRequest, NotificationsResponse, Post,
    RefreshRequest, RegisterRequest, RegisterResponse, SearchResponse, SendDmRequest,
    ThreadResponse, UpdateBioRequest, UserProfile,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, StatusCode};

#[derive(Default)]
struct Tokens {
    access: Option<String>,
    refresh: Option<String>,
}

#[derive(Clone)]
pub struct Api {
    client: Client,
    base: String,
    tokens: Arc<Mutex<Tokens>>,
}

impl Api {
    pub fn new(server_url: &str) -> Result<Self> {
        let base = Self::normalize_base(server_url);
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            base,
            tokens: Arc::new(Mutex::new(Tokens::default())),
        })
    }

    fn normalize_base(server_url: &str) -> String {
        server_url.trim().trim_end_matches('/').to_string()
    }

    /// Point the client at a different server (login / settings). Clears nothing else.
    pub fn set_server_url(&mut self, server_url: &str) {
        self.base = Self::normalize_base(server_url);
    }

    pub fn set_tokens(&self, access: String, refresh: String) {
        let mut t = self.tokens.lock().expect("tokens lock");
        t.access = Some(access);
        t.refresh = Some(refresh);
    }

    pub fn clear_tokens(&self) {
        let mut t = self.tokens.lock().expect("tokens lock");
        t.access = None;
        t.refresh = None;
    }

    pub fn access_token(&self) -> Option<String> {
        self.tokens.lock().expect("tokens lock").access.clone()
    }

    pub fn refresh_token(&self) -> Option<String> {
        self.tokens.lock().expect("tokens lock").refresh.clone()
    }

    fn auth(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(t) = self.access_token() {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        req
    }

    async fn check(resp: reqwest::Response, what: &str) -> Result<reqwest::Response> {
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{what} failed ({status}): {body}");
        }
        Ok(resp)
    }

    /// Rotate access/refresh tokens. Returns false if no refresh token is stored.
    pub async fn refresh_session(&self) -> Result<bool> {
        let Some(refresh) = self.refresh_token() else {
            return Ok(false);
        };
        let url = format!("{}/auth/refresh", self.base);
        let resp = self
            .client
            .post(&url)
            .json(&RefreshRequest {
                refresh_token: refresh,
            })
            .send()
            .await
            .context("refresh request")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            // Stale refresh — drop tokens so caller can re-login.
            self.clear_tokens();
            bail!("refresh failed ({status}): {body}");
        }
        let body: AuthResponse = resp.json().await?;
        self.set_tokens(body.access_token, body.refresh_token);
        Ok(true)
    }

    /// Send an authenticated request; on 401, refresh once and retry.
    async fn send_authed(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
        what: &str,
    ) -> Result<reqwest::Response> {
        let resp = self.auth(build()).send().await.with_context(|| format!("{what} request"))?;
        if resp.status() != StatusCode::UNAUTHORIZED {
            return Self::check(resp, what).await;
        }
        // Drop body and try session refresh.
        let _ = resp.text().await;
        match self.refresh_session().await {
            Ok(true) => {}
            Ok(false) => bail!("{what} failed (401): session expired — log in again"),
            Err(e) => bail!("{what} failed (401): {e:#}"),
        }
        let resp = self.auth(build()).send().await.with_context(|| format!("{what} retry"))?;
        Self::check(resp, what).await
    }

    pub async fn register(&self, handle: &str) -> Result<RegisterResponse> {
        let url = format!("{}/auth/register", self.base);
        let resp = self
            .client
            .post(&url)
            .json(&RegisterRequest {
                handle: handle.to_string(),
            })
            .send()
            .await
            .context("register request")?;
        let resp = Self::check(resp, "register").await?;
        let body: RegisterResponse = resp.json().await?;
        self.set_tokens(body.access_token.clone(), body.refresh_token.clone());
        Ok(body)
    }

    pub async fn login(&self, handle: &str, token: &str) -> Result<AuthResponse> {
        let url = format!("{}/auth/login", self.base);
        let resp = self
            .client
            .post(&url)
            .json(&LoginRequest {
                handle: handle.to_string(),
                token: token.to_string(),
            })
            .send()
            .await
            .context("login request")?;
        let resp = Self::check(resp, "login").await?;
        let body: AuthResponse = resp.json().await?;
        self.set_tokens(body.access_token.clone(), body.refresh_token.clone());
        Ok(body)
    }

    pub async fn feed(&self, cursor: Option<&str>, following: bool) -> Result<FeedResponse> {
        let filter = if following { "following" } else { "all" };
        let mut url = format!("{}/feed?limit=50&filter={filter}", self.base);
        if let Some(c) = cursor {
            url.push_str("&cursor=");
            url.push_str(&urlencoding_lite(c));
        }
        let url = url;
        let resp = self
            .send_authed(|| self.client.get(&url), "feed")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn create_post(&self, content: &str, parent_id: Option<&str>) -> Result<Post> {
        let url = format!("{}/posts", self.base);
        let body = CreatePostRequest {
            content: content.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
        };
        let resp = self
            .send_authed(
                || {
                    self.client
                        .post(&url)
                        .header(CONTENT_TYPE, "application/json")
                        .json(&body)
                },
                "create post",
            )
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn edit_post(&self, post_id: &str, content: &str) -> Result<Post> {
        let url = format!("{}/posts/{post_id}", self.base);
        let body = EditPostRequest {
            content: content.to_string(),
        };
        let resp = self
            .send_authed(|| self.client.patch(&url).json(&body), "edit post")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn get_thread(&self, post_id: &str) -> Result<ThreadResponse> {
        let url = format!("{}/posts/{post_id}", self.base);
        let resp = self
            .send_authed(|| self.client.get(&url), "thread")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn toggle_like(&self, post_id: &str) -> Result<LikeResponse> {
        let url = format!("{}/posts/{post_id}/like", self.base);
        let resp = self
            .send_authed(|| self.client.post(&url), "like")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn delete_post(&self, post_id: &str) -> Result<Post> {
        let url = format!("{}/posts/{post_id}", self.base);
        let resp = self
            .send_authed(|| self.client.delete(&url), "delete")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn get_profile(&self, handle: &str) -> Result<UserProfile> {
        let url = format!("{}/users/{handle}", self.base);
        let resp = self
            .send_authed(|| self.client.get(&url), "profile")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn profile_posts(
        &self,
        handle: &str,
        cursor: Option<&str>,
    ) -> Result<FeedResponse> {
        let mut url = format!("{}/users/{handle}/posts?limit=50", self.base);
        if let Some(c) = cursor {
            url.push_str("&cursor=");
            url.push_str(&urlencoding_lite(c));
        }
        let url = url;
        let resp = self
            .send_authed(|| self.client.get(&url), "profile posts")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn follow(&self, handle: &str) -> Result<UserProfile> {
        let url = format!("{}/users/{handle}/follow", self.base);
        let resp = self
            .send_authed(|| self.client.post(&url), "follow")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn unfollow(&self, handle: &str) -> Result<UserProfile> {
        let url = format!("{}/users/{handle}/follow", self.base);
        let resp = self
            .send_authed(|| self.client.delete(&url), "unfollow")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn block(&self, handle: &str, reason: &str) -> Result<UserProfile> {
        let url = format!("{}/users/{handle}/block", self.base);
        let body = BlockRequest {
            reason: reason.to_string(),
        };
        let resp = self
            .send_authed(|| self.client.post(&url).json(&body), "block")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn unblock(&self, handle: &str) -> Result<()> {
        let url = format!("{}/users/{handle}/block", self.base);
        let resp = self
            .send_authed(|| self.client.delete(&url), "unblock")
            .await?;
        let _ = resp;
        Ok(())
    }

    pub async fn update_bio(&self, bio: Option<&str>) -> Result<UserProfile> {
        let url = format!("{}/me/bio", self.base);
        let body = UpdateBioRequest {
            bio: bio.map(|s| s.to_string()),
        };
        let resp = self
            .send_authed(|| self.client.patch(&url).json(&body), "update bio")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn list_following(&self) -> Result<Vec<String>> {
        let url = format!("{}/me/following", self.base);
        let resp = self
            .send_authed(|| self.client.get(&url), "list following")
            .await?;
        let v: serde_json::Value = resp.json().await?;
        Ok(v.get("handles")
            .and_then(|h| h.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn list_blocks(&self) -> Result<Vec<String>> {
        let url = format!("{}/me/blocks", self.base);
        let resp = self
            .send_authed(|| self.client.get(&url), "list blocks")
            .await?;
        let v: serde_json::Value = resp.json().await?;
        Ok(v.get("handles")
            .and_then(|h| h.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn search(&self, q: &str) -> Result<SearchResponse> {
        let url = format!("{}/search?q={}", self.base, urlencoding_lite(q));
        let resp = self
            .send_authed(|| self.client.get(&url), "search")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn notifications(&self) -> Result<NotificationsResponse> {
        let url = format!("{}/notifications?limit=50", self.base);
        let resp = self
            .send_authed(|| self.client.get(&url), "notifications")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn mark_notifications_read(&self) -> Result<()> {
        let url = format!("{}/notifications/read", self.base);
        let _ = self
            .send_authed(|| self.client.post(&url), "mark read")
            .await?;
        Ok(())
    }

    pub async fn list_dms(&self) -> Result<DmListResponse> {
        let url = format!("{}/dms", self.base);
        let resp = self
            .send_authed(|| self.client.get(&url), "dms list")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn get_dm(&self, id: &str) -> Result<DmThreadResponse> {
        let url = format!("{}/dms/{id}", self.base);
        let resp = self
            .send_authed(|| self.client.get(&url), "dm thread")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn send_dm_to(&self, handle: &str, content: &str) -> Result<DmMessage> {
        let url = format!("{}/dms", self.base);
        let body = SendDmRequest {
            handle: Some(handle.to_string()),
            content: content.to_string(),
        };
        let resp = self
            .send_authed(|| self.client.post(&url).json(&body), "send dm")
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn send_dm_in(&self, conv_id: &str, content: &str) -> Result<DmMessage> {
        let url = format!("{}/dms/{conv_id}", self.base);
        let body = SendDmRequest {
            handle: None,
            content: content.to_string(),
        };
        let resp = self
            .send_authed(|| self.client.post(&url).json(&body), "send dm")
            .await?;
        Ok(resp.json().await?)
    }

    /// WebSocket URL without embedding the access token (use Authorization header).
    pub fn ws_url(&self) -> Result<String> {
        let base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}/ws")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}/ws")
        } else {
            format!("{}/ws", self.base)
        };
        Ok(base)
    }
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
