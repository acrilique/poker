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
