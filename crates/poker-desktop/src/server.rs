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

//! Host mode: run [`poker_sse_server`] in-process so friends on the LAN (or
//! the internet, with a forwarded router port) can join a browser tab pointed
//! at this machine.

use std::path::Path;

use local_ip_address::local_ip;
use poker_sse_server::{CorsConfig, ServerConfig, run};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Resolve the directory holding `poker-sse-server`'s `static/` assets.
///
/// When run via `cargo run` / a debug build, the process CWD is the workspace
/// root and the SSE server's default `"static"` lookup fails, so we fall back
/// to the sibling crate's committed `static/` directory. `STATIC_DIR` still
/// wins, matching the standalone server.
fn resolve_static_dir() -> String {
    if let Ok(dir) = std::env::var("STATIC_DIR") {
        return dir;
    }
    // `<crate root of poker-desktop>/../poker-sse-server/static` — correct
    // under `cargo run -p poker-desktop` (CWD = workspace root) and for an
    // installed binary whose CWD is arbitrary, as long as the static dir is
    // shipped alongside. See the README "packaging" note.
    let sibling = env!("CARGO_MANIFEST_DIR");
    let candidate = format!("{sibling}/../poker-sse-server/static");
    if Path::new(&candidate).is_dir() {
        return candidate;
    }
    // Last resort: let the SSE server's own fallback handle it.
    "static".to_string()
}

/// A locally-hosted game. Dropping the [`Handle`] cancels the server task.
pub struct Handle {
    cancel: CancellationToken,
    #[allow(dead_code)]
    join: JoinHandle<Result<(), String>>,
}

impl Handle {
    /// Stop the embedded server. Idempotent.
    pub fn stop(self) {
        self.cancel.cancel();
        // Detach the task; cancellation is signalled, we don't block the UI
        // thread on its teardown.
    }
}

/// Start the embedded poker server bound to `0.0.0.0:port`, driving it on the
/// given tokio [`Handle`].
///
/// LAN play works out of the box; for internet play the user forwards the
/// same port on their router. Permissive CORS is correct here — anyone who
/// can reach the port should be allowed to play.
pub fn start(port: u16, runtime: &tokio::runtime::Handle) -> Handle {
    let static_dir = resolve_static_dir();
    let config = ServerConfig {
        port,
        static_dir,
        cors: CorsConfig::Permissive,
    };

    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();

    // Spawn on the explicit handle rather than `tokio::spawn`: the main thread
    // runs the tao event loop and is never "inside" the tokio runtime, so there
    // is no ambient reactor to pick up.
    let join = runtime.spawn(async move {
        // Run until cancelled. `axum::serve` doesn't take a stop token, so we
        // race it against cancellation; on cancel we just let the task end.
        // The error is flattened to a String so the JoinHandle output is
        // Send + Sync without `Box<dyn Error>` gymnastics.
        let serve = run(config);
        tokio::select! {
            biased;
            res = serve => res.map_err(|e| e.to_string()),
            () = cancel_for_task.cancelled() => Ok(()),
        }
    });

    Handle { cancel, join }
}

/// Build the shareable URL friends use to reach the hosted game, e.g.
/// `http://192.168.1.42:3001/poker`. Falls back to `127.0.0.1` if no LAN IP
/// can be determined.
#[must_use]
pub fn lan_invite_url(port: u16) -> String {
    let ip = local_ip().map_or_else(|_| "127.0.0.1".to_string(), |ip| ip.to_string());
    format!("http://{ip}:{port}/poker")
}
