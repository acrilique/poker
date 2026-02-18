//! Game table — community cards, hole cards, pot.

use dioxus::prelude::*;
use poker_core::game_state::ClientGameState;

use super::card;

#[component]
pub fn GameTable(state: Signal<ClientGameState>) -> Element {
    let gs = state.read();

    let community = &gs.community_cards;
    let hole = gs.hole_cards;

    rsx! {
        div { class: "flex flex-col items-center justify-center h-full gap-6 p-4",
            // Stage / hand info
            div { class: "text-gray-400 text-sm tracking-wide uppercase",
                "Hand #{gs.hand_number}  ·  {gs.stage}"
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
            div { class: "bg-gray-800 rounded-full px-6 py-2 text-lg font-semibold text-yellow-400 shadow",
                "Pot: {gs.pot}"
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
