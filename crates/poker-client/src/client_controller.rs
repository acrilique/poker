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

//! Framework-agnostic client controller.
//!
//! Owns a [`NetClient`] and a [`ClientGameState`], providing shared
//! dispatch logic:
//!
//! - Processing incoming [`ServerMessage`]s and updating game state.
//! - Forwarding [`ClientMessage`]s to the server.
//!
//! Frontends only need to:
//! 1. Call [`ClientController::connect_ws`] to establish a connection.
//! 2. Call [`ClientController::try_recv`] or [`ClientController::recv`]
//!    to process server messages.
//! 3. Call [`ClientController::send`] to transmit player actions.

use crate::game_state::{ClientGameState, GameEvent, LogCategory, StateChanged};
use crate::net_client::NetClient;
#[cfg(feature = "native")]
use crate::transport::Transport;
#[cfg(any(feature = "native", all(feature = "web", target_arch = "wasm32")))]
use crate::transport::TransportError;
use poker_core::protocol::{ClientMessage, ServerMessage};

/// Outcome of processing a single network event.
#[derive(Debug)]
pub enum PollResult {
    /// A server message was applied; the returned [`StateChanged`] flags
    /// describe what was modified.
    Updated(StateChanged),
    /// The server closed the connection.
    Disconnected,
    /// No event was available (channel empty).
    Empty,
}

/// Owns the network client and game state, providing event dispatch logic.
///
/// Frontends should treat the game state as **read-only** and use the
/// controller methods to mutate it:
///
/// - [`game_state()`](Self::game_state) — immutable view of the current state.
/// - [`snapshot()`](Self::snapshot) — cheap `Clone` for UI signals / snapshots.
/// - [`send()`](Self::send) — forward a player action to the server.
/// - [`add_message()`](Self::add_message) — append a local UI message.
///
/// The `state` field is crate-private so external code cannot accidentally
/// mutate it.
pub struct ClientController {
    net: NetClient,
    pub(crate) state: ClientGameState,
}

impl ClientController {
    // ------------------------------------------------------------------
    // Generic transport constructor (native only — uses tokio::spawn)
    // ------------------------------------------------------------------

    /// Create a controller over any [`Transport`] implementation.
    ///
    /// No handshake messages are sent automatically — the caller should send
    /// `JoinRoom` (or `CreateRoom` + `JoinRoom`) after construction.
    #[cfg(feature = "native")]
    pub fn from_transport<T: Transport>(transport: T, name: &str) -> Self {
        let net = NetClient::from_transport(transport);
        let state = ClientGameState::new(name);
        Self { net, state }
    }

    // ------------------------------------------------------------------
    // WebSocket convenience constructor
    // ------------------------------------------------------------------

    /// Connect to a WebSocket server (e.g. `ws://host/ws/room-id`).
    ///
    /// No join handshake is sent — the caller should send `JoinRoom` after
    /// construction.
    #[cfg(any(feature = "native", all(feature = "web", target_arch = "wasm32")))]
    pub async fn connect_ws(url: &str, name: &str) -> Result<Self, TransportError> {
        let net = NetClient::connect_ws(url).await?;
        let state = ClientGameState::new(name);
        Ok(Self { net, state })
    }

    /// Try to receive and process one network event (non-blocking).
    ///
    /// Returns a [`PollResult`] describing what happened. Frontends should
    /// call this in a loop or select until [`PollResult::Empty`] is returned.
    pub fn try_recv(&mut self) -> PollResult {
        match self.net.incoming.try_recv() {
            Ok(msg) => self.handle_server_message(msg),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => PollResult::Empty,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                self.state.connected = false;
                self.state.add_event(GameEvent::Disconnected);
                PollResult::Disconnected
            }
        }
    }

    /// Await the next network event (blocking/async).
    ///
    /// This is useful in `tokio::select!` loops.
    pub async fn recv(&mut self) -> PollResult {
        match self.net.incoming.recv().await {
            Some(msg) => self.handle_server_message(msg),
            None => {
                self.state.connected = false;
                self.state.add_event(GameEvent::Disconnected);
                PollResult::Disconnected
            }
        }
    }

    /// Send a [`ClientMessage`] to the server.
    pub fn send(&self, msg: ClientMessage) {
        let _ = self.net.send(msg);
    }

    /// Borrow the underlying [`ClientGameState`] immutably.
    pub fn game_state(&self) -> &ClientGameState {
        &self.state
    }

    /// Clone the current game state (cheap snapshot for UI signals).
    pub fn snapshot(&self) -> ClientGameState {
        self.state.clone()
    }

    /// Append a local feedback message to the game event log.
    ///
    /// Frontends should call this instead of mutating `ClientGameState`
    /// directly, keeping the controller as the single mutation gateway.
    pub fn add_message(&mut self, text: impl Into<String>, category: LogCategory) {
        self.state.add_message(text, category);
    }

    // -- private -----------------------------------------------------------

    fn handle_server_message(&mut self, msg: ServerMessage) -> PollResult {
        let changed = self.state.apply_server_message(&msg);
        PollResult::Updated(changed)
    }
}
