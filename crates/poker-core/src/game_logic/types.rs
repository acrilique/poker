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

//! Public types of the game engine: player/action/phase enums, wire types,
//! and errors. [`super`] re-exports them at `poker_core::game_logic`, so
//! `use poker_core::game_logic::PlayerAction` keeps working.

use std::fmt;

use crate::poker::Card;
use serde::{Deserialize, Serialize};

/// An action the player can take during a betting round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerAction {
    Fold,
    Check,
    Call,
    Raise,
    #[serde(rename = "allin")]
    AllIn,
}

impl PlayerAction {
    /// Human-readable label for UI display.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fold => "Fold",
            Self::Check => "Check",
            Self::Call => "Call",
            Self::Raise => "Raise",
            Self::AllIn => "All-In",
        }
    }
}

impl fmt::Display for PlayerAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Configuration for automatic blind increases.
///
/// When `interval_secs` is 0 (or `None` on the wire) blinds never increase.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BlindConfig {
    /// Seconds between each blind increase (0 = disabled).
    #[serde(default)]
    pub interval_secs: u64,
    /// Percentage by which blinds increase each interval (e.g. 50 = +50%).
    #[serde(default)]
    pub increase_percent: u32,
}

impl BlindConfig {
    /// Returns `true` when blind increases are enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.interval_secs > 0 && self.increase_percent > 0
    }
}

/// Player status in current hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerStatus {
    /// Not yet in a hand.
    Waiting,
    /// Still playing in this hand.
    Active,
    /// Folded this hand.
    Folded,
    /// All-in this hand.
    AllIn,
    /// Eliminated from game (no chips).
    Out,
}

/// Represents a connected player.
#[derive(Debug, Clone)]
pub struct Player {
    pub id: u32,
    pub name: String,
    pub chips: u32,
    pub status: PlayerStatus,
    pub hole_cards: Option<(Card, Card)>,
    /// Amount bet in current betting round.
    pub current_bet: u32,
    /// Whether the player is sitting out (auto-check/fold each turn).
    pub sitting_out: bool,
}

/// Game phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamePhase {
    Lobby,
    PreFlop,
    Flop,
    Turn,
    River,
    Showdown,
    HandOver,
}

impl GamePhase {
    /// A betting round is live: players act on their turns (pre-flop through
    /// river). Shared by the engine's turn logic and the transport's
    /// turn/action-bar renderers so "which phases have a turn" is defined once.
    #[must_use]
    pub const fn is_betting(self) -> bool {
        matches!(self, Self::PreFlop | Self::Flop | Self::Turn | Self::River)
    }

    /// A dealt hand is still in progress: any betting phase or showdown.
    /// Excludes the lobby and the post-resolve wait, where per-player
    /// `current_bet` values are stale.
    #[must_use]
    pub const fn is_in_hand(self) -> bool {
        matches!(
            self,
            Self::PreFlop | Self::Flop | Self::Turn | Self::River | Self::Showdown
        )
    }
}

/// Error from [`crate::game_logic::GameState::apply_action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionError {
    /// The game hasn't started.
    GameNotStarted,
    /// It isn't this player's turn.
    NotYourTurn,
    /// The action isn't in the player's valid set.
    InvalidAction,
    /// The player ID isn't seated.
    PlayerNotFound,
    /// `Check` was requested with a non-zero amount to call.
    CannotCheckMustCallOrRaise,
    /// `Raise` requested with insufficient chips.
    NotEnoughChips { have: u32, need: u32 },
    /// `Raise` below the minimum raise floor (and not an all-in).
    RaiseBelowMinimum { min: u32 },
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GameNotStarted => f.write_str("Game not started"),
            Self::NotYourTurn => f.write_str("Not your turn"),
            Self::InvalidAction => f.write_str("Invalid action"),
            Self::PlayerNotFound => f.write_str("Player not found"),
            Self::CannotCheckMustCallOrRaise => f.write_str("Cannot check, must call or raise"),
            Self::NotEnoughChips { have, need } => {
                write!(f, "Not enough chips. Have {have}, need {need}")
            }
            Self::RaiseBelowMinimum { min } => write!(f, "Minimum raise is {min}"),
        }
    }
}

impl std::error::Error for ActionError {}

/// Error from [`crate::game_logic::GameState::try_start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartGameError {
    /// The game has already started.
    AlreadyStarted,
    /// Fewer than two players are seated.
    NotEnoughPlayers,
    /// The caller is not the room host.
    NotHost,
}

impl std::fmt::Display for StartGameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyStarted => f.write_str("Game already started"),
            Self::NotEnoughPlayers => f.write_str("Need at least 2 players to start"),
            Self::NotHost => f.write_str("Only the host can perform this action"),
        }
    }
}

impl std::error::Error for StartGameError {}
