//! Event log — scrollable list of game events.

use dioxus::prelude::*;
use poker_client::game_state::{ClientGameState, GameEvent, LogCategory};

#[component]
pub fn EventLog(state: Signal<ClientGameState>) -> Element {
    let gs = state.read();

    rsx! {
        div { class: "h-full overflow-y-auto p-3 bg-base text-sm font-mono flex flex-col gap-0.5",
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
        GameEvent::PlayerLeft { name, .. } => {
            (format!("{name} left"), category_color(LogCategory::Info))
        }
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
            name,
            action,
            amount,
            ..
        } => {
            let amt = amount.map(|a| format!(" ({a})")).unwrap_or_default();
            (
                format!("{name} {action}{amt}"),
                category_color(LogCategory::Action),
            )
        }
        GameEvent::Showdown { hands } => {
            let lines: Vec<String> = hands
                .iter()
                .map(|(_id, name, cards, hand)| {
                    format!("  {name}: {} {} — {hand}", cards[0], cards[1])
                })
                .collect();
            (
                format!("Showdown:\n{}", lines.join("\n")),
                category_color(LogCategory::Winner),
            )
        }
        GameEvent::AllInShowdown { hands } => {
            let lines: Vec<String> = hands
                .iter()
                .map(|(_id, name, cards, eq)| {
                    format!("  {name}: {} {} — {:.1}%", cards[0], cards[1], eq)
                })
                .collect();
            (
                format!("All-in showdown:\n{}", lines.join("\n")),
                category_color(LogCategory::Winner),
            )
        }
        GameEvent::RoundWinner {
            name, amount, hand, ..
        } => (
            format!("{name} wins {amount} ({hand})"),
            category_color(LogCategory::Winner),
        ),
        GameEvent::PlayerEliminated { name, .. } => (
            format!("{name} eliminated"),
            category_color(LogCategory::Info),
        ),
        GameEvent::GameOver { winner_name, .. } => (
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
        GameEvent::Text { text, category } => (text.clone(), category_color(*category)),
        GameEvent::BlindsIncreased {
            small_blind,
            big_blind,
        } => (
            format!("Blinds increased to {small_blind}/{big_blind}"),
            category_color(LogCategory::System),
        ),
        GameEvent::TurnTimerStarted {
            name, timeout_secs, ..
        } => (
            format!("{name} has {timeout_secs}s to act"),
            category_color(LogCategory::System),
        ),
        GameEvent::PlayerSatOut { name, .. } => (
            format!("{name} is sitting out"),
            category_color(LogCategory::Info),
        ),
        GameEvent::PlayerSatIn { name, .. } => (
            format!("{name} is back in"),
            category_color(LogCategory::Info),
        ),
    };

    rsx! {
        p { class: "{color}", "{text}" }
    }
}

fn category_color(cat: LogCategory) -> &'static str {
    match cat {
        LogCategory::System => "text-foreground/60",
        LogCategory::Chat => "text-primary",
        LogCategory::Action => "text-foreground/80",
        LogCategory::Winner => "text-accent",
        LogCategory::Error => "text-primary",
        LogCategory::Info => "text-foreground/70",
    }
}
