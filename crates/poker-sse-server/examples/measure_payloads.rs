// Measurement harness: renders representative game states through the real
// `state_events` path, serializes them to SSE wire bytes, and reports
// raw / gzip / brotli sizes and ratios — i.e. the bandwidth SSE compression
// would save.
//
// Run with:  cargo run -p poker-sse-server --example measure-payloads

// Throwaway tool, not production code. Cargo.toml's panic-denying lints apply
// to examples too, so we relax them here for legibility.
#![allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::if_then_some_else_none,
    clippy::needless_pass_by_value,
    clippy::items_after_statements
)]

use std::io::Read;
use std::io::Write;

use brotli::CompressorReader as BrotliEncoder;
use flate2::Compression;
use flate2::write::GzEncoder;

use poker_core::game_logic::{GamePhase, GameState, PlayerStatus};
use poker_core::poker::{Card, CardNumber, CardSuit};
use poker_core::protocol::BlindConfig;
use poker_sse_server::render::{self, Ctx};

/// Serialize one Datastar event to the SSE wire format. Reproduces the format
/// directly (rather than via `axum::response::sse::Event`, which only exposes
/// `Display` through the body) so the harness measures the literal bytes.
fn event_to_sse(ev: &datastar::DatastarEvent) -> String {
    let mut out = String::new();
    out.push_str("event: ");
    // `EventType::as_str` is crate-private; the SDK only emits these two.
    let name = match ev.event {
        datastar::consts::EventType::PatchElements => "datastar-patch-elements",
        datastar::consts::EventType::PatchSignals => "datastar-patch-signals",
    };
    out.push_str(name);
    out.push('\n');
    // Datastar joins data lines with "\n"; axum puts them under one `data:`
    // field, prefixing each subsequent line per the SSE spec.
    for (i, line) in ev.data.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str("data: ");
        out.push_str(line);
    }
    out.push_str("\n\n");
    out
}

/// Concatenate a batch of events into the single payload one state update
/// sends down the wire.
fn payload(events: &[datastar::DatastarEvent]) -> Vec<u8> {
    events
        .iter()
        .flat_map(|ev| event_to_sse(ev).into_bytes())
        .collect()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    // In-memory: write failures here would be allocation errors only.
    enc.write_all(bytes).expect("gzip encode");
    enc.finish().expect("gzip finish")
}

fn brotli(bytes: &[u8]) -> Vec<u8> {
    // Quality 4 mirrors tower-http's default for on-the-fly brotli (see
    // tower-http-0.6.8/src/compression/body.rs).
    let mut enc = BrotliEncoder::new(bytes, 4, 22, 22);
    let mut out = Vec::new();
    enc.read_to_end(&mut out).expect("brotli encode");
    out
}

/// Build a `GameState` with `n` players, started, dealt to `phase`.
fn make_state(n_players: usize, phase: GamePhase) -> GameState {
    let mut gs = GameState::new();
    gs.blind_config = BlindConfig::default();
    gs.starting_bbs = 100;
    gs.big_blind = 20;
    for i in 1..=n_players {
        gs.add_player(format!("Player{i}"));
    }
    // Mirror handlers.rs::start_game.
    gs.game_started = true;
    gs.starting_big_blind = gs.big_blind;
    gs.starting_chips = gs.starting_bbs.saturating_mul(gs.big_blind);
    let _ = gs.start_new_hand();

    // Advance the engine to the requested phase.
    loop {
        if gs.phase == phase || matches!(gs.phase, GamePhase::Showdown) {
            break;
        }
        // Force betting-complete so advance_phase is legal.
        gs.is_betting_complete();
        let _ = gs.advance_phase();
    }
    gs
}

/// One measurement row.
struct Sample {
    name: &'static str,
    events: Vec<datastar::DatastarEvent>,
}

fn report(samples: &[Sample]) {
    println!(
        "\n{:<28} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "sample", "raw (B)", "gzip (B)", "br (B)", "gzip :1", "br :1"
    );
    println!("{}", "-".repeat(82));
    let mut total_raw = 0usize;
    let mut total_gzip = 0usize;
    let mut total_br = 0usize;
    for s in samples {
        let raw = payload(&s.events);
        let g = gzip(&raw);
        let b = brotli(&raw);
        let gr = raw.len() as f64 / g.len().max(1) as f64;
        let br = raw.len() as f64 / b.len().max(1) as f64;
        println!(
            "{:<28} {:>10} {:>10} {:>10} {:>9.1}:1 {:>9.1}:1",
            s.name,
            raw.len(),
            g.len(),
            b.len(),
            gr,
            br
        );
        total_raw += raw.len();
        total_gzip += g.len();
        total_br += b.len();
    }
    println!("{}", "-".repeat(82));
    println!(
        "{:<28} {:>10} {:>10} {:>10} {:>9.1}:1 {:>9.1}:1",
        "TOTAL",
        total_raw,
        total_gzip,
        total_br,
        total_raw as f64 / total_gzip.max(1) as f64,
        total_raw as f64 / total_br.max(1) as f64,
    );
}

#[allow(clippy::too_many_lines)]
fn main() {
    // Use deterministic hole cards / community so the size reflects a realistic
    // payload rather than RNG variance. We patch them in after make_state.
    let hole = |n: CardNumber, s: CardSuit| Card(n, s);

    // --- Sample 1: Lobby (pre-game, several players seated) ---
    let mut lobby = GameState::new();
    lobby.big_blind = 20;
    lobby.starting_bbs = 100;
    for i in 1..=6 {
        lobby.add_player(format!("Player{i}"));
    }

    // --- Sample 2: Pre-flop, 6 players, viewer is the actor ---
    let mut preflop = make_state(6, GamePhase::PreFlop);
    {
        // Give the current actor a real hand so hole cards render.
        let actor = preflop.current_player_id().unwrap_or(1);
        if let Some(p) = preflop.players.get_mut(&actor) {
            p.hole_cards = Some((
                hole(CardNumber::Ace, CardSuit::Spades),
                hole(CardNumber::King, CardSuit::Spades),
            ));
        }
    }

    // --- Sample 3: Flop, 6 players, viewer mid-hand ---
    let mut flop = make_state(6, GamePhase::Flop);
    {
        let viewer = flop.player_order.first().copied().unwrap_or(1);
        if let Some(p) = flop.players.get_mut(&viewer) {
            p.hole_cards = Some((
                hole(CardNumber::Queen, CardSuit::Hearts),
                hole(CardNumber::Jack, CardSuit::Hearts),
            ));
        }
    }

    // --- Sample 4: River with bets, 4 players, pot built up ---
    let mut river = make_state(4, GamePhase::River);
    {
        river.pot = 4_800;
        river.current_bet = 600;
        for &pid in &river.player_order.clone() {
            if let Some(p) = river.players.get_mut(&pid) {
                p.current_bet = 600;
                p.chips = p.chips.saturating_sub(600);
            }
        }
        let viewer = river.player_order.first().copied().unwrap_or(1);
        if let Some(p) = river.players.get_mut(&viewer) {
            p.hole_cards = Some((
                hole(CardNumber::Ten, CardSuit::Diamonds),
                hole(CardNumber::Nine, CardSuit::Diamonds),
            ));
        }
    }

    // --- Sample 5: All-in showdown overlay (equity table) ---
    let mut show = make_state(3, GamePhase::Showdown);
    {
        for (i, &pid) in show.player_order.iter().enumerate() {
            if let Some(p) = show.players.get_mut(&pid) {
                p.status = PlayerStatus::AllIn;
                p.hole_cards = Some((
                    hole(
                        [CardNumber::Ace, CardNumber::King, CardNumber::Queen][i],
                        CardSuit::Spades,
                    ),
                    hole(
                        [CardNumber::King, CardNumber::Queen, CardNumber::Jack][i],
                        CardSuit::Hearts,
                    ),
                ));
            }
        }
    }

    // Render each via state_events (the full fat-morph path), viewer = first seat.
    let viewer_of = |gs: &GameState| gs.player_order.first().copied().unwrap_or(1);

    let samples = vec![
        Sample {
            name: "lobby (6 players)",
            events: {
                let ctx = Ctx::new(&lobby, "ROOM42", 30);
                render::state_events(&ctx, viewer_of(&lobby))
            },
        },
        Sample {
            name: "preflop (6 players)",
            events: {
                let ctx = Ctx::new(&preflop, "ROOM42", 30);
                render::state_events(&ctx, viewer_of(&preflop))
            },
        },
        Sample {
            name: "flop (6 players)",
            events: {
                let ctx = Ctx::new(&flop, "ROOM42", 30);
                render::state_events(&ctx, viewer_of(&flop))
            },
        },
        Sample {
            name: "river+pot (4 players)",
            events: {
                let ctx = Ctx::new(&river, "ROOM42", 30);
                render::state_events(&ctx, viewer_of(&river))
            },
        },
        Sample {
            name: "all-in showdown (3)",
            events: {
                let ctx = Ctx::new(&show, "ROOM42", 30);
                render::state_events(&ctx, viewer_of(&show))
            },
        },
    ];

    report(&samples);

    println!("\nNotes:");
    println!("  - gzip uses flate2 default (level 6); brotli uses quality 4 (tower-http default).");
    println!(
        "  - Sizes are the SSE wire bytes (event:/data:/blank-line framing), per state update."
    );
    println!(
        "  - A live game pushes one such payload to every connected viewer on each terminal point."
    );
}
