//! Combined Axum server: serves the main website + poker WebSocket game.
//!
//! # Routes
//!
//! | Method | Path            | Description                                |
//! |--------|-----------------|------------------------------------------- |
//! | `GET`  | `/ws`           | WebSocket upgrade for game connections     |
//! | `GET`  | `/api/rooms`    | List active room IDs (JSON)                |
//! | `GET`  | `/poker/*`      | Poker Dioxus SPA (fallback: poker/index.html) |
//! | `GET`  | `/*`            | Main site static files (fallback: index.html) |
//!
//! Set `STATIC_DIR` to point at the combined static output (default: `./dist`).

mod room;
mod ws_handler;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::WebSocketUpgrade;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

use room::RoomManager;

/// Shared application state available to all handlers.
#[derive(Clone)]
struct AppState {
    room_manager: Arc<RoomManager>,
}

#[tokio::main]
async fn main() {
    // Initialise tracing (respects RUST_LOG env var).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let state = AppState {
        room_manager: Arc::new(RoomManager::new()),
    };

    // Static file directory for the combined site output.
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "./dist".to_string());

    // Poker SPA: /poker/* routes, fallback to /poker/index.html for client-side routing.
    let poker_spa = ServeDir::new(format!("{static_dir}/poker"))
        .not_found_service(ServeFile::new(format!("{static_dir}/poker/index.html")));

    // Main site: everything else falls back to /index.html.
    let main_site = ServeDir::new(&static_dir)
        .not_found_service(ServeFile::new(format!("{static_dir}/index.html")));

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/rooms", get(rooms_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
        .nest_service("/poker", poker_spa)
        .fallback_service(main_site);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Poker server listening on {addr}");
    tracing::info!("Serving static files from {static_dir}");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// `GET /ws` — upgrade to WebSocket and hand off to [`ws_handler::handle_socket`].
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_handler::handle_socket(socket, state.room_manager))
}

/// `GET /api/rooms` — return a JSON array of active room IDs.
async fn rooms_handler(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.room_manager.list_rooms().await)
}
