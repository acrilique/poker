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

//! Library facade so benches, examples, tests, and the desktop client can
//! drive the server.
//!
//! [`run`] boots a standalone server (used by the `poker-sse-server` binary
//! and by `poker-desktop`'s "host a game" mode). [`build_router`] exposes the
//! full route table for embedding.

// The lib target exists for reuse. Promoting the modules to a public API
// trips library-publishing lints that add no value for an internal server
// crate. Allowed here, at the lib root only — `main.rs` still enforces the
// full strict lint set from `Cargo.toml`.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::too_long_first_doc_paragraph
)]

pub mod fanout;
pub mod flow;
pub mod handlers;
pub mod render;
pub mod room;
pub mod sse;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use room::RoomManager;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{NotForContentType, Predicate, SizeAbove};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;

/// Shared application state available to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub room_manager: Arc<RoomManager>,
}

/// CORS policy for the server. Callers decide explicitly instead of reading
/// the environment here.
#[derive(Clone, Debug)]
pub enum CorsConfig {
    /// Permissive — fine for local/LAN play (the desktop client's host mode).
    Permissive,
    /// Restrict to a fixed list of origins (production deployment).
    Allow(Vec<String>),
}

/// Configuration for [`run`] / [`build_router`].
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// TCP port to listen on.
    pub port: u16,
    /// Directory containing `poker.css`, `poker.js`, `datastar.js`.
    pub static_dir: String,
    /// CORS policy.
    pub cors: CorsConfig,
}

impl ServerConfig {
    /// Sensible LAN-friendly defaults: permissive CORS, port 3001, a
    /// `static/` directory resolved relative to the process CWD. Override
    /// fields as needed.
    #[must_use]
    pub fn new(port: u16) -> Self {
        Self {
            port,
            static_dir: "static".to_string(),
            cors: CorsConfig::Permissive,
        }
    }
}

/// Build the full Axum router (routes + compression + CORS + state).
///
/// Exposed so embedders (e.g. the desktop client) can mount it under a
/// sub-path or compose it with other services if they ever need to.
pub fn build_router(config: &ServerConfig) -> Router {
    let state = AppState {
        room_manager: Arc::new(RoomManager::new()),
    };

    let cors = match &config.cors {
        CorsConfig::Permissive => CorsLayer::permissive(),
        CorsConfig::Allow(origins) => {
            let allowed: Vec<_> = origins
                .iter()
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(allowed))
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any)
        }
    };

    // SSE routes are compressed here, with per-event flushing. This works
    // because async-compression >= 0.4.31 auto-flushes the encoder when the
    // body blocks between chunks (i.e. between SSE events) while keeping the
    // dictionary. This workspace pins 0.4.41, so the flush fires.
    //
    // tower-http's `DefaultPredicate` still refuses `text/event-stream`, so we
    // override it below with the same predicate minus the SSE carve-out.
    // Verified in `tests/sse_compression_latency.rs`: ~8:1 brotli / ~6.8:1
    // gzip on real `state_events` payloads (`examples/measure_payloads.rs`).

    // tower-http's default minus the SSE carve-out: compress everything
    // except gRPC and images, but allow `text/event-stream`.
    let compress_when = SizeAbove::default()
        .and(NotForContentType::GRPC)
        .and(NotForContentType::IMAGES);

    Router::new()
        .route("/poker", get(handlers::shell))
        .route("/poker/", get(handlers::shell))
        .route("/poker/manifest.json", get(handlers::manifest))
        .route("/poker/sw.js", get(handlers::service_worker))
        .nest_service("/poker/static", ServeDir::new(&config.static_dir))
        .route("/poker/events", get(sse::events))
        .route("/poker/room/create", post(handlers::room_create))
        .route("/poker/room/join", post(handlers::room_join))
        .route("/poker/room/leave", post(handlers::room_leave))
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
        .with_state(state)
}

/// Boot the server on `0.0.0.0:{config.port}` and serve until shutdown.
///
/// Used by the `poker-sse-server` binary and by `poker-desktop`'s host mode.
pub async fn run(config: ServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    let app = build_router(&config);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("Poker SSE server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
