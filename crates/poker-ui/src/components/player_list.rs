//! Player list sidebar.

use dioxus::prelude::*;
use poker_core::game_state::ClientGameState;
use poker_core::protocol::ClientMessage;

use crate::{StackDisplayMode, UiMessage, format_stack};

#[component]
pub fn PlayerList(state: Signal<ClientGameState>) -> Element {
    let gs = state.read();
    let coroutine = use_coroutine_handle::<UiMessage>();
    let mut display_mode: Signal<StackDisplayMode> = use_context();
    let mode = *display_mode.read();
    let bb = gs.big_blind;

    rsx! {
        div { class: "flex flex-col h-full",
            // Player entries
            div { class: "flex-1 overflow-y-auto p-2",
                for player in gs.players.iter() {
                    {
                        let is_us = player.id == gs.our_player_id;
                        let is_sb = player.id == gs.small_blind_id;
                        let is_bb = player.id == gs.big_blind_id;
                        let is_sat_out = gs.is_player_sitting_out(player.id);
                        let is_folded = gs.is_player_folded(player.id);
                        let is_active_turn = gs.turn_timer_player == Some(player.id);
                        let bg = if is_us { "bg-muted" } else { "bg-surface" };
                        let border = if is_active_turn { "ring-2 ring-accent" } else { "" };
                        let opacity = if is_folded { "opacity-50" } else { "" };

                        // Compute effective stack (chips minus current bet) and bet amount.
                        let bet = gs.player_bets.get(&player.id).copied().unwrap_or(0).min(player.chips);
                        let effective_chips = player.chips.saturating_sub(bet);
                        let stack_text = format_stack(effective_chips, bb, mode);
                        let bet_text = if bet > 0 { Some(format_stack(bet, bb, mode)) } else { None };

                        rsx! {
                            div { class: "flex items-center justify-between px-3 py-2 rounded-lg mb-1 {bg} {border} {opacity}",
                                div { class: "flex items-center gap-2",
                                    if is_sb {
                                        span { class: "text-accent text-xs font-bold", "SB" }
                                    }
                                    if is_bb {
                                        span { class: "text-primary text-xs font-bold", "BB" }
                                    }
                                    span { class: if is_us { "font-semibold text-accent" } else { "text-foreground" }, "{player.name}" }
                                    if is_sat_out {
                                        span { class: "text-foreground/40 text-xs italic", "(away)" }
                                    }
                                    if is_folded {
                                        span { class: "text-foreground/40 text-xs italic", "(folded)" }
                                    }
                                }
                                div {
                                    class: "flex items-center gap-1.5 cursor-pointer select-none",
                                    title: "Click to toggle chips / BB",
                                    onclick: move |_| {
                                        let new_mode = display_mode.read().toggle();
                                        display_mode.set(new_mode);
                                    },
                                    span { class: "text-foreground/60 text-sm hover:text-foreground", "{stack_text}" }
                                    if let Some(bt) = &bet_text {
                                        span { class: "text-accent text-sm font-medium", "+{bt}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Bottom controls: Start / Sit Out / Exit
            div { class: "p-3 border-t border-muted/50 flex flex-col gap-2",
                // Start game button (lobby only)
                if !gs.game_started {
                    button {
                        class: "w-full bg-primary hover:bg-primary-light text-foreground font-semibold rounded-lg py-2 transition",
                        onclick: move |_| {
                            coroutine.send(UiMessage::Action(ClientMessage::StartGame));
                        },
                        "Start Game"
                    }
                }

                // Sit Out / Sit In toggle (visible once game has started)
                if gs.game_started {
                    {
                        let is_sitting_out = gs.is_sitting_out();
                        let (label, btn_class) = if is_sitting_out {
                            ("Sit In", "w-full bg-primary hover:bg-primary-light rounded-lg py-1.5 text-sm font-semibold text-foreground transition")
                        } else {
                            ("Sit Out", "w-full bg-elevated hover:bg-base rounded-lg py-1.5 text-sm font-semibold text-foreground transition")
                        };
                        rsx! {
                            button {
                                class: "{btn_class}",
                                onclick: move |_| {
                                    if is_sitting_out {
                                        coroutine.send(UiMessage::Action(ClientMessage::SitIn));
                                    } else {
                                        coroutine.send(UiMessage::Action(ClientMessage::SitOut));
                                    }
                                },
                                "{label}"
                            }
                        }
                    }
                }

                // Exit game button (always visible)
                button {
                    class: "w-full bg-muted hover:bg-muted-light rounded-lg py-1.5 text-sm font-semibold text-foreground transition",
                    onclick: move |_| {
                        coroutine.send(UiMessage::ExitGame);
                    },
                    "Exit Game"
                }
            }
        }
    }
}
