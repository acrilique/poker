//! Game table — community cards, hole cards, pot.

use dioxus::prelude::*;
use poker_core::game_state::ClientGameState;

use super::card;
use crate::{format_stack, StackDisplayMode};

#[component]
pub fn GameTable(state: Signal<ClientGameState>) -> Element {
    let gs = state.read();
    let mut display_mode: Signal<StackDisplayMode> = use_context();
    let mode = *display_mode.read();
    let bb = gs.big_blind;

    let community = &gs.community_cards;
    let hole = gs.hole_cards;
    let pot_text = format_stack(gs.pot, bb, mode);

    rsx! {
        div { class: "flex flex-col items-center justify-center h-full gap-6 p-4",
            // Room ID + Stage / hand info
            div { class: "flex items-center gap-4 text-gray-400 text-sm tracking-wide uppercase",
                div { class: "bg-gray-800 border border-gray-600 rounded px-3 py-1 select-all cursor-pointer",
                    title: "Room ID — click to select",
                    "Room: {gs.room_id}"
                }
                div { "Hand #{gs.hand_number}  ·  {gs.stage}" }
            }

            // Community cards
            div { class: "flex gap-2",
                for i in 0..5 {
                    if let Some(c) = community.get(i) {
                        card::Card { card: *c }
                    } else {
                        card::EmptyCard {}
                    }
                }
            }

            // Pot
            div {
                class: "bg-gray-800 rounded-full px-6 py-2 text-lg font-semibold text-yellow-400 shadow cursor-pointer hover:brightness-125 select-none",
                title: "Click to toggle chips / BB",
                onclick: move |_| {
                    let new_mode = display_mode.read().toggle();
                    display_mode.set(new_mode);
                },
                "Pot: {pot_text}"
            }

            // Hole cards
            div { class: "flex gap-2 mt-2",
                if let Some(cards) = hole {
                    card::Card { card: cards[0] }
                    card::Card { card: cards[1] }
                } else {
                    card::CardBack {}
                    card::CardBack {}
                }
            }
        }
    }
}
