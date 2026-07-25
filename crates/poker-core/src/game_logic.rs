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
//! or serialization. The transport layer (WebSocket or SSE server) wires it
//! up to a concrete connection.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::poker::{Board, Card, Hand, get_all_cards};
use crate::protocol::{BlindConfig, CardInfo, PlayerAction, ServerMessage, Stage, card_to_info};
use rand::rng;
use rand::seq::SliceRandom;

/// Fixed per-turn timer duration in seconds.
///
/// When a player's turn begins the server starts a countdown.  If the player
/// has not acted by the time it reaches zero, the server forces a *check* (if
/// allowed) or a *fold*.
pub const TURN_TIMEOUT_SECS: u32 = 30;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// GameState
// ---------------------------------------------------------------------------

/// Server-side game state shared across all connections.
///
/// The five independent boolean flags (`game_started`, `big_blind_option`,
/// `has_acted_this_round`, `allow_late_entry`, `waiting_for_players`) are each
/// semantically distinct engine state accessed directly at ~100 sites across
/// both transports; grouping them would obscure the state machine without a
/// correctness benefit, so the bool cap is relaxed here.
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
    /// hand_description)`. Populated by [`Self::resolve_hand`], cleared by
    /// [`Self::start_new_hand`]. The UI reads it during [`GamePhase::HandOver`]
    /// to show who won how much while waiting for the next deal.
    pub last_winners: Vec<(u32, u32, String)>,
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
const fn next_seat(base: usize, offset: usize, seats: usize) -> usize {
    if seats == 0 {
        return 0;
    }
    base.rem_euclid(seats)
        .saturating_add(offset)
        .rem_euclid(seats)
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
        // deterministic promotion across transports.
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
    #[allow(
        dead_code,
        reason = "used by the poker-sse-server crate; the WS server has no equivalent call site yet"
    )]
    #[must_use]
    pub const fn is_game_over(&self) -> bool {
        // Note: no call site lives inside poker-core itself; both server crates
        // consume this from their transport layers.
        !self.game_started && matches!(self.phase, GamePhase::Lobby) && self.hand_number > 0
    }

    /// Get players who can still act (active but not all-in).
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

    /// Start a new hand.
    pub fn start_new_hand(&mut self) -> Vec<ServerMessage> {
        let mut messages = Vec::new();

        // Blind increases run on a wall-clock schedule anchored to game start.
        // Players don't see a step until the next hand (when blinds are posted).
        if self.blind_config.is_enabled()
            && let Some(mut last) = self.last_blind_increase
        {
            let interval = Duration::from_secs(self.blind_config.interval_secs);
            let pct = self.blind_config.increase_percent;
            while last.elapsed() >= interval {
                self.small_blind = self
                    .small_blind
                    .saturating_add(self.small_blind.saturating_mul(pct).div_ceil(100));
                self.big_blind = self
                    .big_blind
                    .saturating_add(self.big_blind.saturating_mul(pct).div_ceil(100));
                // Advance the anchor by exactly one interval to stay anchored
                // to game start. `checked_add` is None only near a
                // monotonically-distant future; in that case stop.
                let Some(next) = last.checked_add(interval) else {
                    break;
                };
                last = next;
                self.last_blind_increase = Some(last);
                messages.push(ServerMessage::BlindsIncreased {
                    small_blind: self.small_blind,
                    big_blind: self.big_blind,
                });
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
            return messages;
        }

        // Move dealer button
        self.dealer_index = next_seat(self.dealer_index, 1, self.player_order.len());

        // Determine blinds positions
        let sb_index = next_seat(self.dealer_index, 1, self.player_order.len());
        let bb_index = next_seat(self.dealer_index, 2, self.player_order.len());

        // The ≥ 2 player guard above guarantees these seats exist; extract them
        // without indexing so a future change to that guard can't panic here.
        let Some(dealer_id) = self.player_order.get(self.dealer_index).copied() else {
            return messages;
        };
        let Some(sb_id) = self.player_order.get(sb_index).copied() else {
            return messages;
        };
        let Some(bb_id) = self.player_order.get(bb_index).copied() else {
            return messages;
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

        messages.push(ServerMessage::NewHand {
            hand_number: self.hand_number,
            dealer_id,
            small_blind_id: sb_id,
            big_blind_id: bb_id,
            small_blind: self.small_blind,
            big_blind: self.big_blind,
        });

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

        messages
    }

    fn post_blind(&mut self, player_id: u32, amount: u32) {
        if let Some(player) = self.players.get_mut(&player_id) {
            let actual = amount.min(player.chips);
            player.chips = player.chips.saturating_sub(actual);
            player.current_bet = actual;
            self.pot = self.pot.saturating_add(actual);
            let entry = self.pot_contributions.entry(player_id).or_insert(0);
            *entry = entry.saturating_add(actual);
            if player.chips == 0 {
                player.status = PlayerStatus::AllIn;
            }
        }
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

    /// Move to next player.
    pub fn next_player(&mut self) {
        let start = self.current_player_index;
        loop {
            self.current_player_index =
                next_seat(self.current_player_index, 1, self.player_order.len());

            if let Some(player) = self
                .player_order
                .get(self.current_player_index)
                .and_then(|&id| self.players.get(&id))
                && player.status == PlayerStatus::Active
            {
                break;
            }

            if self.current_player_index == start {
                break;
            }
        }
    }

    /// Advance to next phase.
    pub fn advance_phase(&mut self) -> Vec<ServerMessage> {
        let mut messages = Vec::new();

        for player in self.players.values_mut() {
            player.current_bet = 0;
        }
        self.current_bet = 0;
        self.last_raiser_index = None;
        self.big_blind_option = false;
        self.has_acted_this_round = false;

        self.current_player_index = next_seat(self.dealer_index, 1, self.player_order.len());

        let start = self.current_player_index;
        loop {
            if let Some(player) = self
                .player_order
                .get(self.current_player_index)
                .and_then(|&id| self.players.get(&id))
                && player.status == PlayerStatus::Active
            {
                break;
            }
            self.current_player_index =
                next_seat(self.current_player_index, 1, self.player_order.len());
            if self.current_player_index == start {
                break;
            }
        }

        self.first_actor_index = Some(self.current_player_index);

        match self.phase {
            GamePhase::PreFlop => {
                self.phase = GamePhase::Flop;
                for _ in 0..3 {
                    if let Some(card) = self.deal_card() {
                        self.community_cards.push(card);
                    }
                }
                let cards: Vec<CardInfo> = self.community_cards.iter().map(card_to_info).collect();
                messages.push(ServerMessage::CommunityCards {
                    stage: Stage::Flop,
                    cards,
                });
            }
            GamePhase::Flop => {
                self.phase = GamePhase::Turn;
                if let Some(card) = self.deal_card() {
                    self.community_cards.push(card);
                }
                let cards: Vec<CardInfo> = self.community_cards.iter().map(card_to_info).collect();
                messages.push(ServerMessage::CommunityCards {
                    stage: Stage::Turn,
                    cards,
                });
            }
            GamePhase::Turn => {
                self.phase = GamePhase::River;
                if let Some(card) = self.deal_card() {
                    self.community_cards.push(card);
                }
                let cards: Vec<CardInfo> = self.community_cards.iter().map(card_to_info).collect();
                messages.push(ServerMessage::CommunityCards {
                    stage: Stage::River,
                    cards,
                });
            }
            GamePhase::River => {
                self.phase = GamePhase::Showdown;
            }
            _ => {}
        }

        messages
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
    /// Returns the winning player IDs and the hand rank description.
    fn find_pot_winners(
        hands: &[(u32, [CardInfo; 2], Hand)],
        eligible: &[u32],
        board: &Board,
    ) -> (Vec<u32>, String) {
        let eligible_hands: Vec<&(u32, [CardInfo; 2], Hand)> = hands
            .iter()
            .filter(|(id, _, _)| eligible.contains(id))
            .collect();

        if eligible_hands.len() <= 1 {
            let id = eligible_hands.first().map_or(0, |(id, _, _)| *id);
            return (vec![id], "Winner".to_string());
        }

        let mut winning_ids: Vec<u32> = Vec::new();
        let mut best_rank = String::new();

        for (id_i, _, hand_i) in &eligible_hands {
            let full_i = hand_i.best(board);
            let mut is_winner = true;

            for (id_j, _, hand_j) in &eligible_hands {
                if id_i == id_j {
                    continue;
                }
                if let (Some(fi), Some(fj)) = (&full_i, &hand_j.best(board)) {
                    use crate::poker::Winner;
                    if fi.compare(fj) == Winner::Hand2 {
                        is_winner = false;
                        break;
                    }
                }
            }

            if is_winner {
                if let Some(full) = &full_i {
                    best_rank = format!("{}", full.rank());
                }
                winning_ids.push(*id_i);
            }
        }

        (winning_ids, best_rank)
    }

    /// Split `total` chips evenly across `winners`; odd chips go to the lowest
    /// player IDs. Accumulates into `winnings` keyed by player ID. Arithmetic
    /// is saturating/checked, so this can't panic or overflow.
    fn award_split(
        winnings: &mut HashMap<u32, (u32, String)>,
        total: u32,
        winners: &[u32],
        rank: &str,
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
            let entry = winnings.entry(*id).or_insert_with(|| (0, rank.to_string()));
            entry.0 = entry.0.saturating_add(share).saturating_add(extra);
        }
    }

    /// Determine winner(s) and distribute pot using side-pot logic.
    pub fn resolve_hand(&mut self) -> Vec<ServerMessage> {
        let mut messages = Vec::new();

        let mut hands_to_show: Vec<(u32, [CardInfo; 2], Hand)> = Vec::new();

        for &id in &self.player_order {
            if let Some(player) = self.players.get(&id)
                && (player.status == PlayerStatus::Active || player.status == PlayerStatus::AllIn)
                && let Some((c1, c2)) = player.hole_cards
            {
                let cards = [card_to_info(&c1), card_to_info(&c2)];
                hands_to_show.push((id, cards, Hand(c1, c2)));
            }
        }

        if hands_to_show.is_empty() {
            return messages;
        }

        let board = self.build_board();

        // Accumulate winnings per player across all side pots.
        let mut winnings: HashMap<u32, (u32, String)> = HashMap::new();

        if hands_to_show.len() == 1 {
            // Everyone else folded — sole survivor takes the whole pot.
            let Some((id, _, _)) = hands_to_show.first() else {
                return messages;
            };
            winnings.insert(*id, (self.pot, "Winner".to_string()));
        } else {
            // Emit showdown message so clients see all hands.
            let showdown_hands: Vec<(u32, [CardInfo; 2], String)> = hands_to_show
                .iter()
                .map(|(id, cards, hand)| {
                    let rank = hand
                        .best(&board)
                        .map_or_else(|| "Unknown".to_string(), |full| format!("{}", full.rank()));
                    (*id, *cards, rank)
                })
                .collect();

            messages.push(ServerMessage::Showdown {
                hands: showdown_hands,
            });

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
            let mut last_rank = String::new();

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

                Self::award_split(&mut winnings, total, &pot_winners, &rank);

                last_winners = pot_winners;
                last_rank = rank;
            }

            // If there's still dead money left over (the highest side pot(s)
            // had no eligible winners), give it to the last known winners.
            if carry_over > 0 && !last_winners.is_empty() {
                Self::award_split(&mut winnings, carry_over, &last_winners, &last_rank);
            }
        }

        // Build the winners list and award chips.
        let mut winners: Vec<(u32, u32, String)> = winnings
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

        messages.push(ServerMessage::RoundWinner { winners });

        for player in self.players.values() {
            messages.push(ServerMessage::ChipUpdate {
                player_id: player.id,
                chips: player.chips,
            });
        }

        for player in self.players.values() {
            if player.chips == 0 && player.status != PlayerStatus::Out {
                messages.push(ServerMessage::PlayerEliminated {
                    player_id: player.id,
                });
            }
        }

        self.phase = GamePhase::HandOver;

        let remaining: Vec<&Player> = self.players.values().filter(|p| p.chips > 0).collect();

        if remaining.len() == 1
            && let Some(winner) = remaining.first()
        {
            messages.push(ServerMessage::GameOver {
                winner_id: winner.id,
                winner_name: winner.name.clone(),
            });
            self.game_started = false;
            self.phase = GamePhase::Lobby;
        }

        self.pot = 0;
        messages
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

    /// The next blind level, computed with the same `increase_percent` and
    /// `div_ceil` rounding the [`Self::start_new_hand`] bump uses. Returns the
    /// current level unchanged when rising blinds aren't configured.
    #[must_use]
    pub fn next_blinds(&self) -> (u32, u32) {
        let pct = self.blind_config.increase_percent;
        let next = |cur: u32| cur.saturating_add(cur.saturating_mul(pct).div_ceil(100));
        (next(self.small_blind), next(self.big_blind))
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
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::doc_markdown,
        clippy::type_complexity
    )]
    use super::*;
    use crate::poker::{Card, CardNumber, CardSuit};
    use crate::protocol::ServerMessage;

    fn setup_game(
        players: Vec<(u32, &str, u32, PlayerStatus, Option<(Card, Card)>, u32)>,
        community: Vec<Card>,
    ) -> GameState {
        let mut gs = GameState::new();
        gs.phase = GamePhase::Showdown;
        for (id, name, chips, status, hole_cards, contribution) in players {
            gs.players.insert(
                id,
                Player {
                    id,
                    name: name.to_string(),
                    chips,
                    status,
                    hole_cards,
                    current_bet: 0,
                    sitting_out: false,
                },
            );
            gs.player_order.push(id);
            if contribution > 0 {
                gs.pot_contributions.insert(id, contribution);
                gs.pot += contribution;
            }
        }
        gs.community_cards = community;
        gs
    }

    /// Board: T♠ J♠ Q♠ K♠ 2♦ — enables Royal Flush with A♠, Straight Flush with 9♠, Straight with any Ace.
    fn standard_board() -> Vec<Card> {
        vec![
            Card(CardNumber::Ten, CardSuit::Spades),
            Card(CardNumber::Jack, CardSuit::Spades),
            Card(CardNumber::Queen, CardSuit::Spades),
            Card(CardNumber::King, CardSuit::Spades),
            Card(CardNumber::Two, CardSuit::Diamonds),
        ]
    }

    /// Board: A♠ K♠ Q♠ J♠ T♠ — Royal Flush on board, every player ties.
    fn tie_board() -> Vec<Card> {
        vec![
            Card(CardNumber::Ace, CardSuit::Spades),
            Card(CardNumber::King, CardSuit::Spades),
            Card(CardNumber::Queen, CardSuit::Spades),
            Card(CardNumber::Jack, CardSuit::Spades),
            Card(CardNumber::Ten, CardSuit::Spades),
        ]
    }

    /// With standard_board → Royal Flush (A♠ K♠ Q♠ J♠ T♠).
    fn royal_flush_hand() -> (Card, Card) {
        (
            Card(CardNumber::Ace, CardSuit::Spades),
            Card(CardNumber::Three, CardSuit::Clubs),
        )
    }

    /// With standard_board → Straight Flush K-high (K♠ Q♠ J♠ T♠ 9♠).
    fn straight_flush_hand() -> (Card, Card) {
        (
            Card(CardNumber::Nine, CardSuit::Spades),
            Card(CardNumber::Three, CardSuit::Hearts),
        )
    }

    /// With standard_board → Ace-high Straight (A K Q J T, not flush).
    fn straight_hand() -> (Card, Card) {
        (
            Card(CardNumber::Ace, CardSuit::Hearts),
            Card(CardNumber::Three, CardSuit::Hearts),
        )
    }

    /// With standard_board → High card (K Q J T 6).
    fn low_hand() -> (Card, Card) {
        (
            Card(CardNumber::Four, CardSuit::Hearts),
            Card(CardNumber::Six, CardSuit::Clubs),
        )
    }

    /// With standard_board → High card (K Q J T 8), slightly better than low_hand.
    fn low_hand2() -> (Card, Card) {
        (
            Card(CardNumber::Seven, CardSuit::Hearts),
            Card(CardNumber::Eight, CardSuit::Clubs),
        )
    }

    fn find_round_winner(msgs: &[ServerMessage]) -> &Vec<(u32, u32, String)> {
        msgs.iter()
            .find_map(|m| match m {
                ServerMessage::RoundWinner { winners } => Some(winners),
                _ => None,
            })
            .expect("expected RoundWinner message")
    }

    fn has_showdown(msgs: &[ServerMessage]) -> bool {
        msgs.iter()
            .any(|m| matches!(m, ServerMessage::Showdown { .. }))
    }

    fn player_winnings(winners: &[(u32, u32, String)], player_id: u32) -> u32 {
        winners
            .iter()
            .filter(|(id, _, _)| *id == player_id)
            .map(|(_, amount, _)| *amount)
            .sum()
    }

    // -----------------------------------------------------------------------
    // Test 1: No side pot — single winner
    // -----------------------------------------------------------------------
    #[test]
    fn test_no_side_pot_single_winner() {
        let mut gs = setup_game(
            vec![
                (
                    1,
                    "Alice",
                    0,
                    PlayerStatus::Active,
                    Some(royal_flush_hand()),
                    100,
                ),
                (2, "Bob", 0, PlayerStatus::Active, Some(low_hand()), 100),
                (
                    3,
                    "Charlie",
                    0,
                    PlayerStatus::Active,
                    Some(low_hand2()),
                    100,
                ),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 300);

        let msgs = gs.resolve_hand();

        assert!(has_showdown(&msgs));
        let winners = find_round_winner(&msgs);
        assert_eq!(player_winnings(winners, 1), 300);
        assert_eq!(gs.players.get(&1).unwrap().chips, 300);
        assert_eq!(gs.players.get(&2).unwrap().chips, 0);
        assert_eq!(gs.players.get(&3).unwrap().chips, 0);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 2: Short-stack all-in wins the main pot
    // -----------------------------------------------------------------------
    #[test]
    fn test_short_stack_allin_wins_main_pot() {
        // A(AllIn,50) best hand, B(Active,100) second-best, C(Active,100) worst.
        // Main pot  = 50×3 = 150 → A
        // Side pot  = 50×2 = 100 → B
        let mut gs = setup_game(
            vec![
                (
                    1,
                    "Alice",
                    0,
                    PlayerStatus::AllIn,
                    Some(royal_flush_hand()),
                    50,
                ),
                (
                    2,
                    "Bob",
                    0,
                    PlayerStatus::Active,
                    Some(straight_flush_hand()),
                    100,
                ),
                (3, "Charlie", 0, PlayerStatus::Active, Some(low_hand()), 100),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 250);

        let msgs = gs.resolve_hand();

        assert!(has_showdown(&msgs));
        let winners = find_round_winner(&msgs);
        assert_eq!(player_winnings(winners, 1), 150);
        assert_eq!(player_winnings(winners, 2), 100);
        assert_eq!(gs.players.get(&1).unwrap().chips, 150);
        assert_eq!(gs.players.get(&2).unwrap().chips, 100);
        assert_eq!(gs.players.get(&3).unwrap().chips, 0);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 3: Active player wins everything (main + side)
    // -----------------------------------------------------------------------
    #[test]
    fn test_active_player_wins_all_pots() {
        // B has the best hand → sweeps both pots for 250.
        let mut gs = setup_game(
            vec![
                (1, "Alice", 0, PlayerStatus::AllIn, Some(low_hand()), 50),
                (
                    2,
                    "Bob",
                    0,
                    PlayerStatus::Active,
                    Some(royal_flush_hand()),
                    100,
                ),
                (
                    3,
                    "Charlie",
                    0,
                    PlayerStatus::Active,
                    Some(low_hand2()),
                    100,
                ),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 250);

        let msgs = gs.resolve_hand();

        assert!(has_showdown(&msgs));
        let winners = find_round_winner(&msgs);
        assert_eq!(player_winnings(winners, 2), 250);
        assert_eq!(gs.players.get(&1).unwrap().chips, 0);
        assert_eq!(gs.players.get(&2).unwrap().chips, 250);
        assert_eq!(gs.players.get(&3).unwrap().chips, 0);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 4: Two all-ins at different levels → three tiers
    // -----------------------------------------------------------------------
    #[test]
    fn test_two_allin_different_levels() {
        // A(AllIn,25) Royal Flush, B(AllIn,50) SF K-high,
        // C(Active,100) Ace-high straight, D(Active,100) high card.
        // Tier 1: 25×4 = 100 → A   (best overall)
        // Tier 2: 25×3 =  75 → B   (best among B,C,D)
        // Tier 3: 50×2 = 100 → C   (best among C,D)
        let mut gs = setup_game(
            vec![
                (
                    1,
                    "Alice",
                    0,
                    PlayerStatus::AllIn,
                    Some(royal_flush_hand()),
                    25,
                ),
                (
                    2,
                    "Bob",
                    0,
                    PlayerStatus::AllIn,
                    Some(straight_flush_hand()),
                    50,
                ),
                (
                    3,
                    "Charlie",
                    0,
                    PlayerStatus::Active,
                    Some(straight_hand()),
                    100,
                ),
                (4, "Diana", 0, PlayerStatus::Active, Some(low_hand()), 100),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 275);

        let msgs = gs.resolve_hand();

        assert!(has_showdown(&msgs));
        let winners = find_round_winner(&msgs);
        assert_eq!(player_winnings(winners, 1), 100);
        assert_eq!(player_winnings(winners, 2), 75);
        assert_eq!(player_winnings(winners, 3), 100);
        assert_eq!(gs.players.get(&1).unwrap().chips, 100);
        assert_eq!(gs.players.get(&2).unwrap().chips, 75);
        assert_eq!(gs.players.get(&3).unwrap().chips, 100);
        assert_eq!(gs.players.get(&4).unwrap().chips, 0);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 5: Folded player's money stays in pot, not eligible to win
    // -----------------------------------------------------------------------
    #[test]
    fn test_folded_player_money_in_pot() {
        // A(Folded,100), B(Active,100) best hand, C(Active,100) worst.
        // Single tier pot = 300. A ineligible → B wins all 300.
        let mut gs = setup_game(
            vec![
                (1, "Alice", 0, PlayerStatus::Folded, None, 100),
                (
                    2,
                    "Bob",
                    0,
                    PlayerStatus::Active,
                    Some(royal_flush_hand()),
                    100,
                ),
                (3, "Charlie", 0, PlayerStatus::Active, Some(low_hand()), 100),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 300);

        let msgs = gs.resolve_hand();

        assert!(has_showdown(&msgs));
        let winners = find_round_winner(&msgs);
        assert_eq!(player_winnings(winners, 2), 300);
        assert_eq!(gs.players.get(&1).unwrap().chips, 0);
        assert_eq!(gs.players.get(&2).unwrap().chips, 300);
        assert_eq!(gs.players.get(&3).unwrap().chips, 0);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 6: Dead money from multiple folded players → solo active wins all
    // -----------------------------------------------------------------------
    #[test]
    fn test_dead_money_from_folded_players() {
        // A(Folded,30), B(Folded,30), C(Active,100). Only C eligible.
        // Pot = 160. C is the sole active player → solo-survivor path.
        let mut gs = setup_game(
            vec![
                (1, "Alice", 0, PlayerStatus::Folded, None, 30),
                (2, "Bob", 0, PlayerStatus::Folded, None, 30),
                (
                    3,
                    "Charlie",
                    0,
                    PlayerStatus::Active,
                    Some(royal_flush_hand()),
                    100,
                ),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 160);

        let msgs = gs.resolve_hand();

        assert!(
            !has_showdown(&msgs),
            "solo active player should not trigger showdown"
        );
        let winners = find_round_winner(&msgs);
        assert_eq!(player_winnings(winners, 3), 160);
        assert_eq!(gs.players.get(&3).unwrap().chips, 160);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 7: Split pot — perfect tie (Royal Flush on board)
    // -----------------------------------------------------------------------
    #[test]
    fn test_split_pot_tie() {
        let mut gs = setup_game(
            vec![
                (1, "Alice", 0, PlayerStatus::Active, Some(low_hand()), 100),
                (2, "Bob", 0, PlayerStatus::Active, Some(low_hand2()), 100),
            ],
            tie_board(),
        );
        assert_eq!(gs.pot, 200);

        let msgs = gs.resolve_hand();

        assert!(has_showdown(&msgs));
        let winners = find_round_winner(&msgs);
        assert_eq!(player_winnings(winners, 1), 100);
        assert_eq!(player_winnings(winners, 2), 100);
        assert_eq!(gs.players.get(&1).unwrap().chips, 100);
        assert_eq!(gs.players.get(&2).unwrap().chips, 100);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 8: Odd chip in split → lower player_id gets the extra chip
    // -----------------------------------------------------------------------
    #[test]
    fn test_odd_chip_split() {
        // P1(Active,100) and P2(Active,100) tie on tie_board.
        // P3(Folded,1) adds 1 dead chip → pot = 201.
        // Tier 1 (level 1): 1×3 = 3, eligible P1 & P2, tie → P1 gets 2, P2 gets 1.
        // Tier 2 (level 100): 99×2 = 198, eligible P1 & P2, tie → 99 each.
        // Totals: P1 = 101, P2 = 100.
        let mut gs = setup_game(
            vec![
                (1, "Alice", 0, PlayerStatus::Active, Some(low_hand()), 100),
                (2, "Bob", 0, PlayerStatus::Active, Some(low_hand2()), 100),
                (3, "Charlie", 0, PlayerStatus::Folded, None, 1),
            ],
            tie_board(),
        );
        assert_eq!(gs.pot, 201);

        let msgs = gs.resolve_hand();

        let _winners = find_round_winner(&msgs);
        let p1_chips = gs.players.get(&1).unwrap().chips;
        let p2_chips = gs.players.get(&2).unwrap().chips;
        assert_eq!(p1_chips + p2_chips, 201, "all chips must be distributed");
        assert!(
            p1_chips >= p2_chips,
            "lower player_id should get the odd chip"
        );
        assert_eq!(p1_chips, 101);
        assert_eq!(p2_chips, 100);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 9: Solo survivor — everyone else folded, no showdown
    // -----------------------------------------------------------------------
    #[test]
    fn test_solo_survivor_everyone_folded() {
        let mut gs = setup_game(
            vec![
                (
                    1,
                    "Alice",
                    0,
                    PlayerStatus::Active,
                    Some(royal_flush_hand()),
                    100,
                ),
                (2, "Bob", 0, PlayerStatus::Folded, None, 100),
                (3, "Charlie", 0, PlayerStatus::Folded, None, 50),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 250);

        let msgs = gs.resolve_hand();

        assert!(
            !has_showdown(&msgs),
            "solo survivor should not trigger showdown"
        );
        let winners = find_round_winner(&msgs);
        assert_eq!(player_winnings(winners, 1), 250);
        assert_eq!(gs.players.get(&1).unwrap().chips, 250);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 10: All-in wins main pot, different active player wins side pot
    // -----------------------------------------------------------------------
    #[test]
    fn test_allin_wins_main_active_wins_side() {
        // A(AllIn,50) Royal Flush, B(Active,100) Ace-high straight, C(Active,100) high card.
        // Main pot = 50×3 = 150 → A
        // Side pot = 50×2 = 100 → B
        let mut gs = setup_game(
            vec![
                (
                    1,
                    "Alice",
                    0,
                    PlayerStatus::AllIn,
                    Some(royal_flush_hand()),
                    50,
                ),
                (
                    2,
                    "Bob",
                    0,
                    PlayerStatus::Active,
                    Some(straight_hand()),
                    100,
                ),
                (3, "Charlie", 0, PlayerStatus::Active, Some(low_hand()), 100),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 250);

        let msgs = gs.resolve_hand();

        assert!(has_showdown(&msgs));
        let winners = find_round_winner(&msgs);
        assert_eq!(player_winnings(winners, 1), 150);
        assert_eq!(player_winnings(winners, 2), 100);
        assert_eq!(gs.players.get(&1).unwrap().chips, 150);
        assert_eq!(gs.players.get(&2).unwrap().chips, 100);
        assert_eq!(gs.players.get(&3).unwrap().chips, 0);
        assert_eq!(gs.pot, 0);
    }

    // -----------------------------------------------------------------------
    // Test 11: Multi-player hand must NOT end the game (regression)
    // -----------------------------------------------------------------------
    #[test]
    fn test_resolve_hand_does_not_end_game_with_multiple_survivors() {
        // Three players still hold chips. Resolving the pot is just "hand
        // over", not "game over" — the engine must keep the session alive so
        // the next hand can be dealt. Regression for the bug where
        // `remaining.first()` being `Some` (true after any hand) was taken as
        // the game-over signal, which tore the game down mid-match.
        let mut gs = setup_game(
            vec![
                (
                    1,
                    "Alice",
                    200,
                    PlayerStatus::Active,
                    Some(royal_flush_hand()),
                    100,
                ),
                (2, "Bob", 100, PlayerStatus::Active, Some(low_hand()), 100),
                (3, "Charlie", 50, PlayerStatus::Folded, None, 0),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 200);
        // Simulate an in-progress game so is_game_over's hand_number guard is met.
        gs.game_started = true;
        gs.hand_number = 5;

        let msgs = gs.resolve_hand();

        // Alice wins the pot; Bob and Charlie still hold chips → not game over.
        assert!(
            msgs.iter()
                .all(|m| !matches!(m, ServerMessage::GameOver { .. })),
            "GameOver must not fire while more than one player has chips"
        );
        assert!(gs.game_started, "game_started must stay true mid-game");
        assert!(!gs.is_game_over());
    }

    // -----------------------------------------------------------------------
    // Test 12: Game ends only when exactly one player has chips
    // -----------------------------------------------------------------------
    #[test]
    fn test_resolve_hand_ends_game_with_single_survivor() {
        // Everyone except one player is at zero chips — genuine game over.
        let mut gs = setup_game(
            vec![
                (
                    1,
                    "Alice",
                    0,
                    PlayerStatus::Active,
                    Some(royal_flush_hand()),
                    100,
                ),
                (2, "Bob", 0, PlayerStatus::Active, Some(low_hand()), 0),
            ],
            standard_board(),
        );
        assert_eq!(gs.pot, 100);
        gs.game_started = true;
        gs.hand_number = 5;

        let msgs = gs.resolve_hand();

        let winner = msgs
            .iter()
            .find_map(|m| match m {
                ServerMessage::GameOver { winner_id, .. } => Some(*winner_id),
                _ => None,
            })
            .expect("GameOver should fire when exactly one player has chips");
        assert_eq!(winner, 1);
        assert!(
            !gs.game_started,
            "game_started must be cleared on game over"
        );
        assert!(gs.is_game_over());
    }

    /// Count `BlindsIncreased` events in a message slice.
    fn count_blind_increases(msgs: &[ServerMessage]) -> usize {
        msgs.iter()
            .filter(|m| matches!(m, ServerMessage::BlindsIncreased { .. }))
            .count()
    }

    /// Latest big blind from a message slice (0 if none).
    fn latest_big_blind(msgs: &[ServerMessage]) -> u32 {
        msgs.iter()
            .rev()
            .find_map(|m| match m {
                ServerMessage::BlindsIncreased { big_blind, .. } => Some(*big_blind),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// Blinds don't increase on a fresh `start_new_hand` (no anchor set yet —
    /// the anchor is established on game start, not on construction).
    #[test]
    fn test_blinds_no_increase_without_anchor() {
        let mut gs = GameState::new();
        gs.blind_config = BlindConfig {
            interval_secs: 1,
            increase_percent: 50,
        };
        // last_blind_increase is None → no increase possible.
        let msgs = gs.start_new_hand();
        assert_eq!(count_blind_increases(&msgs), 0);
    }

    /// Blinds don't increase before the interval has elapsed.
    #[test]
    fn test_blinds_no_increase_within_interval() {
        let mut gs = GameState::new();
        gs.blind_config = BlindConfig {
            interval_secs: 60,
            increase_percent: 50,
        };
        gs.last_blind_increase = Some(Instant::now());
        let msgs = gs.start_new_hand();
        assert_eq!(count_blind_increases(&msgs), 0);
    }

    /// One interval elapsed → exactly one increase.
    #[test]
    fn test_blinds_single_increase_after_one_interval() {
        let mut gs = GameState::new();
        gs.blind_config = BlindConfig {
            interval_secs: 1,
            increase_percent: 50,
        };
        // Anchor 1s in the past so exactly one interval has elapsed.
        gs.last_blind_increase = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .map(Some)
            .unwrap();
        let msgs = gs.start_new_hand();
        assert_eq!(count_blind_increases(&msgs), 1);
        // 20 + ceil(20*50/100) = 30.
        assert_eq!(latest_big_blind(&msgs), 30);
    }

    /// A gap spanning three intervals must apply THREE increases (catch-up),
    /// not one — the anchor advances by one interval per step rather than
    /// resetting to `Instant::now()`.
    #[test]
    fn test_blinds_catch_up_multiple_missed_levels() {
        let mut gs = GameState::new();
        gs.blind_config = BlindConfig {
            interval_secs: 1,
            increase_percent: 100, // doubles each step for easy math
        };
        // Anchor 3s in the past: three intervals have elapsed.
        gs.last_blind_increase = Instant::now()
            .checked_sub(Duration::from_secs(3))
            .map(Some)
            .unwrap();
        let msgs = gs.start_new_hand();
        assert_eq!(
            count_blind_increases(&msgs),
            3,
            "a 3-interval gap must catch up all three levels"
        );
        // 20 → 40 → 80 → 160.
        assert_eq!(latest_big_blind(&msgs), 160);
    }

    /// After catch-up the anchor sits on the interval boundary (not "now"), so
    /// one more interval steps exactly once more.
    #[test]
    fn test_blinds_anchor_advances_by_interval_not_now() {
        let mut gs = GameState::new();
        gs.blind_config = BlindConfig {
            interval_secs: 1,
            increase_percent: 100,
        };
        // Anchor 2s in the past.
        gs.last_blind_increase = Instant::now()
            .checked_sub(Duration::from_secs(2))
            .map(Some)
            .unwrap();
        // First hand: catches up two levels → 20 → 40 → 80. Anchor now at
        // (original + 2s), i.e. ~now.
        let _ = gs.start_new_hand();
        assert_eq!(gs.big_blind, 80);

        // Simulate one more interval passing before the next hand.
        gs.last_blind_increase = gs
            .last_blind_increase
            .and_then(|t| t.checked_sub(Duration::from_secs(1)));
        let msgs = gs.start_new_hand();
        assert_eq!(
            count_blind_increases(&msgs),
            1,
            "exactly one step after one more interval"
        );
        assert_eq!(latest_big_blind(&msgs), 160);
    }

    /// Host removed → host rights go to the lowest-id remaining player.
    #[test]
    fn test_promote_next_host_promotes_lowest_remaining() {
        let mut gs = GameState::new();
        let host = gs.add_player("host".into()).id; // id 1
        let p2 = gs.add_player("two".into()).id; // id 2
        let p3 = gs.add_player("three".into()).id; // id 3
        gs.host_id = host;

        gs.remove_player(host);
        let promoted = gs.promote_next_host(host);
        assert_eq!(promoted, Some(p2));
        assert_eq!(gs.host_id, p2);
        // p3 remains present.
        assert!(gs.players.contains_key(&p3));
    }

    /// Removing a non-host player leaves `host_id` untouched.
    #[test]
    fn test_promote_next_host_noop_for_non_host() {
        let mut gs = GameState::new();
        let host = gs.add_player("host".into()).id;
        let other = gs.add_player("other".into()).id;
        gs.host_id = host;

        gs.remove_player(other);
        assert_eq!(gs.promote_next_host(other), None);
        assert_eq!(gs.host_id, host);
    }

    /// Last player removed → no one to promote, returns `None`.
    #[test]
    fn test_promote_next_host_none_when_empty() {
        let mut gs = GameState::new();
        let host = gs.add_player("host".into()).id;
        gs.host_id = host;

        gs.remove_player(host);
        assert_eq!(gs.promote_next_host(host), None);
        assert!(gs.player_order.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 13: A resolved hand lands in HandOver (not stuck on River) and
    // records its winners; the next deal clears both.
    // -----------------------------------------------------------------------
    #[test]
    fn test_resolve_hand_lands_in_handover_with_winners() {
        // Three players still in (genuine showdown), two will hold chips after
        // the pot is awarded → not game over. Pre-resolve, simulate a river
        // check-down by starting from the Showdown-ish setup_game helper.
        let mut gs = setup_game(
            vec![
                (
                    1,
                    "Alice",
                    200,
                    PlayerStatus::Active,
                    Some(royal_flush_hand()),
                    100,
                ),
                (2, "Bob", 100, PlayerStatus::Active, Some(low_hand()), 100),
                (3, "Charlie", 50, PlayerStatus::Folded, None, 0),
            ],
            standard_board(),
        );
        gs.game_started = true;
        gs.hand_number = 5;
        gs.phase = GamePhase::River;

        let msgs = gs.resolve_hand();

        // Phase must move off River to HandOver — staying on River is the bug
        // that lit up a phantom turn/timer/action bar during the pre-deal wait.
        assert_eq!(gs.phase, GamePhase::HandOver, "phase must be HandOver after resolve");
        assert!(
            msgs.iter()
                .all(|m| !matches!(m, ServerMessage::GameOver { .. })),
            "not game over — two players still hold chips"
        );
        // Alice (royal flush) wins the 200 pot.
        assert_eq!(gs.last_winners.len(), 1);
        let (wid, amount, _rank) = gs
            .last_winners
            .get(0)
            .expect("winner recorded");
        assert_eq!(*wid, 1);
        assert_eq!(*amount, 200);
    }

    // -----------------------------------------------------------------------
    // Test 14: start_new_hand clears the HandOver phase and last_winners.
    // -----------------------------------------------------------------------
    #[test]
    fn test_start_new_hand_clears_handover() {
        let mut gs = setup_game(
            vec![
                (
                    1,
                    "Alice",
                    200,
                    PlayerStatus::Active,
                    Some(royal_flush_hand()),
                    100,
                ),
                (2, "Bob", 100, PlayerStatus::Active, Some(low_hand()), 100),
            ],
            standard_board(),
        );
        gs.game_started = true;
        gs.hand_number = 5;
        gs.phase = GamePhase::River;

        let _ = gs.resolve_hand();
        assert_eq!(gs.phase, GamePhase::HandOver);
        assert!(!gs.last_winners.is_empty());

        let _ = gs.start_new_hand();
        assert_eq!(gs.phase, GamePhase::PreFlop, "new deal must clear HandOver");
        assert!(
            gs.last_winners.is_empty(),
            "last_winners must be cleared by the next deal"
        );
    }
}
