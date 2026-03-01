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

//! Card rendering helpers.

use dioxus::prelude::*;
use poker_core::protocol::CardInfo;

/// Whether the suit should be displayed in red.
fn is_red(suit: u8) -> bool {
    // 0 = Diamonds (red), 3 = Hearts (red)
    suit == 0 || suit == 3
}

/// Render a single card face-up.
#[component]
pub fn Card(card: CardInfo) -> Element {
    let color_class = if is_red(card.suit) {
        "suit-red"
    } else {
        "suit-black"
    };

    rsx! {
        div { class: "card {color_class}",
            span { "{card.rank_str()}{card.suit_str()}" }
        }
    }
}

/// Render an empty card slot (placeholder).
#[component]
pub fn EmptyCard() -> Element {
    rsx! {
        div { class: "card-empty",
            span { "?" }
        }
    }
}

/// Render a face-down card.
#[component]
pub fn CardBack() -> Element {
    rsx! {
        div { class: "card-back",
            span { "♠" }
        }
    }
}
