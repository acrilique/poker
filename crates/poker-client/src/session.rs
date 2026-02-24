//! Session persistence and reconnection helpers.
//!
//! These live in the client crate because they operate purely on
//! [`ClientController`] and the poker protocol — no UI framework
//! dependency.

use crate::client_controller::{ClientController, PollResult};
use crate::game_state::GameEvent;
use poker_core::protocol::ClientMessage;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Maximum number of automatic reconnection attempts before giving up.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Base delay between reconnection attempts in ms (doubles each attempt).
pub const RECONNECT_BASE_DELAY_MS: u64 = 1_000;

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
#[cfg(any(feature = "native", feature = "web"))]
pub async fn try_rejoin(
    ws_url: &str,
    room_id: &str,
    name: &str,
    session_token: &str,
) -> Option<ClientController> {
    let mut ctrl = ClientController::connect_ws(ws_url, name).await.ok()?;
    ctrl.send(ClientMessage::Rejoin {
        room_id: room_id.to_string(),
        session_token: session_token.to_string(),
    });

    // Wait for Rejoined or an error.
    loop {
        match ctrl.recv().await {
            PollResult::Updated(changed) => {
                if changed.players
                    || changed.phase
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
}
