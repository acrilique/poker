//! Server-side game logic: types, state management, and betting rules.
//!
//! This module is transport-agnostic — it knows nothing about TCP, channels,
//! or serialization.  The [`server`](crate::server) module wires it up to a
//! concrete transport.

use std::collections::HashMap;
use std::time::Instant;

use poker_core::poker::{Board, Card, Hand, get_all_cards};
use poker_core::protocol::{BlindConfig, CardInfo, PlayerAction, ServerMessage, card_to_info};
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
}

// ---------------------------------------------------------------------------
// GameState
// ---------------------------------------------------------------------------

/// Server-side game state shared across all connections.
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
        }
    }
}

impl GameState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_player(&mut self, name: String) -> Player {
        self.add_player_with_chips(name, None)
    }

    /// Add a player with an optional chip override (used for late entries).
    pub fn add_player_with_chips(&mut self, name: String, chips_override: Option<u32>) -> Player {
        let bb = if self.starting_big_blind > 0 { self.starting_big_blind } else { self.big_blind };
        let starting_chips = chips_override.unwrap_or(self.starting_bbs * bb);
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
        self.next_player_id += 1;
        player
    }

    pub fn remove_player(&mut self, id: u32) {
        self.players.remove(&id);
        self.player_order.retain(|&pid| pid != id);
    }

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
    pub fn is_current_player_sitting_out(&self) -> bool {
        self.current_player_id()
            .and_then(|id| self.players.get(&id))
            .map(|p| p.sitting_out)
            .unwrap_or(false)
    }

    /// Get active players (not folded, not out).
    pub fn active_player_count(&self) -> usize {
        self.players
            .values()
            .filter(|p| p.status == PlayerStatus::Active || p.status == PlayerStatus::AllIn)
            .count()
    }

    /// Get players who can still act (active but not all-in).
    pub fn actionable_players(&self) -> Vec<u32> {
        self.player_order
            .iter()
            .filter(|&&id| {
                self.players
                    .get(&id)
                    .map(|p| p.status == PlayerStatus::Active)
                    .unwrap_or(false)
            })
            .copied()
            .collect()
    }

    /// Shuffle and create a new deck.
    pub fn new_deck(&mut self) {
        self.deck = get_all_cards();
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

        // Check if blinds should increase.
        if self.blind_config.is_enabled() {
            let should_increase = match self.last_blind_increase {
                Some(last) => last.elapsed().as_secs() >= self.blind_config.interval_secs,
                None => false, // first hand — initialised on game start
            };
            if should_increase {
                let pct = self.blind_config.increase_percent;
                self.small_blind = self.small_blind + (self.small_blind * pct).div_ceil(100);
                self.big_blind = self.big_blind + (self.big_blind * pct).div_ceil(100);
                self.last_blind_increase = Some(Instant::now());
                messages.push(ServerMessage::BlindsIncreased {
                    small_blind: self.small_blind,
                    big_blind: self.big_blind,
                });
            }
        }

        self.hand_number += 1;
        self.phase = GamePhase::PreFlop;
        self.pot = 0;
        self.pot_contributions.clear();
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
            .retain(|&id| self.players.get(&id).map(|p| p.chips > 0).unwrap_or(false));

        if self.player_order.len() < 2 {
            return messages;
        }

        // Move dealer button
        self.dealer_index = (self.dealer_index + 1) % self.player_order.len();

        // Determine blinds positions
        let sb_index = (self.dealer_index + 1) % self.player_order.len();
        let bb_index = (self.dealer_index + 2) % self.player_order.len();

        let dealer_id = self.player_order[self.dealer_index];
        let sb_id = self.player_order[sb_index];
        let bb_id = self.player_order[bb_index];

        // Post blinds
        self.post_blind(sb_id, self.small_blind);
        self.post_blind(bb_id, self.big_blind);
        self.current_bet = self.big_blind;
        self.min_raise = self.big_blind;

        // Action starts after big blind
        self.current_player_index = (bb_index + 1) % self.player_order.len();
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
                self.players
                    .get(&id)
                    .map(|p| p.status == PlayerStatus::Active || p.status == PlayerStatus::AllIn)
                    .unwrap_or(false)
            })
            .copied()
            .collect();

        for player_id in players_to_deal {
            let c1 = self.deal_card().unwrap();
            let c2 = self.deal_card().unwrap();
            if let Some(player) = self.players.get_mut(&player_id) {
                player.hole_cards = Some((c1, c2));
            }
        }

        messages
    }

    fn post_blind(&mut self, player_id: u32, amount: u32) {
        if let Some(player) = self.players.get_mut(&player_id) {
            let actual = amount.min(player.chips);
            player.chips -= actual;
            player.current_bet = actual;
            self.pot += actual;
            *self.pot_contributions.entry(player_id).or_insert(0) += actual;
            if player.chips == 0 {
                player.status = PlayerStatus::AllIn;
            }
        }
    }

    /// Get the current player's ID.
    pub fn current_player_id(&self) -> Option<u32> {
        self.player_order.get(self.current_player_index).copied()
    }

    /// Check if betting round is complete.
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
                .map(|p| p.status == PlayerStatus::Active)
                .unwrap_or(false);

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
                    .map(|p| p.status == PlayerStatus::Active)
                    .unwrap_or(false)
                {
                    break;
                }
                sentinel = (sentinel + 1) % self.player_order.len();
            }
            return self.current_player_index == sentinel;
        }

        true
    }

    /// Move to next player.
    pub fn next_player(&mut self) {
        let start = self.current_player_index;
        loop {
            self.current_player_index = (self.current_player_index + 1) % self.player_order.len();

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

        self.current_player_index = (self.dealer_index + 1) % self.player_order.len();

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
            self.current_player_index = (self.current_player_index + 1) % self.player_order.len();
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
                    stage: "flop".to_string(),
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
                    stage: "turn".to_string(),
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
                    stage: "river".to_string(),
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
        levels.sort();
        levels.dedup();

        let mut side_pots: Vec<(u32, Vec<u32>)> = Vec::new();
        let mut prev_level = 0u32;

        for &level in &levels {
            let layer = level - prev_level;
            if layer == 0 {
                continue;
            }

            // How many players contributed at least `level` chips total?
            let contributors: usize = contributions.iter().filter(|(_, a)| *a >= level).count();

            let pot_amount = layer * u32::try_from(contributors).unwrap();

            // Only non-folded players who contributed at least `level` are
            // eligible to *win* this sub-pot.
            let eligible: Vec<u32> = contributions
                .iter()
                .filter(|(id, a)| {
                    *a >= level
                        && self
                            .players
                            .get(id)
                            .map(|p| {
                                p.status == PlayerStatus::Active || p.status == PlayerStatus::AllIn
                            })
                            .unwrap_or(false)
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
            let id = eligible_hands.first().map(|(id, _, _)| *id).unwrap_or(0);
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
                    use poker_core::poker::Winner;
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
            let (id, _, _) = &hands_to_show[0];
            winnings.insert(*id, (self.pot, "Winner".to_string()));
        } else {
            // Emit showdown message so clients see all hands.
            let showdown_hands: Vec<(u32, [CardInfo; 2], String)> = hands_to_show
                .iter()
                .map(|(id, cards, hand)| {
                    let rank = if let Some(full) = hand.best(&board) {
                        format!("{}", full.rank())
                    } else {
                        "Unknown".to_string()
                    };
                    (*id, *cards, rank)
                })
                .collect();

            messages.push(ServerMessage::Showdown {
                hands: showdown_hands,
            });

            // Calculate and award each side pot independently.
            let side_pots = self.calculate_side_pots();

            for (pot_amount, eligible) in &side_pots {
                if eligible.is_empty() {
                    // All contributors at this tier folded. Distribute evenly
                    // among the remaining (lower-tier) winners. In practice
                    // this shouldn't happen because at least one non-folded
                    // player will be eligible.
                    continue;
                }

                let (pot_winners, rank) = Self::find_pot_winners(&hands_to_show, eligible, &board);

                if pot_winners.is_empty() {
                    continue;
                }

                let num_winners = u32::try_from(pot_winners.len()).unwrap();
                let share = pot_amount / num_winners;
                let remainder = pot_amount % num_winners;
                for (i, id) in pot_winners.iter().enumerate() {
                    let entry = winnings.entry(*id).or_insert((0, rank.clone()));
                    entry.0 += share
                        + if u32::try_from(i).unwrap() < remainder {
                            1
                        } else {
                            0
                        };
                }
            }
        }

        // Build the winners list and award chips.
        let mut winners: Vec<(u32, u32, String)> = winnings
            .into_iter()
            .map(|(id, (amount, rank))| (id, amount, rank))
            .collect();
        // Sort by player ID for deterministic ordering.
        winners.sort_by_key(|(id, _, _)| *id);

        for (winner_id, amount, _) in &winners {
            if let Some(player) = self.players.get_mut(winner_id) {
                player.chips += amount;
            }
        }

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

        let remaining: Vec<&Player> = self.players.values().filter(|p| p.chips > 0).collect();

        if remaining.len() == 1 {
            messages.push(ServerMessage::GameOver {
                winner_id: remaining[0].id,
                winner_name: remaining[0].name.clone(),
            });
            self.game_started = false;
            self.phase = GamePhase::Lobby;
        }

        self.pot = 0;
        messages
    }

    /// Build a [`Board`] from the current community cards.
    pub fn build_board(&self) -> Board {
        let flop = if self.community_cards.len() >= 3 {
            Some((
                self.community_cards[0],
                self.community_cards[1],
                self.community_cards[2],
            ))
        } else {
            None
        };

        let turn = self.community_cards.get(3).copied();
        let river = self.community_cards.get(4).copied();

        Board { flop, turn, river }
    }

    /// Get valid actions for current player.
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
