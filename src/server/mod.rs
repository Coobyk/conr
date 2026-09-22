pub mod auth;
pub mod db;
pub mod error;
pub mod handlers;
pub mod ratelimit;
pub mod state;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, patch, post};
use axum::Router;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::server::ratelimit::RateLimiter;
use crate::server::state::AppState;

fn resolve_media_dir() -> anyhow::Result<std::path::PathBuf> {
    if let Ok(p) = std::env::var("CONR_MEDIA_DIR") {
        let path = std::path::PathBuf::from(p);
        std::fs::create_dir_all(&path)?;
        return Ok(path);
    }
    let path = if let Some(base) = dirs::data_local_dir() {
        base.join("conr-server").join("media")
    } else {
        std::path::PathBuf::from("data/media")
    };
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

/// Initialize tracing for the server process (idempotent-ish: first wins).
pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("info,conr_server=info,tower_http=info,sqlx=warn,hyper=warn,h2=warn")
        }))
        .with_target(false)
        .with_level(true)
        .compact()
        .try_init();
}

/// Bind and serve the conr HTTP API until the process exits.
pub async fn run() -> anyhow::Result<()> {
    init_tracing();

    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        // whoami can return "anonymous" on some systems; prefer $USER.
        let user = std::env::var("USER").unwrap_or_else(|_| "postgres".into());
        format!("postgres://{user}@localhost/conr")
    });
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

    let media_dir = resolve_media_dir()?;
    tracing::info!("media dir {}", media_dir.display());

    let (broadcast_tx, _) = broadcast::channel(1024);
    let state = Arc::new(AppState {
        db: pool,
        broadcast_tx,
        signing_key,
        media_dir,
        login_limiter: RateLimiter::new(Duration::from_secs(15 * 60), 20),
        register_limiter: RateLimiter::new(Duration::from_secs(60 * 60), 10),
    });

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/auth/register", post(handlers::auth::register))
        .route("/auth/login", post(handlers::auth::login))
        .route("/auth/refresh", post(handlers::auth::refresh))
        .route("/feed", get(handlers::posts::feed))
        .route("/posts", post(handlers::posts::create_post))
        .route(
            "/posts/{id}",
            get(handlers::posts::get_thread)
                .patch(handlers::posts::edit_post)
                .delete(handlers::posts::delete_post),
        )
        .route("/posts/{id}/like", post(handlers::posts::toggle_like))
        .route(
            "/media",
            post(handlers::media::upload).layer(handlers::media::upload_body_limit()),
        )
        .route("/media/{id}", get(handlers::media::get))
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
        .route("/search", get(handlers::search::search))
        .route("/tags/{tag}", get(handlers::search::tag_posts))
        .route("/notifications", get(handlers::notifications::list))
        .route(
            "/notifications/read",
            post(handlers::notifications::mark_read),
        )
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
    tracing::info!("media: POST /media · GET /media/:id · up to 4 images / post");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
