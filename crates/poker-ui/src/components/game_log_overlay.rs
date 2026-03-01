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
