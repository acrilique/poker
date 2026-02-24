//! Game screen — main layout shown during an active poker session.
//!
//! Composes the player list sidebar, game table, action bar and event log
//! into the full game view. Mirrors [`super::connection_screen`] so the
//! root app only routes between the two screens.

use dioxus::prelude::*;
use poker_client::game_state::ClientGameState;

use super::{action_bar, event_log, game_log_overlay, game_table, player_list};

#[component]
pub fn GameScreen(state: Signal<ClientGameState>) -> Element {
    rsx! {
        div { class: "flex h-screen portrait-rotate",
            // Left sidebar: player list
            div { class: "w-64 bg-surface border-r border-muted/50 flex flex-col",
                player_list::PlayerList { state }
            }
            // Main area
            div { class: "flex-1 flex flex-col relative",
                // Game table (top part, takes available space)
                div { class: "flex-1 flex flex-col",
                    game_table::GameTable { state }
                }
                // Action bar
                action_bar::ActionBar { state }
                // Event log: always visible on large screens, hidden overlay on small
                div { class: "hidden lg:block h-48 border-t border-muted/50",
                    event_log::EventLog { state }
                }
                // Mobile log overlay (managed by the GameLogOverlay component)
                div { class: "lg:hidden",
                    game_log_overlay::GameLogOverlay { state }
                }
            }
        }
    }
}
