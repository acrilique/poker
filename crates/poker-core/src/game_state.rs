use std::collections::{HashMap, HashSet, VecDeque};

use crate::poker::{Board, Hand, HandRank};
use crate::protocol::{
    BlindConfig, CardInfo, ClientMessage, PlayerAction, PlayerInfo, ServerMessage,
};

/// A revealed hand during showdown, for direct UI display.
#[derive(Debug, Clone)]
pub struct ShowdownHand {
    pub player_id: u32,
    pub name: String,
    pub cards: [CardInfo; 2],
    /// Hand rank description (e.g. "Full House"). Present on river showdown.
    pub hand_rank: Option<String>,
    /// Win+tie equity percentage (0–100). Present on all-in showdown.
    pub equity: Option<f64>,
}

/// Semantic category for log/event messages. The UI layer decides how to style each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogCategory {
    System,
    Chat,
    Action,
    Winner,
    Error,
    Info,
}

/// A structured game event. Frontends render these however they see fit —
/// TUI uses coloured text, a GUI might use card widgets and animations.
#[derive(Debug, Clone)]
pub enum GameEvent {
    /// Welcome message from server.
    Welcome { message: String },
    /// We successfully joined the game.
    Joined {
        player_id: u32,
        chips: u32,
        player_count: usize,
    },
    /// Another player joined.
    PlayerJoined { player_id: u32, name: String },
    /// A player left.
    PlayerLeft { player_id: u32, name: String },
    /// Chat message from a player.
    Chat {
        player_id: u32,
        player_name: String,
        message: String,
    },
    /// The game started.
    GameStarted,
    /// A new hand is starting.
    NewHand {
        hand_number: u32,
        dealer_id: u32,
        small_blind_id: u32,
        big_blind_id: u32,
        small_blind: u32,
        big_blind: u32,
    },
    /// Our hole cards were dealt.
    HoleCards { cards: [CardInfo; 2] },
    /// Community cards revealed.
    CommunityCards { stage: String, cards: Vec<CardInfo> },
    /// It's our turn to act.
    YourTurn,
    /// A player performed an action.
    PlayerActed {
        player_id: u32,
        name: String,
        action: PlayerAction,
        amount: Option<u32>,
    },
    /// Showdown — reveal hands.
    Showdown {
        hands: Vec<(u32, String, [CardInfo; 2], String)>,
    },
    /// All-in showdown with equity.
    AllInShowdown {
        hands: Vec<(u32, String, [CardInfo; 2], f64)>,
    },
    /// A player won the round.
    RoundWinner {
        player_id: u32,
        name: String,
        amount: u32,
        hand: String,
    },
    /// A player was eliminated (out of chips).
    PlayerEliminated { player_id: u32, name: String },
    /// The game is over.
    GameOver { winner_id: u32, winner_name: String },
    /// Pong response.
    Pong,
    /// Error from the server.
    ServerError { message: String },
    /// Server disconnected.
    Disconnected,
    /// Generic text message (used by the UI layer for local feedback).
    Text { text: String, category: LogCategory },
    /// Blinds increased at the start of a new level.
    BlindsIncreased { small_blind: u32, big_blind: u32 },
    /// A player's turn timer has started (broadcast to all).
    TurnTimerStarted {
        player_id: u32,
        name: String,
        timeout_secs: u32,
    },
    /// A player sat out.
    PlayerSatOut { player_id: u32, name: String },
    /// A player sat back in.
    PlayerSatIn { player_id: u32, name: String },
}

impl GameEvent {
    /// Semantic category for styling purposes.
    pub fn category(&self) -> LogCategory {
        match self {
            Self::Welcome { .. }
            | Self::Joined { .. }
            | Self::GameStarted
            | Self::NewHand { .. }
            | Self::YourTurn => LogCategory::System,

            Self::Chat { .. } => LogCategory::Chat,
            Self::PlayerActed { .. } => LogCategory::Action,

            Self::Showdown { .. }
            | Self::AllInShowdown { .. }
            | Self::RoundWinner { .. }
            | Self::GameOver { .. } => LogCategory::Winner,

            Self::ServerError { .. } | Self::Disconnected => LogCategory::Error,

            Self::PlayerJoined { .. }
            | Self::PlayerLeft { .. }
            | Self::HoleCards { .. }
            | Self::CommunityCards { .. }
            | Self::PlayerEliminated { .. }
            | Self::Pong => LogCategory::Info,

            Self::Text { category, .. } => *category,
            Self::BlindsIncreased { .. } => LogCategory::System,
            Self::TurnTimerStarted { .. } => LogCategory::System,
            Self::PlayerSatOut { .. } => LogCategory::Info,
            Self::PlayerSatIn { .. } => LogCategory::Info,
        }
    }
}

/// Describes what changed in the game state after applying a server message.
///
/// Frontends can inspect these flags to decide what to re-render, animate,
/// or recalculate. All flags default to `false`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StateChanged {
    /// The set of valid player actions changed (new turn, game start/end).
    pub actions: bool,
    /// The player list changed (join, leave, chip update).
    pub players: bool,
    /// Hole cards or community cards changed.
    pub cards: bool,
    /// The pot amount changed.
    pub pot: bool,
    /// The game phase/stage changed (new hand, flop, turn, river, showdown).
    pub phase: bool,
    /// The turn timer changed (started for a new player).
    pub timer: bool,
}

impl StateChanged {
    /// Returns `true` if any flag is set.
    pub fn any(self) -> bool {
        self.actions || self.players || self.cards || self.pot || self.phase || self.timer
    }
}

/// Contains all poker game data the client tracks.
#[derive(Clone)]
pub struct ClientGameState {
    /// Structured game events (replaces formatted log messages).
    pub events: VecDeque<GameEvent>,
    /// List of players
    pub players: Vec<PlayerInfo>,
    /// Our hole cards
    pub hole_cards: Option<[CardInfo; 2]>,
    /// Community cards
    pub community_cards: Vec<CardInfo>,
    /// Current pot
    pub pot: u32,
    /// Current hand number
    pub hand_number: u32,
    /// Is it our turn?
    pub is_our_turn: bool,
    /// Valid actions (when it's our turn)
    pub valid_actions: Vec<PlayerAction>,
    /// Current bet to call
    pub current_bet: u32,
    /// Our current bet this round
    pub our_bet: u32,
    /// Minimum raise
    pub min_raise: u32,
    /// Our chips
    pub our_chips: u32,
    /// Our player name
    pub our_name: String,
    /// Current big blind amount (for display in BB units)
    pub big_blind: u32,
    /// Game stage (preflop, flop, turn, river)
    pub stage: String,
    /// Dealer ID
    pub dealer_id: u32,
    /// Small blind player ID
    pub small_blind_id: u32,
    /// Big blind player ID
    pub big_blind_id: u32,
    /// Our player ID (assigned by server on join)
    pub our_player_id: u32,
    /// Room ID the player is in
    pub room_id: String,
    /// Blind increase configuration for this room
    pub blind_config: BlindConfig,
    /// Connection status
    pub connected: bool,
    /// Game has started
    pub game_started: bool,
    /// Per-player total bet amounts for the current hand.
    /// Tracks chips invested since the last `NewHand` so the UI can show
    /// effective stacks (stack minus bet) alongside the bet.
    pub player_bets: HashMap<u32, u32>,
    /// Player whose turn timer is currently running (if any).
    pub turn_timer_player: Option<u32>,
    /// Duration (in seconds) of the current turn timer.
    pub turn_timer_secs: u32,
    /// Monotone counter incremented each time a turn timer starts.
    /// Used by the UI to restart CSS animations.
    pub turn_counter: u64,
    /// Set of player IDs currently sitting out.
    pub sitting_out_players: HashSet<u32>,
    /// Set of player IDs that have folded in the current hand.
    pub folded_players: HashSet<u32>,
    /// Session token for reconnection after a disconnect.
    pub session_token: String,
    /// Revealed hands during showdown (cleared on NewHand).
    pub showdown_hands: Vec<ShowdownHand>,
}

impl ClientGameState {
    pub fn new(name: &str) -> Self {
        let mut events = VecDeque::new();
        events.push_back(GameEvent::Text {
            text: "Welcome to Poker! Select Start when ready to begin.".to_string(),
            category: LogCategory::System,
        });
        Self {
            events,
            players: Vec::new(),
            hole_cards: None,
            community_cards: Vec::new(),
            pot: 0,
            hand_number: 0,
            is_our_turn: false,
            valid_actions: Vec::new(),
            current_bet: 0,
            our_bet: 0,
            min_raise: 0,
            our_chips: 0,
            our_name: name.to_string(),
            big_blind: 0,
            stage: "Waiting".to_string(),
            dealer_id: 0,
            small_blind_id: 0,
            big_blind_id: 0,
            our_player_id: 0,
            room_id: String::new(),
            blind_config: BlindConfig::default(),
            connected: true,
            game_started: false,
            player_bets: HashMap::new(),
            turn_timer_player: None,
            turn_timer_secs: 0,
            turn_counter: 0,
            sitting_out_players: HashSet::new(),
            folded_players: HashSet::new(),
            session_token: String::new(),
            showdown_hands: Vec::new(),
        }
    }

    /// Append a game event, keeping only the last 100 entries.
    pub fn add_event(&mut self, event: GameEvent) {
        self.events.push_back(event);
        if self.events.len() > 100 {
            self.events.pop_front();
        }
    }

    /// Convenience: append a [`GameEvent::Text`] for ad-hoc messages.
    pub fn add_message(&mut self, text: String, category: LogCategory) {
        self.add_event(GameEvent::Text { text, category });
    }

    /// Returns true if the given action is currently valid.
    pub fn has_action(&self, action: PlayerAction) -> bool {
        self.valid_actions.contains(&action)
    }

    /// Returns true if we (the local player) are currently sitting out.
    pub fn is_sitting_out(&self) -> bool {
        self.sitting_out_players.contains(&self.our_player_id)
    }

    /// Returns true if the given player is sitting out.
    pub fn is_player_sitting_out(&self, player_id: u32) -> bool {
        self.sitting_out_players.contains(&player_id)
    }

    /// Whether a player has folded in the current hand.
    pub fn is_player_folded(&self, player_id: u32) -> bool {
        self.folded_players.contains(&player_id)
    }

    /// Look up a player's display name by ID, falling back to `"Player #N"`.
    pub fn player_name(&self, player_id: u32) -> String {
        self.players
            .iter()
            .find(|p| p.id == player_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| format!("Player #{}", player_id))
    }

    /// Evaluate the current best hand rank from the player's hole cards and community cards.
    ///
    /// Returns `None` if the player has no hole cards or not enough community
    /// cards to form a 5-card hand (need at least 3 community cards = flop).
    pub fn hand_rank(&self) -> Option<HandRank> {
        let hole = self.hole_cards?;
        if self.community_cards.len() < 3 {
            return None;
        }

        let hand = Hand(hole[0].to_card(), hole[1].to_card());
        let cc: Vec<_> = self.community_cards.iter().map(|c| c.to_card()).collect();

        let board = Board {
            flop: Some((cc[0], cc[1], cc[2])),
            turn: cc.get(3).copied(),
            river: cc.get(4).copied(),
        };

        hand.best(&board).map(|fh| fh.rank())
    }

    /// Apply a server message to the game state.
    ///
    /// Returns a [`StateChanged`] describing which aspects of the state were
    /// modified, so the UI layer can decide what to re-render or animate.
    pub fn apply_server_message(&mut self, msg: &ServerMessage) -> StateChanged {
        let mut changed = StateChanged::default();

        match msg {
            ServerMessage::Welcome { message } => {
                self.add_event(GameEvent::Welcome {
                    message: message.clone(),
                });
            }
            ServerMessage::JoinedGame {
                player_id,
                chips,
                player_count,
                session_token,
            } => {
                self.our_player_id = *player_id;
                self.our_chips = *chips;
                if !session_token.is_empty() {
                    self.session_token = session_token.clone();
                }
                if !self.players.iter().any(|p| p.id == *player_id) {
                    self.players.push(PlayerInfo {
                        id: *player_id,
                        name: self.our_name.clone(),
                        chips: *chips,
                    });
                }
                self.add_event(GameEvent::Joined {
                    player_id: *player_id,
                    chips: *chips,
                    player_count: *player_count,
                });
                changed.players = true;
            }
            ServerMessage::PlayerJoined { player_id, name } => {
                if !self.players.iter().any(|p| p.id == *player_id) {
                    // Use our own starting chips as a best guess; the
                    // server will send ChipUpdate / PlayerList with exact
                    // values shortly after.
                    self.players.push(PlayerInfo {
                        id: *player_id,
                        name: name.clone(),
                        chips: self.our_chips,
                    });
                }
                self.add_event(GameEvent::PlayerJoined {
                    player_id: *player_id,
                    name: name.clone(),
                });
                changed.players = true;
            }
            ServerMessage::PlayerLeft { player_id } => {
                let name = self.player_name(*player_id);
                self.players.retain(|p| p.id != *player_id);
                self.add_event(GameEvent::PlayerLeft {
                    player_id: *player_id,
                    name,
                });
                changed.players = true;
            }
            ServerMessage::PlayerList { players } => {
                self.players = players.clone();
                changed.players = true;
            }
            ServerMessage::ChatMessage { player_id, message } => {
                let name = self
                    .players
                    .iter()
                    .find(|p| p.id == *player_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "???".to_string());
                self.add_event(GameEvent::Chat {
                    player_id: *player_id,
                    player_name: name,
                    message: message.clone(),
                });
            }
            ServerMessage::GameStarted => {
                self.game_started = true;
                self.add_event(GameEvent::GameStarted);
                changed.actions = true;
                changed.phase = true;
            }
            ServerMessage::NewHand {
                hand_number,
                dealer_id,
                small_blind_id,
                big_blind_id,
                small_blind,
                big_blind,
            } => {
                self.hand_number = *hand_number;
                self.dealer_id = *dealer_id;
                self.small_blind_id = *small_blind_id;
                self.big_blind_id = *big_blind_id;
                self.big_blind = *big_blind;
                self.hole_cards = None;
                self.community_cards.clear();
                self.showdown_hands.clear();
                self.folded_players.clear();
                self.pot = small_blind + big_blind;
                self.stage = "Preflop".to_string();
                self.is_our_turn = false;
                self.turn_timer_player = None;
                self.turn_timer_secs = 0;
                // Reset per-player bets and record blind postings.
                self.player_bets.clear();
                self.player_bets.insert(*small_blind_id, *small_blind);
                self.player_bets.insert(*big_blind_id, *big_blind);
                self.add_event(GameEvent::NewHand {
                    hand_number: *hand_number,
                    dealer_id: *dealer_id,
                    small_blind_id: *small_blind_id,
                    big_blind_id: *big_blind_id,
                    small_blind: *small_blind,
                    big_blind: *big_blind,
                });
                changed.phase = true;
                changed.cards = true;
                changed.pot = true;
            }
            ServerMessage::HoleCards { cards } => {
                self.hole_cards = Some(*cards);
                self.add_event(GameEvent::HoleCards { cards: *cards });
                changed.cards = true;
            }
            ServerMessage::CommunityCards { stage, cards } => {
                self.community_cards = cards.clone();
                self.stage = stage.clone();
                self.add_event(GameEvent::CommunityCards {
                    stage: stage.clone(),
                    cards: cards.clone(),
                });
                changed.cards = true;
                changed.phase = true;
            }
            ServerMessage::YourTurn {
                current_bet,
                your_bet,
                pot,
                min_raise,
                valid_actions,
            } => {
                self.is_our_turn = true;
                self.current_bet = *current_bet;
                self.our_bet = *your_bet;
                self.pot = *pot;
                self.min_raise = *min_raise;
                self.valid_actions = valid_actions.clone();
                self.add_event(GameEvent::YourTurn);
                changed.actions = true;
                changed.pot = true;
            }
            ServerMessage::PlayerActed {
                player_id,
                action,
                amount,
            } => {
                if *player_id == self.our_player_id {
                    self.is_our_turn = false;
                }
                // Track folded players.
                if *action == PlayerAction::Fold {
                    self.folded_players.insert(*player_id);
                }
                // Track the chips this player put in during this action.
                if let Some(a) = amount {
                    *self.player_bets.entry(*player_id).or_insert(0) += a;
                }
                self.add_event(GameEvent::PlayerActed {
                    player_id: *player_id,
                    name: self.player_name(*player_id),
                    action: *action,
                    amount: *amount,
                });
                changed.players = true;
            }
            ServerMessage::PotUpdate { pot } => {
                self.pot = *pot;
                changed.pot = true;
            }
            ServerMessage::ChipUpdate { player_id, chips } => {
                if *player_id == self.our_player_id {
                    self.our_chips = *chips;
                }
                if let Some(p) = self.players.iter_mut().find(|p| p.id == *player_id) {
                    p.chips = *chips;
                }
                // Chips have been reconciled by the server; clear tracked bet.
                self.player_bets.remove(player_id);
                changed.players = true;
            }
            ServerMessage::Showdown { hands } => {
                let hands_with_names: Vec<_> = hands
                    .iter()
                    .map(|(id, cards, rank)| (*id, self.player_name(*id), *cards, rank.clone()))
                    .collect();
                self.showdown_hands = hands_with_names
                    .iter()
                    .map(|(id, name, cards, rank)| ShowdownHand {
                        player_id: *id,
                        name: name.clone(),
                        cards: *cards,
                        hand_rank: Some(rank.clone()),
                        equity: None,
                    })
                    .collect();
                self.add_event(GameEvent::Showdown {
                    hands: hands_with_names,
                });
                changed.phase = true;
                changed.cards = true;
            }
            ServerMessage::AllInShowdown {
                hands,
                community_cards,
            } => {
                self.community_cards = community_cards.clone();
                let hands_with_names: Vec<_> = hands
                    .iter()
                    .map(|(id, cards, eq)| (*id, self.player_name(*id), *cards, *eq))
                    .collect();
                self.showdown_hands = hands_with_names
                    .iter()
                    .map(|(id, name, cards, eq)| ShowdownHand {
                        player_id: *id,
                        name: name.clone(),
                        cards: *cards,
                        hand_rank: None,
                        equity: Some(*eq),
                    })
                    .collect();
                self.add_event(GameEvent::AllInShowdown {
                    hands: hands_with_names,
                });
                changed.cards = true;
                changed.phase = true;
            }
            ServerMessage::RoundWinner { winners } => {
                for (player_id, amount, hand) in winners {
                    self.add_event(GameEvent::RoundWinner {
                        player_id: *player_id,
                        name: self.player_name(*player_id),
                        amount: *amount,
                        hand: hand.clone(),
                    });
                }
            }
            ServerMessage::PlayerEliminated { player_id } => {
                self.add_event(GameEvent::PlayerEliminated {
                    player_id: *player_id,
                    name: self.player_name(*player_id),
                });
                changed.players = true;
            }
            ServerMessage::GameOver {
                winner_id,
                winner_name,
            } => {
                self.add_event(GameEvent::GameOver {
                    winner_id: *winner_id,
                    winner_name: winner_name.clone(),
                });
                self.game_started = false;
                self.turn_timer_player = None;
                self.turn_timer_secs = 0;
                changed.actions = true;
                changed.phase = true;
                changed.timer = true;
            }
            ServerMessage::Ok => {}
            ServerMessage::Pong => {
                self.add_event(GameEvent::Pong);
            }
            ServerMessage::Error { message } => {
                self.add_event(GameEvent::ServerError {
                    message: message.clone(),
                });
            }
            ServerMessage::RoomCreated { .. } => {
                // Handled at the connection-screen level, not game state.
            }
            ServerMessage::RoomJoined {
                room_id,
                blind_config,
            } => {
                self.room_id = room_id.clone();
                self.blind_config = *blind_config;
            }
            ServerMessage::Rejoined {
                room_id,
                player_id,
                session_token,
                chips,
                game_started,
                hand_number,
                pot,
                stage,
                community_cards,
                hole_cards,
                players,
                sitting_out,
                folded,
                blind_config,
                dealer_id,
                small_blind_id,
                big_blind_id,
                small_blind: _,
                big_blind,
            } => {
                self.room_id = room_id.clone();
                self.our_player_id = *player_id;
                self.session_token = session_token.clone();
                self.our_chips = *chips;
                self.game_started = *game_started;
                self.hand_number = *hand_number;
                self.pot = *pot;
                self.stage = stage.clone();
                self.community_cards = community_cards.clone();
                self.hole_cards = *hole_cards;
                self.players = players.clone();
                self.sitting_out_players = sitting_out.iter().copied().collect();
                self.folded_players = folded.iter().copied().collect();
                self.blind_config = *blind_config;
                self.big_blind = *big_blind;
                self.dealer_id = *dealer_id;
                self.small_blind_id = *small_blind_id;
                self.big_blind_id = *big_blind_id;
                self.connected = true;
                self.is_our_turn = false;
                self.valid_actions.clear();
                self.showdown_hands.clear();
                self.add_message("Reconnected to game.".to_string(), LogCategory::System);
                changed.players = true;
                changed.cards = true;
                changed.pot = true;
                changed.phase = true;
            }
            ServerMessage::RoomError { message } => {
                self.add_event(GameEvent::ServerError {
                    message: message.clone(),
                });
            }
            ServerMessage::BlindsIncreased {
                small_blind,
                big_blind,
            } => {
                self.big_blind = *big_blind;
                self.add_event(GameEvent::BlindsIncreased {
                    small_blind: *small_blind,
                    big_blind: *big_blind,
                });
                changed.phase = true;
            }
            ServerMessage::TurnTimerStarted {
                player_id,
                timeout_secs,
            } => {
                self.turn_timer_player = Some(*player_id);
                self.turn_timer_secs = *timeout_secs;
                self.turn_counter += 1;
                self.add_event(GameEvent::TurnTimerStarted {
                    player_id: *player_id,
                    name: self.player_name(*player_id),
                    timeout_secs: *timeout_secs,
                });
                changed.timer = true;
            }
            ServerMessage::PlayerSatOut { player_id } => {
                self.sitting_out_players.insert(*player_id);
                self.add_event(GameEvent::PlayerSatOut {
                    player_id: *player_id,
                    name: self.player_name(*player_id),
                });
                changed.players = true;
            }
            ServerMessage::PlayerSatIn { player_id } => {
                self.sitting_out_players.remove(player_id);
                self.add_event(GameEvent::PlayerSatIn {
                    player_id: *player_id,
                    name: self.player_name(*player_id),
                });
                changed.players = true;
            }
        }

        changed
    }

    /// Decide which `ClientMessage` to send for a fold/check action.
    pub fn fold_or_check(&self) -> Option<ClientMessage> {
        if !self.is_our_turn {
            return None;
        }
        if self.has_action(PlayerAction::Check) {
            Some(ClientMessage::Check)
        } else if self.has_action(PlayerAction::Fold) {
            Some(ClientMessage::Fold)
        } else {
            None
        }
    }

    /// Decide which `ClientMessage` to send for a call action.
    pub fn call(&self) -> Option<ClientMessage> {
        if !self.is_our_turn || !self.has_action(PlayerAction::Call) {
            return None;
        }
        Some(ClientMessage::Call)
    }

    /// Validate and build a raise `ClientMessage`.
    ///
    /// Returns `Err(message)` with a user-facing error string on invalid input.
    pub fn raise(&self, amount: u32, is_all_in: bool) -> Result<ClientMessage, String> {
        if !self.is_our_turn {
            return Err("Not your turn".to_string());
        }
        let can_raise = self.has_action(PlayerAction::Raise);
        let can_allin = self.has_action(PlayerAction::AllIn);

        if is_all_in && can_allin {
            return Ok(ClientMessage::AllIn);
        }
        if amount == 0 {
            return Err("Raise amount must be greater than 0".to_string());
        }
        if !can_raise {
            if can_allin {
                return Err("Raise not available. Select All-In and press Raise.".to_string());
            }
            return Err("Raise not available".to_string());
        }
        Ok(ClientMessage::Raise { amount })
    }

    /// Compute a pot-percentage raise amount, clamped to the player's stack.
    pub fn pot_percentage_raise(&self, percentage: u32) -> u32 {
        let to_call = self.current_bet.saturating_sub(self.our_bet);
        let max_raise = self.our_chips.saturating_sub(to_call);
        (self.pot.saturating_mul(percentage) / 100)
            .max(self.min_raise)
            .min(max_raise)
    }

    /// Maximum raise (all-in amount).
    pub fn max_raise(&self) -> u32 {
        let to_call = self.current_bet.saturating_sub(self.our_bet);
        self.our_chips.saturating_sub(to_call)
    }
}

// ---------------------------------------------------------------------------
// Raise presets
// ---------------------------------------------------------------------------

/// A preset raise amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaisePreset {
    /// A percentage of the current pot (e.g. 35, 50, 75, 100).
    Pot(u32),
    /// Go all-in.
    AllIn,
}

/// The default set of raise presets shown to players.
pub const RAISE_PRESETS: &[RaisePreset] = &[
    RaisePreset::Pot(35),
    RaisePreset::Pot(50),
    RaisePreset::Pot(75),
    RaisePreset::Pot(100),
    RaisePreset::AllIn,
];

impl RaisePreset {
    /// Human-readable label for this preset.
    pub fn label(self) -> &'static str {
        match self {
            RaisePreset::Pot(35) => "35%",
            RaisePreset::Pot(50) => "50%",
            RaisePreset::Pot(75) => "75%",
            RaisePreset::Pot(100) => "100%",
            RaisePreset::Pot(_) => "?%",
            RaisePreset::AllIn => "All-In",
        }
    }

    /// Compute the raise amount for this preset given the current game state.
    pub fn amount(self, gs: &ClientGameState) -> u32 {
        match self {
            RaisePreset::Pot(pct) => gs.pot_percentage_raise(pct),
            RaisePreset::AllIn => gs.max_raise(),
        }
    }
}
