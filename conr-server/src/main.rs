mod auth;
mod error;
mod handlers;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Default: our app at info, hush noisy crates. Override with RUST_LOG=...
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new(
                "info,conr_server=info,tower_http=info,sqlx=warn,hyper=warn,h2=warn",
            )
        }))
        .with_target(false)
        .with_level(true)
        .compact()
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://localhost/conr".to_string());
    let port: u16 = std::env::var("CONR_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let signing_key = auth::load_signing_key()?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    let (broadcast_tx, _) = broadcast::channel(1024);
    let state = Arc::new(AppState {
        db: pool,
        broadcast_tx,
        signing_key,
    });

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/auth/register", post(handlers::auth::register))
        .route("/auth/login", post(handlers::auth::login))
        .route("/auth/refresh", post(handlers::auth::refresh))
        .route("/feed", get(handlers::posts::feed))
        .route("/posts", post(handlers::posts::create_post))
        .route("/ws", get(handlers::ws::ws_upgrade))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("conr-server listening on {addr}");
    tracing::info!("logging  register / login / feed / post / websocket at info level");
    tracing::info!("         set RUST_LOG=debug for more detail (e.g. per-ws NewPost fan-out)");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
