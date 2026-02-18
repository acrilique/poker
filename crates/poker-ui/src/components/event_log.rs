//! Event log — scrollable list of game events.

use dioxus::prelude::*;
use poker_core::game_state::{ClientGameState, GameEvent, LogCategory};

#[component]
pub fn EventLog(state: Signal<ClientGameState>) -> Element {
    let gs = state.read();

    rsx! {
        div { class: "h-full overflow-y-auto p-3 bg-gray-900 text-sm font-mono flex flex-col gap-0.5",
            for event in gs.events.iter() {
                {render_event(event)}
            }
        }
    }
}

fn render_event(event: &GameEvent) -> Element {
    let (text, color) = match event {
        GameEvent::Welcome { message } => (message.clone(), category_color(LogCategory::System)),
        GameEvent::Joined {
            player_id,
            chips,
            player_count,
        } => (
            format!("Joined as player #{player_id} with {chips} chips ({player_count} players)"),
            category_color(LogCategory::System),
        ),
        GameEvent::PlayerJoined { name, .. } => (
            format!("{name} joined the game"),
            category_color(LogCategory::Info),
        ),
        GameEvent::PlayerLeft { player_id } => (
            format!("Player #{player_id} left"),
            category_color(LogCategory::Info),
        ),
        GameEvent::Chat {
            player_name,
            message,
            ..
        } => (
            format!("{player_name}: {message}"),
            category_color(LogCategory::Chat),
        ),
        GameEvent::GameStarted => (
            "Game started!".to_string(),
            category_color(LogCategory::System),
        ),
        GameEvent::NewHand {
            hand_number,
            small_blind,
            big_blind,
            ..
        } => (
            format!("── Hand #{hand_number} ── Blinds: {small_blind}/{big_blind}"),
            category_color(LogCategory::System),
        ),
        GameEvent::HoleCards { cards } => (
            format!("Your cards: {} {}", cards[0], cards[1]),
            category_color(LogCategory::Info),
        ),
        GameEvent::CommunityCards { stage, cards } => {
            let card_str: Vec<String> = cards.iter().map(|c| c.to_string()).collect();
            (
                format!("{stage}: {}", card_str.join(" ")),
                category_color(LogCategory::Info),
            )
        }
        GameEvent::YourTurn => (
            "Your turn!".to_string(),
            category_color(LogCategory::System),
        ),
        GameEvent::PlayerActed {
            player_id,
            action,
            amount,
        } => {
            let amt = amount
                .map(|a| format!(" ({a})"))
                .unwrap_or_default();
            (
                format!("Player #{player_id} {action}{amt}"),
                category_color(LogCategory::Action),
            )
        }
        GameEvent::Showdown { hands } => {
            let lines: Vec<String> = hands
                .iter()
                .map(|(id, cards, hand)| format!("  #{id}: {} {} — {hand}", cards[0], cards[1]))
                .collect();
            (
                format!("Showdown:\n{}", lines.join("\n")),
                category_color(LogCategory::Winner),
            )
        }
        GameEvent::AllInShowdown { hands } => {
            let lines: Vec<String> = hands
                .iter()
                .map(|(id, cards, eq)| {
                    format!("  #{id}: {} {} — {:.1}%", cards[0], cards[1], eq * 100.0)
                })
                .collect();
            (
                format!("All-in showdown:\n{}", lines.join("\n")),
                category_color(LogCategory::Winner),
            )
        }
        GameEvent::RoundWinner {
            player_id,
            amount,
            hand,
        } => (
            format!("Player #{player_id} wins {amount} ({hand})"),
            category_color(LogCategory::Winner),
        ),
        GameEvent::PlayerEliminated { player_id } => (
            format!("Player #{player_id} eliminated"),
            category_color(LogCategory::Info),
        ),
        GameEvent::GameOver {
            winner_name,
            ..
        } => (
            format!("🏆 {winner_name} wins the game!"),
            category_color(LogCategory::Winner),
        ),
        GameEvent::Pong => ("Pong".to_string(), category_color(LogCategory::Info)),
        GameEvent::ServerError { message } => (
            format!("Error: {message}"),
            category_color(LogCategory::Error),
        ),
        GameEvent::Disconnected => (
            "Disconnected from server".to_string(),
            category_color(LogCategory::Error),
        ),
        GameEvent::ConnectionError { message } => (
            format!("Connection error: {message}"),
            category_color(LogCategory::Error),
        ),
        GameEvent::Unknown { raw } => (
            format!("Unknown: {raw}"),
            category_color(LogCategory::Info),
        ),
        GameEvent::Text { text, category } => (text.clone(), category_color(*category)),
        GameEvent::BlindsIncreased {
            small_blind,
            big_blind,
        } => (
            format!("Blinds increased to {small_blind}/{big_blind}"),
            category_color(LogCategory::System),
        ),
        GameEvent::TurnTimerStarted { player_id, timeout_secs } => (
            format!("Player {player_id} has {timeout_secs}s to act"),
            category_color(LogCategory::System),
        ),
    };

    rsx! {
        p { class: "{color}", "{text}" }
    }
}

fn category_color(cat: LogCategory) -> &'static str {
    match cat {
        LogCategory::System => "text-gray-400",
        LogCategory::Chat => "text-blue-400",
        LogCategory::Action => "text-gray-200",
        LogCategory::Winner => "text-emerald-400",
        LogCategory::Error => "text-red-400",
        LogCategory::Info => "text-gray-300",
    }
}
