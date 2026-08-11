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
/// Starts the game through [`GameState::try_start`] — the engine's single
/// start path (validation, baseline freeze, first deal) — then advances the
/// engine phase by phase until the requested phase or `Showdown` is reached.
/// Requires `n_players >= 2`; `try_start` rejects fewer and the state would
/// stay in the lobby.
#[must_use]
pub fn make_state(n_players: usize, phase: GamePhase) -> GameState {
    let mut gs = GameState::new();
    gs.blind_config = BlindConfig::default();
    gs.starting_bbs = 100;
    gs.big_blind = 20;
    for i in 1..=n_players {
        gs.add_player(format!("Player{i}"));
    }
    // The first seated player hosts, like `RoomManager::join_room`.
    gs.host_id = 1;
    // Fixtures always seat >= 2 players; a failed start would leave the
    // state in the lobby and the caller's assertions visibly wrong.
    let _ = gs.try_start(1);

    // Advance the engine to the requested phase.
    loop {
        if gs.phase == phase || matches!(gs.phase, GamePhase::Showdown) {
            break;
        }
        gs.advance_phase();
    }
    gs
}
