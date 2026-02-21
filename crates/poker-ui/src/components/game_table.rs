//! Game table — community cards, hole cards, pot.

use dioxus::prelude::*;
use poker_core::game_state::ClientGameState;

use super::card;
use crate::{StackDisplayMode, format_stack};

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
        div { class: "flex flex-col items-center h-full gap-2 p-2 lg:justify-center lg:gap-6 lg:p-4",
            // Room ID + Stage / hand info
            div { class: "flex items-center justify-between w-full px-1 mb-1 text-foreground/60 text-xs tracking-wide uppercase lg:mb-0 lg:justify-center lg:gap-4 lg:text-sm lg:w-auto lg:px-0",
                div { class: "bg-surface border border-muted/50 rounded px-2 py-0.5 select-all cursor-pointer lg:px-3 lg:py-1",
                    title: "Room ID — click to select",
                    "Room: {gs.room_id}"
                }
                div { "Hand #{gs.hand_number}  ·  {gs.stage}" }
            }

            // Community cards
            div { class: "flex gap-2 lg:gap-3",
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
                class: "bg-surface rounded-full px-4 py-1 text-sm font-semibold text-accent shadow cursor-pointer hover:brightness-125 select-none lg:px-6 lg:py-2 lg:text-lg",
                title: "Click to toggle chips / BB",
                onclick: move |_| {
                    let new_mode = display_mode.read().toggle();
                    display_mode.set(new_mode);
                },
                "Pot: {pot_text}"
            }

            // Hole cards + hand rank
            div { class: "flex flex-col items-center gap-1",
                div { class: "flex gap-2 lg:gap-3",
                    if let Some(cards) = hole {
                        card::Card { card: cards[0] }
                        card::Card { card: cards[1] }
                    } else {
                        card::CardBack {}
                        card::CardBack {}
                    }
                }
                if let Some(rank) = gs.hand_rank() {
                    div { class: "text-sm font-medium text-accent tracking-wide",
                        "{rank}"
                    }
                }
            }
        }
    }
}
