//! Mobile log overlay — toggle button + full-screen overlay.

use dioxus::prelude::*;
use poker_client::game_state::ClientGameState;

use super::event_log;

/// A small component that shows a "Logs" toggle button at the bottom-right
/// of the game area. When tapped, an overlay with the event log covers the
/// board + action bar.
#[component]
pub fn GameLogOverlay(state: Signal<ClientGameState>) -> Element {
    let mut show_log = use_signal(|| false);
    let visible = *show_log.read();

    rsx! {
        if visible {
            // Overlay covering the main area
            div {
                class: "absolute inset-0 z-40 bg-base/95 overflow-y-auto",
                event_log::EventLog { state }
            }
        }
        // Toggle button always pinned to the bottom-right of the main area
        // Rendered after the overlay so it paints on top
        div { class: "absolute bottom-14 right-3 z-50",
            button {
                class: "px-3 py-1.5 bg-surface/80 hover:bg-muted rounded-lg text-xs font-semibold text-foreground/70 shadow-lg transition backdrop-blur-sm",
                onclick: move |_| show_log.set(!visible),
                if visible { "✕ Close" } else { "Logs" }
            }
        }
    }
}
