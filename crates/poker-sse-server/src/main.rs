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

//! Standalone poker SSE server binary.
//!
//! Thin entry point: parses environment variables via
//! [`ServerConfig::from_env`] and delegates to [`poker_sse_server::run`]. All
//! real work lives in the library so the desktop client can reuse it for
//! "host a game" mode.

use poker_sse_server::{ServerConfig, run};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialise tracing (respects RUST_LOG env var).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    run(ServerConfig::from_env()).await
}
