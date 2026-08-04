// Copyright (C) 2026 Lluc Simó Margalef <lluc.simo@protonmail.com>
// SPDX-License-Identifier: GPL-3.0-only
//
// This file is part of acrilique/poker.
//
// poker is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, version 3 of the License.
//
// poker is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with poker. If not, see <https://www.gnu.org/licenses/>.

//! Datastar (SSE) poker server.
//!
//! One long-lived `GET /poker/events` SSE stream per player (reads) plus
//! short-lived action POSTs (writes) — the CQRS pattern from the Datastar
//! Tao. The game engine lives in `poker-core` (`game_logic` + the poker
//! primitives) and is shared with the WebSocket server; only the delivery
//! layer differs.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{NotForContentType, Predicate, SizeAbove};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

use poker_sse_server::AppState;
use poker_sse_server::handlers;
use poker_sse_server::room::RoomManager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialise tracing (respects RUST_LOG env var).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let state = AppState {
        room_manager: Arc::new(RoomManager::new()),
    };

    // Static assets (poker.css, poker.js, datastar.js) live next to the
    // crate; STATIC_DIR overrides for the Docker image.
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "static".to_string());

    // CORS: restrict to specific origins in production via CORS_ORIGIN env var
    // Falls back to permissive for local development.
    let cors = std::env::var("CORS_ORIGIN").map_or_else(
        |_| CorsLayer::permissive(),
        |origins| {
            let allowed: Vec<_> = origins
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(allowed))
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any)
        },
    );

    // SSE routes are compressed here, with per-event flushing. This works
    // because async-compression >= 0.4.31 auto-flushes the encoder when the
    // body blocks between chunks (i.e. between SSE events) while keeping the
    // dictionary. This workspace pins 0.4.41, so the flush fires.
    //
    // tower-http's `DefaultPredicate` still refuses `text/event-stream`, so we
    // override it below with the same predicate minus the SSE carve-out.
    // Verified in `tests/sse_compression_latency.rs`: ~8:1 brotli / ~6.8:1
    // gzip on real `state_events` payloads (`examples/measure-payloads.rs`).

    // tower-http's default minus the SSE carve-out: compress everything
    // except gRPC and images, but allow `text/event-stream`.
    let compress_when = SizeAbove::default()
        .and(NotForContentType::GRPC)
        .and(NotForContentType::IMAGES);

    let app = Router::new()
        .route("/poker", get(handlers::shell))
        .route("/poker/api/rooms", get(rooms_handler))
        .nest_service("/poker/static", ServeDir::new(static_dir))
        .route("/poker/events", get(handlers::events))
        .route("/poker/room/create", post(handlers::room_create))
        .route("/poker/room/join", post(handlers::room_join))
        .route("/poker/action/start", post(handlers::action_start))
        .route("/poker/action/fold", post(handlers::action_fold))
        .route("/poker/action/check", post(handlers::action_check))
        .route("/poker/action/call", post(handlers::action_call))
        .route("/poker/action/raise", post(handlers::action_raise))
        .route("/poker/action/allin", post(handlers::action_allin))
        .route("/poker/action/sitin", post(handlers::action_sitin))
        .route(
            "/poker/action/toggle-late-entry",
            post(handlers::action_toggle_late_entry),
        )
        .route(
            "/poker/action/update-settings",
            post(handlers::action_update_settings),
        )
        .layer(
            CompressionLayer::new()
                .br(true)
                .gzip(true)
                .compress_when(compress_when),
        )
        .layer(cors)
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3001);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Poker SSE server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// `GET /poker/api/rooms` — return a JSON array of active room IDs.
async fn rooms_handler(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.room_manager.list_rooms().await)
}
