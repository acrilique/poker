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

//! Session persistence and reconnection helpers.
//!
//! These live in the client crate because they operate purely on
//! [`ClientController`] and the poker protocol — no UI framework
//! dependency.

#[cfg(any(feature = "native", all(feature = "web", target_arch = "wasm32")))]
use crate::client_controller::{ClientController, PollResult};
#[cfg(any(feature = "native", all(feature = "web", target_arch = "wasm32")))]
use crate::game_state::GameEvent;
#[cfg(any(feature = "native", all(feature = "web", target_arch = "wasm32")))]
use poker_core::protocol::ClientMessage;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Maximum number of automatic reconnection attempts before giving up.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Base delay between reconnection attempts in ms (doubles each attempt).
pub const RECONNECT_BASE_DELAY_MS: u64 = 1_000;

/// Maximum time (in seconds) to wait for a rejoin confirmation before
/// giving up and returning `None`.
pub const REJOIN_TIMEOUT_SECS: u64 = 10;

// ---------------------------------------------------------------------------
// Session persistence trait
// ---------------------------------------------------------------------------

/// Abstraction over session storage so reconnection logic stays
/// platform-agnostic.
///
/// Implementations live in the platform crate (e.g. `sessionStorage` on web,
/// a file on disk for native).
pub trait SessionStore {
    /// Persist the current session so it survives a page reload / restart.
    fn save(&self, ws_url: &str, room_id: &str, name: &str, session_token: &str);
    /// Load a previously saved session, if any.
    fn load(&self) -> Option<(String, String, String, String)>;
    /// Clear the saved session.
    fn clear(&self);
}

// ---------------------------------------------------------------------------
// Reconnection helper
// ---------------------------------------------------------------------------

/// Attempt to rejoin a room using a saved session token.
///
/// Opens a fresh WebSocket connection, sends `Rejoin`, and waits for the
/// server to confirm. Returns a fully-connected [`ClientController`] on
/// success, or `None` if the session is invalid / expired.
#[cfg(any(feature = "native", all(feature = "web", target_arch = "wasm32")))]
pub async fn try_rejoin(
    ws_url: &str,
    room_id: &str,
    name: &str,
    session_token: &str,
) -> Option<ClientController> {
    use futures_util::future::{Either, select};
    use std::pin::pin;

    let mut ctrl = ClientController::connect_ws(ws_url, name).await.ok()?;
    ctrl.send(ClientMessage::Rejoin {
        room_id: room_id.to_string(),
        session_token: session_token.to_string(),
    });

    // Wait for Rejoined or an error, but give up after REJOIN_TIMEOUT_SECS
    // so we never hang the frontend indefinitely.
    let recv_loop = async move {
        loop {
            match ctrl.recv().await {
                PollResult::Updated(changed) => {
                    if (changed.players || changed.phase)
                        && ctrl.state.our_player_id != 0
                        && !ctrl.state.room_id.is_empty()
                    {
                        return Some(ctrl);
                    }
                    // Check if the latest event is an error (session expired).
                    if let Some(ev) = ctrl.state.events.back()
                        && matches!(ev, GameEvent::ServerError { .. })
                    {
                        return None;
                    }
                }
                PollResult::Disconnected => return None,
                _ => {}
            }
        }
    };

    #[cfg(feature = "native")]
    let timeout = pin!(tokio::time::sleep(std::time::Duration::from_secs(
        REJOIN_TIMEOUT_SECS,
    )));
    #[cfg(all(feature = "web", not(feature = "native"), target_arch = "wasm32"))]
    let timeout = pin!(gloo_timers::future::TimeoutFuture::new(
        (REJOIN_TIMEOUT_SECS * 1000) as u32,
    ));

    match select(pin!(recv_loop), timeout).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => None, // timed out
    }
}
