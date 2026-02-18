//! Player list sidebar.

use dioxus::prelude::*;
use poker_core::game_state::ClientGameState;
use poker_core::protocol::ClientMessage;

use crate::{format_stack, StackDisplayMode, UiMessage};

#[component]
pub fn PlayerList(state: Signal<ClientGameState>) -> Element {
    let gs = state.read();
    let coroutine = use_coroutine_handle::<UiMessage>();
    let mut display_mode: Signal<StackDisplayMode> = use_context();
    let mode = *display_mode.read();
    let bb = gs.big_blind;

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
                        let is_sb = player.id == gs.small_blind_id;
                        let is_bb = player.id == gs.big_blind_id;
                        let bg = if is_us { "bg-gray-700" } else { "bg-gray-800" };
                        let stack_text = format_stack(player.chips, bb, mode);

                        rsx! {
                            div { class: "flex items-center justify-between px-3 py-2 rounded-lg mb-1 {bg}",
                                div { class: "flex items-center gap-2",
                                    if is_sb {
                                        span { class: "text-yellow-400 text-xs font-bold", "SB" }
                                    }
                                    if is_bb {
                                        span { class: "text-blue-400 text-xs font-bold", "BB" }
                                    }
                                    span { class: if is_us { "font-semibold text-emerald-300" } else { "text-white" }, "{player.name}" }
                                }
                                span {
                                    class: "text-gray-400 text-sm cursor-pointer hover:text-gray-200 select-none",
                                    title: "Click to toggle chips / BB",
                                    onclick: move |_| {
                                        let new_mode = display_mode.read().toggle();
                                        display_mode.set(new_mode);
                                    },
                                    "{stack_text}"
                                }
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
