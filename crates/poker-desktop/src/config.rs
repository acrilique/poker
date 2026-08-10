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

//! CLI parsing and the default public server the client connects to.

use clap::Parser;

/// The public server desktop users join by default so they play in the same
/// games as browser users. Override at runtime with `--server`.
#[must_use]
pub fn default_server_url() -> String {
    "https://acrilique.com/poker".to_string()
}

/// A webview poker client. By default it connects to the public server; pass
/// `--host` to start a local server instead so friends can join you over the
/// LAN (or the internet with a forwarded router port).
#[derive(Parser, Debug)]
#[command(name = "poker-desktop", version, long_about = None)]
pub struct Cli {
    /// Server URL to connect to (client mode). Ignored when `--host` is set.
    #[arg(long, default_value_t = default_server_url())]
    server: String,

    /// Start a local poker server and join it, so friends can connect to you.
    #[arg(long, default_value_t = false)]
    host: bool,

    /// Port for the local server when `--host` is set. Defaults to the SSE
    /// server's default port.
    #[arg(long, default_value_t = poker_sse_server::ServerConfig::default().port)]
    port: u16,
}

impl Cli {
    #[must_use]
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }

    /// Whether to boot in host mode (embedded server).
    #[must_use]
    pub const fn host(&self) -> bool {
        self.host
    }

    /// Remote server URL for client mode.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Port for the embedded server in host mode.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }
}
