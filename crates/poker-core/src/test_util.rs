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

//! Shared fixtures for tests, benches, and examples.
//!
//! Lives inside `poker-core` (rather than a separate crate) so unit tests,
//! benches, and examples all use the same crate instance of the poker types —
//! a fixture crate depending on `poker-core` would get a second copy and its
//! `Card`/`GameState` values wouldn't type-check at the call sites.

use crate::game_logic::{BlindConfig, GamePhase, GameState};
use crate::poker::{Board, Card, CardNumber, CardSuit};

/// Build a `Card` from a rank and suit.
#[must_use]
pub const fn c(rank: CardNumber, suit: CardSuit) -> Card {
    Card(rank, suit)
}

/// Build a `Board` from optional flop/turn/river cards.
#[must_use]
pub fn make_board(flop: Option<[Card; 3]>, turn: Option<Card>, river: Option<Card>) -> Board {
    Board {
        flop: flop.map(|f| (f[0], f[1], f[2])),
        turn,
        river,
    }
}

/// Build a started `GameState` with `n_players` seated, dealt to `phase`.
///
/// Mirrors the server's `handlers.rs::start_game` setup, then advances the
/// engine phase by phase (forcing betting-complete) until the requested phase
/// or `Showdown` is reached.
#[must_use]
pub fn make_state(n_players: usize, phase: GamePhase) -> GameState {
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
    gs.start_new_hand();

    // Advance the engine to the requested phase.
    loop {
        if gs.phase == phase || matches!(gs.phase, GamePhase::Showdown) {
            break;
        }
        // Force betting-complete so advance_phase is legal.
        let _ = gs.is_betting_complete();
        gs.advance_phase();
    }
    gs
}
