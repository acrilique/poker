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

//! Action bar — fold/check, call, raise with presets.

use dioxus::prelude::*;
use poker_client::game_state::{ClientGameState, RAISE_PRESETS, RaisePreset};
use poker_core::protocol::PlayerAction;

use crate::{StackDisplayMode, UiMessage, format_stack};

#[component]
pub fn ActionBar(state: Signal<ClientGameState>) -> Element {
    let gs = state.read();
    let coroutine = use_coroutine_handle::<UiMessage>();
    let mut raise_input = use_signal(String::new);
    let display_mode: Signal<StackDisplayMode> = use_context();
    let mode = *display_mode.read();
    let bb = gs.big_blind;
    let is_sitting_out = gs.is_sitting_out();

    // Only show when it's our turn.
    if !gs.is_our_turn {
        return rsx! {
            div { class: "h-16 bg-surface border-t border-muted/50 flex items-center justify-center gap-4 text-foreground/50 text-sm",
                if is_sitting_out {
                    "Sitting out…"
                } else {
                    "Waiting for your turn…"
                }
            }
        };
    }

    let can_fold = gs.has_action(PlayerAction::Fold);
    let can_check = gs.has_action(PlayerAction::Check);
    let can_call = gs.has_action(PlayerAction::Call);
    let can_raise = gs.has_action(PlayerAction::Raise);
    let can_allin = gs.has_action(PlayerAction::AllIn);

    let fold_check_label = if can_check { "Check" } else { "Fold" };
    let call_amount = gs.current_bet.saturating_sub(gs.our_bet);
    let call_text = format_stack(call_amount, bb, mode);

    rsx! {
        div { class: "bg-surface border-t border-muted/50 p-3 flex flex-col gap-2",
            // Raise presets (top row)
            if can_raise || can_allin {
                div { class: "flex items-center gap-2 justify-center",
                    for preset in RAISE_PRESETS.iter() {
                        {
                            let amount = preset.amount(&gs);
                            let label = preset.label();
                            let is_allin = *preset == RaisePreset::AllIn;

                            let btn_class = if is_allin {
                                "px-3 py-1 bg-muted hover:bg-muted-light rounded-lg text-sm font-semibold text-foreground transition"
                            } else {
                                "px-3 py-1 bg-elevated hover:bg-base rounded-lg text-sm font-semibold text-foreground transition"
                            };

                            rsx! {
                                button {
                                    class: "{btn_class}",
                                    onclick: {
                                        move |_| {
                                            if is_allin {
                                                if let Ok(msg) = state.read().raise(0, true) {
                                                    coroutine.send(UiMessage::Action(msg));
                                                }
                                            } else if mode == StackDisplayMode::Blinds && bb > 0 {
                                                let bb_val = amount as f64 / bb as f64;
                                                if bb_val.fract() == 0.0 {
                                                    raise_input.set(format!("{}", bb_val as u64));
                                                } else {
                                                    raise_input.set(format!("{bb_val:.1}"));
                                                }
                                            } else {
                                                raise_input.set(amount.to_string());
                                            }
                                        }
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }

            // Main action buttons (bottom row)
            div { class: "flex items-center gap-3 justify-center",
                // Fold / Check
                if can_fold || can_check {
                    button {
                        class: "px-4 py-2 rounded-lg font-semibold transition {fold_check_style(can_check)}",
                        onclick: {
                            move |_| {
                                if let Some(msg) = state.read().fold_or_check() {
                                    coroutine.send(UiMessage::Action(msg));
                                }
                            }
                        },
                        "{fold_check_label}"
                    }
                }

                // Call
                if can_call {
                    button {
                        class: "px-4 py-2 bg-primary hover:bg-primary-light rounded-lg font-semibold text-foreground transition",
                        onclick: {
                            move |_| {
                                if let Some(msg) = state.read().call() {
                                    coroutine.send(UiMessage::Action(msg));
                                }
                            }
                        },
                        "Call {call_text}"
                    }
                }

                // Raise input + button
                if can_raise || can_allin {
                    {
                        let valid_raise = raise_input.read().parse::<f64>().is_ok();
                        let raise_disabled_class = if valid_raise {
                            "px-4 py-2 bg-accent hover:bg-accent-light rounded-lg font-semibold text-base transition"
                        } else {
                            "px-4 py-2 bg-accent/50 rounded-lg font-semibold text-base/50 transition cursor-not-allowed"
                        };
                        rsx! {
                            div { class: "flex items-center gap-2",
                                div { class: "flex items-center bg-muted rounded-lg focus-within:ring-2 focus-within:ring-accent",
                                    input {
                                        class: "bg-transparent px-3 py-2 text-foreground w-28 outline-none",
                                        r#type: "number",
                                        placeholder: "Amount",
                                        value: "{raise_input}",
                                        oninput: move |e| raise_input.set(e.value()),
                                    }
                                    span { class: "pr-3 text-foreground/50 text-sm select-none",
                                        if mode == StackDisplayMode::Blinds && bb > 0 { "BB" } else { "chips" }
                                    }
                                }
                                button {
                                    class: "{raise_disabled_class}",
                                    disabled: !valid_raise,
                                    onclick: {
                                        move |_| {
                                            if let Ok(raw) = raise_input.read().parse::<f64>() {
                                                let amount: u32 = if mode == StackDisplayMode::Blinds && bb > 0 {
                                                    (raw * bb as f64).round() as u32
                                                } else {
                                                    raw as u32
                                                };
                                                match state.read().raise(amount, false) {
                                                    Ok(msg) => coroutine.send(UiMessage::Action(msg)),
                                                    Err(_e) => {} // TODO: show error feedback
                                                }
                                            }
                                        }
                                    },
                                    "Raise"
                                }
                            }
                        }
                    }
                }
            }

        }
    }
}

fn fold_check_style(is_check: bool) -> &'static str {
    if is_check {
        "bg-primary hover:bg-primary-light text-foreground"
    } else {
        "bg-elevated hover:bg-base text-foreground"
    }
}
