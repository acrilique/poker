//! Framework-agnostic client controller.
//!
//! Owns a [`NetClient`] and a [`ClientGameState`], providing shared
//! dispatch logic:
//!
//! - Processing incoming [`NetEvent`]s and updating game state.
//! - Forwarding [`ClientMessage`]s to the server.
//!
//! Frontends only need to:
//! 1. Call [`ClientController::connect`] or [`ClientController::connect_ws`]
//!    to establish a connection.
//! 2. Call [`ClientController::poll_net`] (or drain `incoming()`) to process
//!    server events.
//! 3. Call [`ClientController::send`] to transmit player actions.

use crate::game_state::{ClientGameState, GameEvent, LogCategory, StateChanged};
use crate::net_client::{NetClient, NetEvent};
use crate::protocol::{ClientMessage, ServerMessage};
#[cfg(feature = "native")]
use crate::transport::Transport;

/// Outcome of processing a single network event.
#[derive(Debug)]
pub enum PollResult {
    /// A server message was applied; the returned [`StateChanged`] flags
    /// describe what was modified.
    Updated(StateChanged),
    /// An unrecognised line was received (already logged).
    Unknown,
    /// The server closed the connection.
    Disconnected,
    /// An I/O error occurred (already logged).
    Error,
    /// No event was available (channel empty).
    Empty,
}

/// Owns the network client and game state, providing event dispatch logic.
pub struct ClientController {
    net: NetClient,
    pub state: ClientGameState,
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
    #[cfg(any(feature = "native", feature = "web"))]
    pub async fn connect_ws(url: &str, name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let net = NetClient::connect_ws(url).await?;
        let state = ClientGameState::new(name);
        Ok(Self { net, state })
    }

    // ------------------------------------------------------------------
    // Legacy TCP constructor (native only)
    // ------------------------------------------------------------------

    /// Connect to the server over TCP and perform the initial handshake.
    ///
    /// Only available with the `legacy-server` feature (old TCP protocol).
    #[cfg(feature = "legacy-server")]
    pub async fn connect(address: &str, name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let net = NetClient::connect(address, name).await?;
        let state = ClientGameState::new(name);
        Ok(Self { net, state })
    }

    /// Spawn a local TCP server in the background, then connect as a player.
    ///
    /// **Deprecated** — the server is moving to a standalone binary
    /// (`poker-server`). Use [`connect_ws`](Self::connect_ws) instead once
    /// the Axum server is available.
    #[cfg(feature = "legacy-server")]
    #[deprecated(note = "Use `connect_ws` with the standalone poker-server instead")]
    pub async fn host_and_connect(
        address: &str,
        name: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let server = crate::server::PokerServer::new(address);
        let server_address = address.to_string();
        tokio::spawn(async move {
            if let Err(e) = server.run().await {
                eprintln!("Server error: {}", e);
            }
        });
        // Give the listener a moment to bind.
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        Self::connect(&server_address, name).await
    }

    /// Try to receive and process one network event (non-blocking).
    ///
    /// Returns a [`PollResult`] describing what happened. Frontends should
    /// call this in a loop or select until [`PollResult::Empty`] is returned.
    pub fn try_recv(&mut self) -> PollResult {
        match self.net.incoming.try_recv() {
            Ok(event) => self.handle_net_event(event),
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
            Some(event) => self.handle_net_event(event),
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

    /// Borrow the underlying [`ClientGameState`] mutably.
    pub fn game_state_mut(&mut self) -> &mut ClientGameState {
        &mut self.state
    }

    /// Append a local feedback message to the game event log.
    ///
    /// Frontends should call this instead of mutating `ClientGameState`
    /// directly, keeping the controller as the single mutation gateway.
    pub fn add_message(&mut self, text: String, category: LogCategory) {
        self.state.add_message(text, category);
    }

    // -- private -----------------------------------------------------------

    fn handle_net_event(&mut self, event: NetEvent) -> PollResult {
        match event {
            NetEvent::Message(msg) => {
                // Keep the reader task's PRIVATE filter in sync.
                if let ServerMessage::JoinedGame { player_id, .. } = &msg {
                    self.net.set_player_id(*player_id);
                }
                let changed = self.state.apply_server_message(&msg);
                PollResult::Updated(changed)
            }
            NetEvent::Unknown(raw) => {
                self.state.add_event(GameEvent::Unknown { raw });
                PollResult::Unknown
            }
            NetEvent::Error(e) => {
                self.state.connected = false;
                self.state
                    .add_event(GameEvent::ConnectionError { message: e });
                PollResult::Error
            }
            NetEvent::Disconnected => {
                self.state.connected = false;
                self.state.add_event(GameEvent::Disconnected);
                PollResult::Disconnected
            }
        }
    }
}
