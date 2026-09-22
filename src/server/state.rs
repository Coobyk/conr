use std::path::PathBuf;
use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::shared::Event;

use crate::server::auth::SigningKey;
use crate::server::ratelimit::RateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub broadcast_tx: broadcast::Sender<Arc<Event>>,
    pub signing_key: SigningKey,
    /// Directory for uploaded image bytes (`{id}` files).
    pub media_dir: PathBuf,
    /// Per-IP limit on login attempts (20 / 15 min).
    pub login_limiter: RateLimiter,
    /// Per-IP limit on account creation (10 / hour).
    pub register_limiter: RateLimiter,
}
