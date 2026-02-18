//! Shared Dioxus UI components for the poker app.
//!
//! This crate is platform-agnostic — it provides reusable components,
//! the shared `UiMessage` type, and the `Screen` enum used by both the
//! desktop (`poker-gui`) and web (`poker-web`) frontends.

pub mod components;

use poker_core::protocol::ClientMessage;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// Which screen the app is showing.
#[derive(Clone, Debug, PartialEq)]
pub enum Screen {
    Connection,
    Game,
}

/// Messages sent from UI components to the background coroutine.
#[derive(Debug)]
pub enum UiMessage {
    /// Connect to a server room.
    Connect {
        name: String,
        server_url: String,
        room_id: String,
        create: bool,
    },
    /// A game action to forward to the server.
    Action(ClientMessage),
}
