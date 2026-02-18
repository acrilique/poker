//! Shared Dioxus UI components for the poker app.
//!
//! This crate is platform-agnostic — it provides reusable components,
//! the shared `UiMessage` type, and the `Screen` enum used by both the
//! desktop (`poker-gui`) and web (`poker-web`) frontends.

pub mod components;

use poker_core::protocol::{BlindConfig, ClientMessage};

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// Which screen the app is showing.
#[derive(Clone, Debug, PartialEq)]
pub enum Screen {
    Connection,
    Game,
}

/// Whether stacks / pots are displayed in chip units or big-blind units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackDisplayMode {
    /// Show values as big-blind multiples (e.g. "12.5 BB").
    Blinds,
    /// Show raw chip counts (e.g. "250").
    Chips,
}

impl StackDisplayMode {
    /// Toggle between the two modes.
    pub fn toggle(self) -> Self {
        match self {
            Self::Blinds => Self::Chips,
            Self::Chips => Self::Blinds,
        }
    }
}

/// Format a chip amount according to the chosen display mode.
///
/// When `big_blind` is 0 (game hasn't started yet) we always fall back to
/// raw chip display to avoid division by zero.
pub fn format_stack(chips: u32, big_blind: u32, mode: StackDisplayMode) -> String {
    if big_blind == 0 || mode == StackDisplayMode::Chips {
        return format!("{chips}");
    }
    let bb = chips as f64 / big_blind as f64;
    if bb.fract() == 0.0 {
        format!("{} BB", bb as u64)
    } else {
        format!("{bb:.1} BB")
    }
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
        blind_config: BlindConfig,
    },
    /// A game action to forward to the server.
    Action(ClientMessage),
}
