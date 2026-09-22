use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::shared::{hash_token, new_id, strip_token, validate_token, User};

use crate::server::error::{ApiError, ApiResult};
use crate::server::state::AppState;

const ACCESS_TTL_SECS: u64 = 3600;
const REFRESH_TTL_DAYS: i64 = 14;

#[derive(Clone)]
pub struct SigningKey {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl SigningKey {
    pub fn from_seed(seed: &[u8; 32]) -> anyhow::Result<Self> {
        // CONR_SIGNING_KEY is a 32-byte seed (base64). JWTs use HS256 with that secret.
        // (Ed25519 JWT wiring can replace this later without changing the env var shape.)
        Ok(Self {
            encoding: EncodingKey::from_secret(seed),
            decoding: DecodingKey::from_secret(seed),
        })
    }

    pub fn decode_access(&self, token: &str) -> ApiResult<(String, String)> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;
        validation.leeway = 5;
        let data = decode::<Claims>(token, &self.decoding, &validation)
            .map_err(|_| ApiError::unauthorized("invalid or expired access token"))?;
        Ok((data.claims.sub, data.claims.handle))
    }
}

/// Load 32-byte signing seed from CONR_SIGNING_KEY (base64) or generate ephemeral (dev only).
pub fn load_signing_key() -> anyhow::Result<SigningKey> {
    if let Ok(b64) = std::env::var("CONR_SIGNING_KEY") {
        let bytes = B64
            .decode(b64.trim())
            .map_err(|e| anyhow::anyhow!("CONR_SIGNING_KEY base64 decode: {e}"))?;
        if bytes.len() != 32 {
            anyhow::bail!(
                "CONR_SIGNING_KEY must decode to 32 bytes, got {}",
                bytes.len()
            );
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes);
        return SigningKey::from_seed(&seed);
    }

    tracing::warn!("CONR_SIGNING_KEY not set — generating ephemeral key (dev only)");
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    tracing::info!("dev CONR_SIGNING_KEY={}", B64.encode(seed));
    SigningKey::from_seed(&seed)
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    handle: String,
    exp: u64,
    iat: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_secs()
}

pub fn issue_access_token(key: &SigningKey, user_id: &str, handle: &str) -> ApiResult<String> {
    let iat = now_secs();
    let claims = Claims {
        sub: user_id.to_string(),
        handle: handle.to_string(),
        iat,
        exp: iat + ACCESS_TTL_SECS,
    };
    encode(&Header::default(), &claims, &key.encoding)
        .map_err(|e| ApiError::internal(format!("jwt encode: {e}")))
}

pub async fn issue_refresh_token(pool: &PgPool, user_id: &str) -> ApiResult<String> {
    let raw = format!("rfr_{}", new_id()) + &new_id();
    let id = new_id();
    let hash = hash_token(&raw);
    let expires = Utc::now() + ChronoDuration::days(REFRESH_TTL_DAYS);
    sqlx::query(
        r#"INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at)
           VALUES ($1, $2, $3, $4)"#,
    )
    .bind(&id)
    .bind(user_id)
    .bind(&hash[..])
    .bind(expires)
    .execute(pool)
    .await?;
    Ok(raw)
}

pub async fn rotate_refresh_token(pool: &PgPool, raw: &str) -> ApiResult<(String, User)> {
    let hash = hash_token(raw);
    let row = sqlx::query_as::<_, (String, String, chrono::DateTime<Utc>)>(
        r#"SELECT id, user_id, expires_at FROM refresh_tokens WHERE token_hash = $1"#,
    )
    .bind(&hash[..])
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("invalid refresh token"))?;

    let (id, user_id, expires_at) = row;
    if expires_at < Utc::now() {
        sqlx::query("DELETE FROM refresh_tokens WHERE id = $1")
            .bind(&id)
            .execute(pool)
            .await?;
        return Err(ApiError::unauthorized("refresh token expired"));
    }

    sqlx::query("DELETE FROM refresh_tokens WHERE id = $1")
        .bind(&id)
        .execute(pool)
        .await?;

    let user = fetch_user(pool, &user_id).await?;
    let new_refresh = issue_refresh_token(pool, &user_id).await?;
    Ok((new_refresh, user))
}

pub async fn fetch_user(pool: &PgPool, user_id: &str) -> ApiResult<User> {
    let row = sqlx::query_as::<_, (String, String, Option<String>, chrono::DateTime<Utc>)>(
        r#"SELECT id, handle, bio, created_at FROM users WHERE id = $1"#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("user not found"))?;

    Ok(User {
        id: row.0,
        handle: row.1,
        bio: row.2,
        created_at: row.3,
    })
}

pub async fn fetch_user_by_handle(
    pool: &PgPool,
    handle: &str,
) -> ApiResult<Option<(User, Vec<u8>)>> {
    let row =
        sqlx::query_as::<
            _,
            (
                String,
                String,
                Option<String>,
                chrono::DateTime<Utc>,
                Vec<u8>,
            ),
        >(r#"SELECT id, handle, bio, created_at, token_hash FROM users WHERE handle = $1"#)
        .bind(handle)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| {
        (
            User {
                id: r.0,
                handle: r.1,
                bio: r.2,
                created_at: r.3,
            },
            r.4,
        )
    }))
}

/// Constant-time compare of token hashes.
pub fn token_matches(stored: &[u8], candidate_token: &str) -> bool {
    let cleaned = strip_token(candidate_token);
    if validate_token(&cleaned).is_err() {
        return false;
    }
    let got = hash_token(&cleaned);
    if stored.len() != got.len() {
        return false;
    }
    // constant-time-ish
    let mut diff = 0u8;
    for (a, b) in stored.iter().zip(got.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub handle: String,
}

impl FromRequestParts<std::sync::Arc<AppState>> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let auth = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?;

        let token = auth
            .strip_prefix("Bearer ")
            .ok_or_else(|| ApiError::unauthorized("expected Bearer token"))?;

        let (id, handle) = state.signing_key.decode_access(token)?;
        Ok(AuthUser { id, handle })
    }
}
