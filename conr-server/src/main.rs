mod auth;
mod db;
mod error;
mod handlers;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::{get, patch, post};
use axum::Router;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://localhost/conr".to_string());
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
        // auth
        .route("/auth/register", post(handlers::auth::register))
        .route("/auth/login", post(handlers::auth::login))
        .route("/auth/refresh", post(handlers::auth::refresh))
        // feed + posts
        .route("/feed", get(handlers::posts::feed))
        .route("/posts", post(handlers::posts::create_post))
        .route(
            "/posts/{id}",
            get(handlers::posts::get_thread)
                .patch(handlers::posts::edit_post)
                .delete(handlers::posts::delete_post),
        )
        .route("/posts/{id}/like", post(handlers::posts::toggle_like))
        // social
        .route("/users/{handle}", get(handlers::social::get_profile))
        .route(
            "/users/{handle}/posts",
            get(handlers::social::profile_posts),
        )
        .route(
            "/users/{handle}/follow",
            post(handlers::social::follow).delete(handlers::social::unfollow),
        )
        .route(
            "/users/{handle}/block",
            post(handlers::social::block).delete(handlers::social::unblock),
        )
        .route("/me/bio", patch(handlers::social::update_bio))
        .route("/me/following", get(handlers::social::list_following))
        .route("/me/blocks", get(handlers::social::list_blocks))
        // search + tags
        .route("/search", get(handlers::search::search))
        .route("/tags/{tag}", get(handlers::search::tag_posts))
        // notifications
        .route("/notifications", get(handlers::notifications::list))
        .route(
            "/notifications/read",
            post(handlers::notifications::mark_read),
        )
        // DMs
        .route(
            "/dms",
            get(handlers::dms::list_conversations).post(handlers::dms::send_to_handle),
        )
        .route(
            "/dms/{id}",
            get(handlers::dms::get_conversation).post(handlers::dms::send_in_conversation),
        )
        .route("/ws", get(handlers::ws::ws_upgrade))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("conr-server listening on {addr}");
    tracing::info!("phase C: follows · blocks · search · tags · notifs · DMs · edit");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
