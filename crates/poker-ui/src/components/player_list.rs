//! Player list sidebar.

use dioxus::prelude::*;
use poker_core::game_state::ClientGameState;
use poker_core::protocol::ClientMessage;

use crate::UiMessage;

#[component]
pub fn PlayerList(state: Signal<ClientGameState>) -> Element {
    let gs = state.read();
    let coroutine = use_coroutine_handle::<UiMessage>();

    rsx! {
        div { class: "flex flex-col h-full",
            // Header
            div { class: "p-4 border-b border-gray-700",
                h2 { class: "text-lg font-bold text-emerald-400", "Players" }
            }

            // Player entries
            div { class: "flex-1 overflow-y-auto p-2",
                for player in gs.players.iter() {
                    {
                        let is_us = player.id == gs.our_player_id;
                        let is_dealer = player.id == gs.dealer_id;
                        let bg = if is_us { "bg-gray-700" } else { "bg-gray-800" };

                        rsx! {
                            div { class: "flex items-center justify-between px-3 py-2 rounded-lg mb-1 {bg}",
                                div { class: "flex items-center gap-2",
                                    if is_dealer {
                                        span { class: "text-yellow-400 text-xs font-bold", "D" }
                                    }
                                    span { class: if is_us { "font-semibold text-emerald-300" } else { "text-white" }, "{player.name}" }
                                }
                                span { class: "text-gray-400 text-sm", "{player.chips}" }
                            }
                        }
                    }
                }
            }

            // Start game button (visible in lobby when game hasn't started)
            if !gs.game_started {
                div { class: "p-3 border-t border-gray-700",
                    button {
                        class: "w-full bg-emerald-600 hover:bg-emerald-500 text-white font-semibold rounded-lg py-2 transition",
                        onclick: move |_| {
                            coroutine.send(UiMessage::Action(ClientMessage::StartGame));
                        },
                        "Start Game"
                    }
                }
            }
        }
    }
}
