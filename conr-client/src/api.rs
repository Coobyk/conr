use anyhow::{bail, Context, Result};
use conr_shared::{
    AuthResponse, CreatePostRequest, FeedResponse, LoginRequest, Post, RegisterRequest,
    RegisterResponse,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;

#[derive(Clone)]
pub struct Api {
    client: Client,
    base: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

impl Api {
    pub fn new(server_url: &str) -> Result<Self> {
        let base = server_url.trim_end_matches('/').to_string();
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            base,
            access_token: None,
            refresh_token: None,
        })
    }

    pub fn set_tokens(&mut self, access: String, refresh: String) {
        self.access_token = Some(access);
        self.refresh_token = Some(refresh);
    }

    pub async fn register(&mut self, handle: &str) -> Result<RegisterResponse> {
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
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("register failed ({status}): {body}");
        }
        let body: RegisterResponse = resp.json().await?;
        self.set_tokens(body.access_token.clone(), body.refresh_token.clone());
        Ok(body)
    }

    pub async fn login(&mut self, handle: &str, token: &str) -> Result<AuthResponse> {
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
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("login failed ({status}): {body}");
        }
        let body: AuthResponse = resp.json().await?;
        self.set_tokens(body.access_token.clone(), body.refresh_token.clone());
        Ok(body)
    }

    pub async fn feed(&self, cursor: Option<&str>) -> Result<FeedResponse> {
        let mut url = format!("{}/feed?limit=50", self.base);
        if let Some(c) = cursor {
            url.push_str(&format!("&cursor={c}"));
        }
        let mut req = self.client.get(&url);
        if let Some(t) = &self.access_token {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = req.send().await.context("feed request")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("feed failed ({status}): {body}");
        }
        Ok(resp.json().await?)
    }

    pub async fn create_post(&self, content: &str) -> Result<Post> {
        let url = format!("{}/posts", self.base);
        let mut req = self
            .client
            .post(&url)
            .header(CONTENT_TYPE, "application/json")
            .json(&CreatePostRequest {
                content: content.to_string(),
                parent_id: None,
            });
        if let Some(t) = &self.access_token {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = req.send().await.context("create post")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("create post failed ({status}): {body}");
        }
        Ok(resp.json().await?)
    }

    pub fn ws_url(&self) -> Result<String> {
        let access = self
            .access_token
            .as_ref()
            .context("not authenticated")?;
        let base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}/ws?token={access}")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}/ws?token={access}")
        } else {
            format!("{}/ws?token={access}", self.base)
        };
        Ok(base)
    }
}
