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

//! Server-side game logic: types, state management, and betting rules.
//!
//! This module is transport-agnostic — it knows nothing about TCP, channels,
//! or serialization.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::poker::{Board, Card, FullHand, Hand, HandRank, best_hand_indices, get_all_cards};
use rand::rng;
use rand::seq::SliceRandom;

pub mod types;
pub use types::{
    ActionError, BlindConfig, GamePhase, Player, PlayerAction, PlayerStatus, StartGameError,
};

/// Fixed per-turn timer duration in seconds.
///
/// When a player's turn begins the server starts a countdown.  If the player
/// has not acted by the time it reaches zero, the server forces a *check* (if
/// allowed) or a *fold*.
pub const TURN_TIMEOUT_SECS: u32 = 30;

// ---------------------------------------------------------------------------
// GameState
// ---------------------------------------------------------------------------

/// Server-side game state shared across all connections.
///
/// The five independent boolean flags (`game_started`, `big_blind_option`,
/// `has_acted_this_round`, `allow_late_entry`, `waiting_for_players`) are each
/// semantically distinct engine state accessed directly at ~100 sites;
/// grouping them would obscure the state machine without a correctness
/// benefit, so the bool cap is relaxed here.
#[allow(clippy::struct_excessive_bools)]
pub struct GameState {
    pub players: HashMap<u32, Player>,
    /// Order of play (seat positions).
    pub player_order: Vec<u32>,
    pub next_player_id: u32,
    pub game_started: bool,
    pub phase: GamePhase,
    pub hand_number: u32,
    pub dealer_index: usize,
    pub current_player_index: usize,
    pub pot: u32,
    /// Current bet to match.
    pub current_bet: u32,
    pub min_raise: u32,
    pub small_blind: u32,
    pub big_blind: u32,
    pub deck: Vec<Card>,
    pub community_cards: Vec<Card>,
    /// Track who last raised.
    pub last_raiser_index: Option<usize>,
    /// Track if big blind has had option to act in pre-flop.
    pub big_blind_option: bool,
    /// Track who was first to act in this betting round.
    pub first_actor_index: Option<usize>,
    /// Track if current player has acted at least once.
    pub has_acted_this_round: bool,
    /// Configuration for automatic blind increases.
    pub blind_config: BlindConfig,
    /// When blinds were last increased (or when the game started).
    pub last_blind_increase: Option<Instant>,
    /// Number of big blinds each player starts with.
    pub starting_bbs: u32,
    /// Whether late entry is allowed (toggled by host).
    pub allow_late_entry: bool,
    /// Player ID of the room host (first player to join).
    pub host_id: u32,
    /// Starting chip count, frozen at game start for late entries.
    pub starting_chips: u32,
    /// Initial big blind value, frozen at game start so that
    /// `starting_bbs * starting_big_blind` always equals the original buy-in.
    pub starting_big_blind: u32,
    /// True when the game is paused because fewer than 2 players are active
    /// (not sitting out). Cleared when a player sits back in and triggers
    /// a new hand.
    pub waiting_for_players: bool,
    /// Total chips each player has contributed to the pot in the current hand
    /// (across all betting rounds).  Used for side-pot calculation.
    pub pot_contributions: HashMap<u32, u32>,
    /// Winners of the most recently resolved hand: `(player_id, amount_won,
    /// rank)`. `rank` is `None` when the pot was won without a showdown (the
    /// remaining player(s) took it by fold). Populated by [`Self::resolve_hand`],
    /// cleared by [`Self::start_new_hand`]. The UI reads it during
    /// [`GamePhase::HandOver`] to show who won how much while waiting for the
    /// next deal.
    pub last_winners: Vec<(u32, u32, Option<HandRank>)>,
}

impl Default for GameState {
    fn default() -> Self {
        Self {
            players: HashMap::new(),
            player_order: Vec::new(),
            next_player_id: 1,
            game_started: false,
            phase: GamePhase::Lobby,
            hand_number: 0,
            dealer_index: 0,
            current_player_index: 0,
            pot: 0,
            current_bet: 0,
            min_raise: 0,
            small_blind: 10,
            big_blind: 20,
            deck: Vec::new(),
            community_cards: Vec::new(),
            last_raiser_index: None,
            big_blind_option: false,
            first_actor_index: None,
            has_acted_this_round: false,
            blind_config: BlindConfig::default(),
            last_blind_increase: None,
            starting_bbs: 50,
            allow_late_entry: false,
            host_id: 0,
            starting_chips: 0,
            starting_big_blind: 0,
            waiting_for_players: false,
            pot_contributions: HashMap::new(),
            last_winners: Vec::new(),
        }
    }
}

/// `(base + offset) mod seats`, computed with checked arithmetic so it can't
/// overflow or divide by zero. Returns `0` when `seats == 0`.
///
/// Public so the transport can derive seat positions (blinds, dealer) from
/// the same arithmetic the engine uses.
#[must_use]
pub const fn next_seat(base: usize, offset: usize, seats: usize) -> usize {
    if seats == 0 {
        return 0;
    }
    base.rem_euclid(seats)
        .saturating_add(offset)
        .rem_euclid(seats)
}

/// One blind-level step: `cur` increased by `pct`%, rounded up to the next
/// chip. Shared by [`GameState::start_new_hand`]'s catch-up loop and
/// [`GameState::next_blinds`] so the two blind schedules can't drift.
const fn next_blind_level(cur: u32, pct: u32) -> u32 {
    cur.saturating_add(cur.saturating_mul(pct).div_ceil(100))
}

impl GameState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_player(&mut self, name: String) -> Player {
        self.add_player_with_chips(name, None)
    }

    /// Add a player with an optional chip override (used for late entries).
    pub fn add_player_with_chips(&mut self, name: String, chips_override: Option<u32>) -> Player {
        let bb = if self.starting_big_blind > 0 {
            self.starting_big_blind
        } else {
            self.big_blind
        };
        let starting_chips = chips_override.unwrap_or_else(|| self.starting_bbs.saturating_mul(bb));
        let player = Player {
            id: self.next_player_id,
            name,
            chips: starting_chips,
            status: PlayerStatus::Waiting,
            hole_cards: None,
            current_bet: 0,
            sitting_out: false,
        };
        self.players.insert(player.id, player.clone());
        self.player_order.push(player.id);
        self.next_player_id = self.next_player_id.saturating_add(1);
        player
    }

    pub fn remove_player(&mut self, id: u32) {
        self.players.remove(&id);
        self.player_order.retain(|&pid| pid != id);
    }

    /// Promote a new host if the player just removed was the host.
    pub fn promote_next_host(&mut self, removed_id: u32) -> Option<u32> {
        if self.host_id != removed_id || self.player_order.is_empty() {
            return None;
        }
        // player_order holds remaining players; pick the lowest id for
        // deterministic promotion.
        let next = *self.player_order.iter().min()?;
        self.host_id = next;
        Some(next)
    }

    #[must_use]
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// Set a player to sitting out.
    pub fn set_sitting_out(&mut self, player_id: u32) {
        if let Some(player) = self.players.get_mut(&player_id) {
            player.sitting_out = true;
        }
    }

    /// Set a player back to active (no longer sitting out).
    pub fn set_sitting_in(&mut self, player_id: u32) {
        if let Some(player) = self.players.get_mut(&player_id) {
            player.sitting_out = false;
        }
    }

    /// Check whether the current player is sitting out.
    #[must_use]
    pub fn is_current_player_sitting_out(&self) -> bool {
        self.current_player_id()
            .and_then(|id| self.players.get(&id))
            .is_some_and(|p| p.sitting_out)
    }

    /// Get active players (not folded, not out).
    #[must_use]
    pub fn active_player_count(&self) -> usize {
        self.players
            .values()
            .filter(|p| p.status == PlayerStatus::Active || p.status == PlayerStatus::AllIn)
            .count()
    }

    /// Whether the game has reached a terminal state. The engine sets
    /// `game_started = false` and `phase = Lobby` only in [`resolve_hand`],
    /// once a single player holds all the chips. A game that never started
    /// (`hand_number == 0`) is not "over".
    #[must_use]
    pub const fn is_game_over(&self) -> bool {
        !self.game_started && matches!(self.phase, GamePhase::Lobby) && self.hand_number > 0
    }

    /// Get players who can still act (active but not all-in).
    ///
    /// One of three "how many players" predicates — pick by intent:
    /// - [`Self::active_player_count`] — mid-hand, by status flag (`Active | AllIn`).
    /// - [`Self::actionable_players`] — players who can still bet this round (`Active` only).
    /// - [`Self::dealable_player_count`] — seated, not sitting out, with chips; the
    ///   "can we deal a new hand?" threshold.
    #[must_use]
    pub fn actionable_players(&self) -> Vec<u32> {
        self.player_order
            .iter()
            .filter(|&&id| {
                self.players
                    .get(&id)
                    .is_some_and(|p| p.status == PlayerStatus::Active)
            })
            .copied()
            .collect()
    }

    /// Count seated players who are not sitting out and still hold chips — the
    /// threshold for dealing a new hand. See [`Self::actionable_players`] for the
    /// other player-count predicates.
    #[must_use]
    pub fn dealable_player_count(&self) -> usize {
        self.player_order
            .iter()
            .filter(|&&id| {
                self.players
                    .get(&id)
                    .is_some_and(|p| !p.sitting_out && p.chips > 0)
            })
            .count()
    }

    /// The player's hole cards as a [`Hand`], if they have any.
    #[must_use]
    pub fn hand_of(&self, player_id: u32) -> Option<Hand> {
        let (c1, c2) = self.players.get(&player_id)?.hole_cards?;
        Some(Hand(c1, c2))
    }

    /// The hands still in play: `(player_id, Hand)` for every Active/AllIn
    /// player with hole cards, in seat order. Shared by the engine's
    /// [`Self::resolve_hand`] and the transport's showdown renderers so the
    /// "collect live hands" pattern lives in one place.
    #[must_use]
    pub fn live_hands(&self) -> Vec<(u32, Hand)> {
        self.player_order
            .iter()
            .filter(|&&id| {
                self.players
                    .get(&id)
                    .is_some_and(|p| matches!(p.status, PlayerStatus::Active | PlayerStatus::AllIn))
            })
            .filter_map(|&id| self.hand_of(id).map(|hand| (id, hand)))
            .collect()
    }

    /// Shuffle and create a new deck.
    pub fn new_deck(&mut self) {
        self.deck = get_all_cards().to_vec();
        let mut rng = rng();
        self.deck.shuffle(&mut rng);
    }

    /// Deal a card from the deck.
    pub fn deal_card(&mut self) -> Option<Card> {
        self.deck.pop()
    }

    /// Start the game: validate the caller and preconditions, freeze the
    /// starting-chips/big-blind baseline (so late entrants match the original
    /// buy-in), seed the blind schedule, and deal the first hand.
    ///
    /// This owns the game-start invariants so transports don't re-derive them.
    /// The caller still renders state and notifies the first player's turn
    /// after this returns.
    ///
    /// # Errors
    /// Returns [`StartGameError`] when a precondition fails. The transport
    /// surfaces the message to the player.
    pub fn try_start(&mut self, host_id: u32) -> Result<(), StartGameError> {
        if self.game_started {
            return Err(StartGameError::AlreadyStarted);
        }
        if self.player_count() < 2 {
            return Err(StartGameError::NotEnoughPlayers);
        }
        self.require_host(host_id)?;

        self.game_started = true;

        // Freeze the starting chip amount and big blind for late entries.
        self.starting_big_blind = self.big_blind;
        self.starting_chips = self.starting_bbs.saturating_mul(self.big_blind);

        // Initialise the blind increase timer if configured.
        if self.blind_config.is_enabled() {
            self.last_blind_increase = Some(Instant::now());
        }

        self.start_new_hand();
        Ok(())
    }

    /// Check that `id` is the room host. Host-gated actions in the transport
    /// layers use this; [`Self::try_start`] shares the same check.
    ///
    /// # Errors
    /// Returns [`StartGameError::NotHost`] when `id` is not the host.
    pub const fn require_host(&self, id: u32) -> Result<(), StartGameError> {
        if self.host_id != id {
            return Err(StartGameError::NotHost);
        }
        Ok(())
    }

    /// Start a new hand.
    pub fn start_new_hand(&mut self) {
        // Blind increases run on a wall-clock schedule anchored to game start.
        // Players don't see a step until the next hand (when blinds are posted).
        if self.blind_config.is_enabled()
            && let Some(mut last) = self.last_blind_increase
        {
            let interval = Duration::from_secs(self.blind_config.interval_secs);
            let pct = self.blind_config.increase_percent;
            while last.elapsed() >= interval {
                self.small_blind = next_blind_level(self.small_blind, pct);
                self.big_blind = next_blind_level(self.big_blind, pct);
                // Advance the anchor by exactly one interval to stay anchored
                // to game start. `checked_add` is None only near a
                // monotonically-distant future; in that case stop.
                let Some(next) = last.checked_add(interval) else {
                    break;
                };
                last = next;
                self.last_blind_increase = Some(last);
            }
        }

        self.hand_number = self.hand_number.saturating_add(1);
        self.phase = GamePhase::PreFlop;
        self.pot = 0;
        self.pot_contributions.clear();
        self.last_winners.clear();
        self.current_bet = 0;
        self.community_cards.clear();
        self.new_deck();

        // Reset player states for new hand
        for player in self.players.values_mut() {
            if player.chips > 0 {
                player.status = PlayerStatus::Active;
            } else {
                player.status = PlayerStatus::Out;
            }
            player.hole_cards = None;
            player.current_bet = 0;
        }

        // Remove eliminated players from order
        self.player_order
            .retain(|&id| self.players.get(&id).is_some_and(|p| p.chips > 0));

        if self.player_order.len() < 2 {
            return;
        }

        // Move dealer button
        self.dealer_index = next_seat(self.dealer_index, 1, self.player_order.len());

        // Determine blinds positions
        let sb_index = next_seat(self.dealer_index, 1, self.player_order.len());
        let bb_index = next_seat(self.dealer_index, 2, self.player_order.len());

        // The ≥ 2 player guard above guarantees these seats exist; extract them
        // without indexing so a future change to that guard can't panic here.
        let Some(sb_id) = self.player_order.get(sb_index).copied() else {
            return;
        };
        let Some(bb_id) = self.player_order.get(bb_index).copied() else {
            return;
        };

        // Post blinds
        self.post_blind(sb_id, self.small_blind);
        self.post_blind(bb_id, self.big_blind);
        self.current_bet = self.big_blind;
        self.min_raise = self.big_blind;

        // Action starts after big blind
        self.current_player_index = next_seat(bb_index, 1, self.player_order.len());
        self.last_raiser_index = Some(bb_index);
        self.big_blind_option = true;
        self.first_actor_index = Some(self.current_player_index);
        self.has_acted_this_round = false;

        // Deal hole cards
        let players_to_deal: Vec<u32> = self
            .player_order
            .iter()
            .filter(|&&id| {
                self.players.get(&id).is_some_and(|p| {
                    p.status == PlayerStatus::Active || p.status == PlayerStatus::AllIn
                })
            })
            .copied()
            .collect();

        for player_id in players_to_deal {
            // The freshly shuffled 52-card deck always holds enough cards for
            // the capped player count, but deal defensively rather than panic.
            let Some(c1) = self.deal_card() else {
                break;
            };
            let Some(c2) = self.deal_card() else {
                break;
            };
            if let Some(player) = self.players.get_mut(&player_id) {
                player.hole_cards = Some((c1, c2));
            }
        }
    }

    fn post_blind(&mut self, player_id: u32, amount: u32) {
        // A blind can only cover what the player has; the shortfall goes all-in.
        let Some(player) = self.players.get(&player_id) else {
            return;
        };
        let actual = amount.min(player.chips);
        // Blinds are posted from a zeroed `current_bet` (see `start_new_hand`),
        // so the shared add is equivalent to setting the bet.
        self.place_bet(player_id, actual, 0);
    }

    /// Get the current player's ID.
    #[must_use]
    pub fn current_player_id(&self) -> Option<u32> {
        self.player_order.get(self.current_player_index).copied()
    }

    /// Check if betting round is complete.
    #[must_use]
    pub fn is_betting_complete(&self) -> bool {
        let actionable = self.actionable_players();

        if actionable.is_empty() {
            return true;
        }

        if self.active_player_count() <= 1 {
            return true;
        }

        let mut all_bets_matched = true;
        for &id in &self.player_order {
            if let Some(player) = self.players.get(&id)
                && player.status == PlayerStatus::Active
                && player.current_bet < self.current_bet
            {
                all_bets_matched = false;
                break;
            }
        }

        if !all_bets_matched {
            return false;
        }

        if self.phase == GamePhase::PreFlop && self.big_blind_option {
            return false;
        }

        if let Some(raiser_idx) = self.last_raiser_index {
            let raiser_id = self.player_order.get(raiser_idx).copied();
            let raiser_can_act = raiser_id
                .and_then(|id| self.players.get(&id))
                .is_some_and(|p| p.status == PlayerStatus::Active);

            if raiser_can_act && self.current_player_index != raiser_idx {
                return false;
            }

            return true;
        }

        if !self.has_acted_this_round {
            return false;
        }

        if let Some(first_idx) = self.first_actor_index {
            // The original first actor may have folded (or gone all-in) since
            // the index was recorded.  Advance to the next Active player from
            // that position so the sentinel is reachable by next_player().
            let mut sentinel = first_idx;
            for _ in 0..self.player_order.len() {
                if self
                    .player_order
                    .get(sentinel)
                    .and_then(|&id| self.players.get(&id))
                    .is_some_and(|p| p.status == PlayerStatus::Active)
                {
                    break;
                }
                sentinel = next_seat(sentinel, 1, self.player_order.len());
            }
            return self.current_player_index == sentinel;
        }

        true
    }

    /// Walk forward from the current seat to the next seat whose player is
    /// [`PlayerStatus::Active`], wrapping at most once around `player_order`.
    ///
    /// With `include_start` the starting seat itself is checked first
    /// ([`Self::advance_phase`] begins each phase at the small-blind seat);
    /// without it the scan starts at the following seat, so the player who
    /// just acted ([`Self::next_player`]) is always skipped. If the full
    /// circle has no Active seat, the index is left on the starting seat.
    fn advance_to_active_seat(&mut self, include_start: bool) {
        let start = self.current_player_index;
        let mut cursor = if include_start {
            start
        } else {
            next_seat(start, 1, self.player_order.len())
        };
        loop {
            let is_active = self
                .player_order
                .get(cursor)
                .and_then(|&id| self.players.get(&id))
                .is_some_and(|p| p.status == PlayerStatus::Active);
            if is_active {
                self.current_player_index = cursor;
                return;
            }
            cursor = next_seat(cursor, 1, self.player_order.len());
            if cursor == start {
                // Full circle without an Active seat — fall back to the start.
                self.current_player_index = start;
                return;
            }
        }
    }

    /// Move to next player.
    pub fn next_player(&mut self) {
        // Skip the seat that just acted: the scan starts after it.
        self.advance_to_active_seat(false);
    }

    /// Advance to next phase.
    pub fn advance_phase(&mut self) {
        for player in self.players.values_mut() {
            player.current_bet = 0;
        }
        self.current_bet = 0;
        self.last_raiser_index = None;
        self.big_blind_option = false;
        self.has_acted_this_round = false;

        self.current_player_index = next_seat(self.dealer_index, 1, self.player_order.len());
        // The small-blind seat opens the phase, so it is eligible itself.
        self.advance_to_active_seat(true);

        self.first_actor_index = Some(self.current_player_index);

        match self.phase {
            GamePhase::PreFlop => {
                self.phase = GamePhase::Flop;
                for _ in 0..3 {
                    if let Some(card) = self.deal_card() {
                        self.community_cards.push(card);
                    }
                }
            }
            GamePhase::Flop => {
                self.phase = GamePhase::Turn;
                if let Some(card) = self.deal_card() {
                    self.community_cards.push(card);
                }
            }
            GamePhase::Turn => {
                self.phase = GamePhase::River;
                if let Some(card) = self.deal_card() {
                    self.community_cards.push(card);
                }
            }
            GamePhase::River => {
                self.phase = GamePhase::Showdown;
            }
            _ => {}
        }
    }

    /// Calculate side pots from player contributions.
    ///
    /// Returns a list of `(pot_amount, eligible_player_ids)` tuples sorted
    /// from main pot (lowest contribution tier) to highest side pot.
    /// "Eligible" means the player contributed enough **and** has not folded.
    fn calculate_side_pots(&self) -> Vec<(u32, Vec<u32>)> {
        // Gather every player's total contribution (including folded players).
        let contributions: Vec<(u32, u32)> = self
            .pot_contributions
            .iter()
            .filter(|&(_, &amount)| amount > 0)
            .map(|(&id, &amount)| (id, amount))
            .collect();

        if contributions.is_empty() {
            return Vec::new();
        }

        // Unique contribution levels, ascending.
        let mut levels: Vec<u32> = contributions.iter().map(|(_, a)| *a).collect();
        levels.sort_unstable();
        levels.dedup();

        let mut side_pots: Vec<(u32, Vec<u32>)> = Vec::new();
        let mut prev_level = 0u32;

        for &level in &levels {
            let layer = level.saturating_sub(prev_level);
            if layer == 0 {
                continue;
            }

            // How many players contributed at least `level` chips total?
            let contributors: usize = contributions.iter().filter(|(_, a)| *a >= level).count();

            let contributors_u32 = u32::try_from(contributors).unwrap_or(0);
            let pot_amount = layer.saturating_mul(contributors_u32);

            // Only non-folded players who contributed at least `level` are
            // eligible to *win* this sub-pot.
            let eligible: Vec<u32> = contributions
                .iter()
                .filter(|(id, a)| {
                    *a >= level
                        && self.players.get(id).is_some_and(|p| {
                            p.status == PlayerStatus::Active || p.status == PlayerStatus::AllIn
                        })
                })
                .map(|(id, _)| *id)
                .collect();

            if pot_amount > 0 {
                side_pots.push((pot_amount, eligible));
            }

            prev_level = level;
        }

        side_pots
    }

    /// Find the winner(s) of a pot among a set of eligible player hands.
    ///
    /// Returns the winning player IDs and the winning hand rank. The rank is
    /// `None` when the pot is won without a showdown (at most one eligible
    /// hand, or no eligible hand has a 5-card combination). Winner selection
    /// reuses [`best_hand_indices`] (`poker-core::poker`) — the same
    /// single-pass, ties-kept-all semantics as the equity simulator — instead
    /// of a hand-rolled pairwise loop.
    fn find_pot_winners(
        hands: &[(u32, Hand)],
        eligible: &[u32],
        board: &Board,
    ) -> (Vec<u32>, Option<HandRank>) {
        let eligible_hands: Vec<&(u32, Hand)> = hands
            .iter()
            .filter(|(id, _)| eligible.contains(id))
            .collect();

        if eligible_hands.len() <= 1 {
            let id = eligible_hands.first().map_or(0, |(id, _)| *id);
            return (vec![id], None);
        }

        // Evaluate every eligible hand; hands with no 5-card combination are
        // dropped (they can't win). `best` always succeeds here because
        // `resolve_hand` only runs on a full board, but keep it defensive.
        let ranked: Vec<(u32, FullHand)> = eligible_hands
            .iter()
            .filter_map(|(id, hand)| hand.best(board).map(|full| (*id, full)))
            .collect();

        if ranked.is_empty() {
            return (Vec::new(), None);
        }

        let winning_ids = best_hand_indices(&ranked);
        // All winners tie, so any winner's rank is the winning rank.
        let best_rank = winning_ids
            .first()
            .and_then(|wid| ranked.iter().find(|(id, _)| id == wid))
            .map(|(_, full)| full.rank());

        (winning_ids, best_rank)
    }

    /// Split `total` chips evenly across `winners`; odd chips go to the lowest
    /// player IDs. Accumulates into `winnings` keyed by player ID. Arithmetic
    /// is saturating/checked, so this can't panic or overflow.
    fn award_split(
        winnings: &mut HashMap<u32, (u32, Option<HandRank>)>,
        total: u32,
        winners: &[u32],
        rank: Option<HandRank>,
    ) {
        let Ok(count_u32) = u32::try_from(winners.len()) else {
            return;
        };
        // `checked_div`/`checked_rem` are None only on divide-by-zero; count is
        // ≥ 1 here (an empty winners slice is a no-op below), so the unwrap is
        // safe and avoids the side-effect-flagging `/` and `%`.
        let count = count_u32.max(1);
        let share = total.checked_div(count).unwrap_or(0);
        let remainder = total.checked_rem(count).unwrap_or(0);
        for (i, id) in winners.iter().enumerate() {
            let extra = u32::try_from(i).map_or(0, |i| u32::from(i < remainder));
            let entry = winnings.entry(*id).or_insert_with(|| (0, rank));
            entry.0 = entry.0.saturating_add(share).saturating_add(extra);
        }
    }

    /// Determine winner(s) and distribute pot using side-pot logic.
    pub fn resolve_hand(&mut self) {
        let hands_to_show = self.live_hands();

        if hands_to_show.is_empty() {
            return;
        }

        let board = self.build_board();

        // Accumulate winnings per player across all side pots.
        let mut winnings: HashMap<u32, (u32, Option<HandRank>)> = HashMap::new();

        if hands_to_show.len() == 1 {
            // Everyone else folded — sole survivor takes the whole pot.
            let Some((id, _)) = hands_to_show.first() else {
                return;
            };
            // No showdown, so there is no rank to report.
            winnings.insert(*id, (self.pot, None));
        } else {
            // Calculate and award each side pot independently.
            let side_pots = self.calculate_side_pots();

            // Track "dead money" from side pots where all eligible players
            // folded.  This is carried forward and awarded to the winners of
            // the next pot that has at least one eligible winner.
            let mut carry_over: u32 = 0;
            // Remember the last set of winners so that if the highest side pot
            // is uncontested we can give the carry-over to the most recent
            // winners.
            let mut last_winners: Vec<u32> = Vec::new();
            let mut last_rank: Option<HandRank> = None;

            for (pot_amount, eligible) in &side_pots {
                if eligible.is_empty() {
                    // All contributors at this tier folded — accumulate as
                    // dead money for the next contested pot.
                    carry_over = carry_over.saturating_add(*pot_amount);
                    continue;
                }

                let (pot_winners, rank) = Self::find_pot_winners(&hands_to_show, eligible, &board);

                if pot_winners.is_empty() {
                    carry_over = carry_over.saturating_add(*pot_amount);
                    continue;
                }

                // Include any accumulated dead money from previous
                // uncontested pots.
                let total = pot_amount.saturating_add(carry_over);
                carry_over = 0;

                Self::award_split(&mut winnings, total, &pot_winners, rank);

                last_winners = pot_winners;
                last_rank = rank;
            }

            // If there's still dead money left over (the highest side pot(s)
            // had no eligible winners), give it to the last known winners.
            if carry_over > 0 && !last_winners.is_empty() {
                Self::award_split(&mut winnings, carry_over, &last_winners, last_rank);
            }
        }

        // Build the winners list and award chips.
        let mut winners: Vec<(u32, u32, Option<HandRank>)> = winnings
            .into_iter()
            .map(|(id, (amount, rank))| (id, amount, rank))
            .collect();
        // Sort by player ID for deterministic ordering.
        winners.sort_unstable_by_key(|(id, _, _)| *id);

        for (winner_id, amount, _) in &winners {
            if let Some(player) = self.players.get_mut(winner_id) {
                player.chips = player.chips.saturating_add(*amount);
            }
        }

        self.last_winners.clone_from(&winners);

        self.phase = GamePhase::HandOver;

        // The game ends when exactly one player holds all the chips.
        if self.players.values().filter(|p| p.chips > 0).count() == 1 {
            self.game_started = false;
            self.phase = GamePhase::Lobby;
        }

        self.pot = 0;
    }

    /// Build a [`Board`] from the current community cards.
    #[must_use]
    pub fn build_board(&self) -> Board {
        let flop = match self.community_cards.as_slice() {
            &[c1, c2, c3, ..] => Some((c1, c2, c3)),
            _ => None,
        };

        let turn = self.community_cards.get(3).copied();
        let river = self.community_cards.get(4).copied();

        Board { flop, turn, river }
    }

    /// The next blind level, computed via the shared [`next_blind_level`] step.
    /// Returns the current level unchanged when rising blinds aren't configured.
    #[must_use]
    pub const fn next_blinds(&self) -> (u32, u32) {
        let pct = self.blind_config.increase_percent;
        (
            next_blind_level(self.small_blind, pct),
            next_blind_level(self.big_blind, pct),
        )
    }

    /// Seconds remaining until the next blind increase, anchored to
    /// [`Self::last_blind_increase`] with the same `last + n*interval` semantics
    /// the `start_new_hand` catch-up loop uses — so the countdown reaches 0 at
    /// exactly the moment the next hand will step the level. `None` when rising
    /// blinds aren't configured or the anchor hasn't been set yet (lobby).
    #[must_use]
    pub fn seconds_to_next_blind(&self) -> Option<u64> {
        if !self.blind_config.is_enabled() {
            return None;
        }
        let last = self.last_blind_increase?;
        let interval = self.blind_config.interval_secs;
        // Elapsed since the anchor, modulo one interval: how far into the
        // current level we are. The remainder is what's left of this level.
        let elapsed_secs = last.elapsed().as_secs();
        let into_level = elapsed_secs.rem_euclid(interval);
        let remaining = interval.saturating_sub(into_level);
        Some(remaining)
    }

    /// Apply host-initiated settings: a new blind schedule and (pre-game only)
    /// a new starting stack.
    ///
    /// `starting_bbs` is frozen into `starting_chips` at game start
    /// ([`Self::try_start`]), so only apply it pre-game. Chips are frozen into
    /// each seated player at join time (`add_player_with_chips`), so when the
    /// stack changes pre-game — before any chips have been won or lost — every
    /// player is still at the now-stale buy-in and must be rebought at the new
    /// amount so they match subsequent joiners. `big_blind` is the right
    /// multiplier here: `starting_big_blind` is only frozen at game start, so
    /// it's still 0 pre-game (matching the `bb` selection in
    /// `add_player_with_chips`).
    ///
    /// Mid-game, re-anchor the blind schedule to now so the catch-up loop in
    /// [`Self::start_new_hand`] doesn't step blinds repeatedly when the interval
    /// changes.
    pub fn apply_settings(&mut self, config: BlindConfig, starting_bbs: u32) {
        self.blind_config = config;

        if !self.game_started {
            self.starting_bbs = starting_bbs;
            let new_stack = starting_bbs.saturating_mul(self.big_blind);
            for player in self.players.values_mut() {
                player.chips = new_stack;
            }
        }

        if self.game_started && self.blind_config.is_enabled() {
            self.last_blind_increase = Some(Instant::now());
        }
    }

    /// Get valid actions for current player.
    #[must_use]
    pub fn valid_actions(&self, player_id: u32) -> Vec<PlayerAction> {
        let mut actions = Vec::new();

        if let Some(player) = self.players.get(&player_id) {
            if player.status != PlayerStatus::Active {
                return actions;
            }

            actions.push(PlayerAction::Fold);

            let to_call = self.current_bet.saturating_sub(player.current_bet);

            if to_call == 0 {
                actions.push(PlayerAction::Check);
            } else if player.chips >= to_call {
                actions.push(PlayerAction::Call);
            }

            if player.chips > to_call {
                actions.push(PlayerAction::Raise);
            }

            if player.chips > 0 {
                actions.push(PlayerAction::AllIn);
            }
        }

        actions
    }

    /// The auto-action a sitting-out player (or one whose turn timer expired)
    /// takes: `Check` when legal, otherwise `Fold`. `None` if the player has no
    /// valid action (not seated / not active). Unifies the check-or-fold logic
    /// that previously lived inline in the transport layer's turn-timer and
    /// disconnect paths.
    #[must_use]
    pub fn auto_action(&self, player_id: u32) -> Option<PlayerAction> {
        let valid = self.valid_actions(player_id);
        if valid.contains(&PlayerAction::Check) {
            Some(PlayerAction::Check)
        } else if valid.contains(&PlayerAction::Fold) {
            Some(PlayerAction::Fold)
        } else {
            None
        }
    }

    /// Apply one betting action: validate it, mutate chips / bets / pot / raise
    /// state, then advance the turn. Does **not** drive the post-action state
    /// machine (phase advance, hand resolution, next hand) — that's the caller's
    /// job, since it needs transport-side fanout and timers between steps.
    ///
    /// # Errors
    /// Returns [`ActionError`] when the action is illegal right now (game not
    /// started, not the player's turn, invalid action, insufficient chips, …).
    /// The transport layer surfaces the message to the player.
    pub fn apply_action(
        &mut self,
        player_id: u32,
        action: PlayerAction,
        amount: u32,
    ) -> Result<(), ActionError> {
        if !self.game_started {
            return Err(ActionError::GameNotStarted);
        }
        if self.current_player_id() != Some(player_id) {
            return Err(ActionError::NotYourTurn);
        }
        if !self.valid_actions(player_id).contains(&action) {
            return Err(ActionError::InvalidAction);
        }

        // Read the player's immutable state as Copy scalars up front, so the
        // betting arms below can mutate `self` (players / pot / raise flags)
        // without holding a shared borrow.
        let player = self
            .players
            .get(&player_id)
            .ok_or(ActionError::PlayerNotFound)?;
        let to_call = self.current_bet.saturating_sub(player.current_bet);
        let chips = player.chips;
        let prev_current_bet = player.current_bet;

        match action {
            PlayerAction::Fold => {
                if let Some(p) = self.players.get_mut(&player_id) {
                    p.status = PlayerStatus::Folded;
                }
            }
            PlayerAction::Check => {
                if to_call != 0 {
                    return Err(ActionError::CannotCheckMustCallOrRaise);
                }
                if self.phase == GamePhase::PreFlop && self.big_blind_option {
                    self.big_blind_option = false;
                    self.last_raiser_index = None;
                }
            }
            PlayerAction::Call => {
                let call_amount = to_call.min(chips);
                self.place_bet(player_id, call_amount, prev_current_bet);
            }
            PlayerAction::Raise => {
                let raise_total = to_call.saturating_add(amount);
                if raise_total > chips {
                    return Err(ActionError::NotEnoughChips {
                        have: chips,
                        need: raise_total,
                    });
                }
                // A sub-minimum raise is only legal as an all-in
                // (raise_total == chips); otherwise enforce the floor.
                if amount < self.min_raise && raise_total < chips {
                    return Err(ActionError::RaiseBelowMinimum {
                        min: self.min_raise,
                    });
                }

                let new_bet = self.place_bet(player_id, raise_total, prev_current_bet);
                let previous_current_bet = self.current_bet;
                self.current_bet = new_bet;
                // Only reopen betting (set last_raiser / bump min_raise) if this
                // raise constitutes a full legal raise. A sub-minimum all-in
                // raise must NOT, matching the AllIn arm below.
                let raise_increment = new_bet.saturating_sub(previous_current_bet);
                if raise_increment >= self.min_raise {
                    self.min_raise = raise_increment.max(self.big_blind);
                    self.last_raiser_index = Some(self.current_player_index);
                }
                self.big_blind_option = false;
            }
            PlayerAction::AllIn => {
                let all_in = chips;
                let new_bet = self.place_bet(player_id, all_in, prev_current_bet);
                if new_bet > self.current_bet {
                    // Only reopen betting if the all-in constitutes a full legal
                    // raise. An all-in never raises the min_raise floor.
                    let raise_increment = new_bet.saturating_sub(self.current_bet);
                    if raise_increment >= self.min_raise {
                        self.last_raiser_index = Some(self.current_player_index);
                    }
                    self.current_bet = new_bet;
                }
            }
        }

        self.has_acted_this_round = true;
        self.next_player();
        Ok(())
    }

    /// Move `amount` chips from `player_id`'s stack into their current bet
    /// and the pot ledger (via [`Self::add_to_pot`]), flipping them to
    /// all-in if the stack empties. Returns the player's new `current_bet`.
    ///
    /// Shared by every betting arm in [`Self::apply_action`] and by
    /// [`Self::post_blind`]. `prev_bet` is only consulted when the player is
    /// somehow absent — callers pass the bet the player had before this
    /// action so the fallback stays consistent.
    fn place_bet(&mut self, player_id: u32, amount: u32, prev_bet: u32) -> u32 {
        let new_bet = if let Some(p) = self.players.get_mut(&player_id) {
            p.chips = p.chips.saturating_sub(amount);
            p.current_bet = p.current_bet.saturating_add(amount);
            if p.chips == 0 {
                p.status = PlayerStatus::AllIn;
            }
            p.current_bet
        } else {
            prev_bet.saturating_add(amount)
        };
        self.add_to_pot(player_id, amount);
        new_bet
    }

    /// Add `amount` to the pot and record it against the player's contribution
    /// ledger (used later for side-pot calculation). Shared by every betting
    /// path via [`Self::place_bet`].
    fn add_to_pot(&mut self, player_id: u32, amount: u32) {
        self.pot = self.pot.saturating_add(amount);
        let entry = self.pot_contributions.entry(player_id).or_insert(0);
        *entry = entry.saturating_add(amount);
    }
}

#[cfg(test)]
mod tests;
