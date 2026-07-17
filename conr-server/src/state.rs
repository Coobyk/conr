use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::broadcast;

use conr_shared::Event;

use crate::auth::SigningKey;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub broadcast_tx: broadcast::Sender<Arc<Event>>,
    pub signing_key: SigningKey,
}
