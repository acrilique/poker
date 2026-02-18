use std::collections::VecDeque;

use crate::protocol::{CardInfo, ClientMessage, PlayerAction, PlayerInfo, ServerMessage};

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
    PlayerLeft { player_id: u32 },
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
        action: PlayerAction,
        amount: Option<u32>,
    },
    /// Showdown — reveal hands.
    Showdown {
        hands: Vec<(u32, [CardInfo; 2], String)>,
    },
    /// All-in showdown with equity.
    AllInShowdown {
        hands: Vec<(u32, [CardInfo; 2], f64)>,
    },
    /// A player won the round.
    RoundWinner {
        player_id: u32,
        amount: u32,
        hand: String,
    },
    /// A player was eliminated (out of chips).
    PlayerEliminated { player_id: u32 },
    /// The game is over.
    GameOver { winner_id: u32, winner_name: String },
    /// Pong response.
    Pong,
    /// Error from the server.
    ServerError { message: String },
    /// Server disconnected.
    Disconnected,
    /// Connection error.
    ConnectionError { message: String },
    /// An unrecognised message from the server.
    Unknown { raw: String },
    /// Generic text message (used by the UI layer for local feedback).
    Text { text: String, category: LogCategory },
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

            Self::ServerError { .. } | Self::Disconnected | Self::ConnectionError { .. } => {
                LogCategory::Error
            }

            Self::PlayerJoined { .. }
            | Self::PlayerLeft { .. }
            | Self::HoleCards { .. }
            | Self::CommunityCards { .. }
            | Self::PlayerEliminated { .. }
            | Self::Pong
            | Self::Unknown { .. } => LogCategory::Info,

            Self::Text { category, .. } => *category,
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
}

impl StateChanged {
    /// Returns `true` if any flag is set.
    pub fn any(self) -> bool {
        self.actions || self.players || self.cards || self.pot || self.phase
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
    /// Our player ID (assigned by server on join)
    pub our_player_id: u32,
    /// Room ID the player is in
    pub room_id: String,
    /// Connection status
    pub connected: bool,
    /// Game has started
    pub game_started: bool,
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
            our_player_id: 0,
            room_id: String::new(),
            connected: true,
            game_started: false,
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
            } => {
                self.our_player_id = *player_id;
                self.our_chips = *chips;
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
                    self.players.push(PlayerInfo {
                        id: *player_id,
                        name: name.clone(),
                        chips: 1000,
                    });
                }
                self.add_event(GameEvent::PlayerJoined {
                    player_id: *player_id,
                    name: name.clone(),
                });
                changed.players = true;
            }
            ServerMessage::PlayerLeft { player_id } => {
                self.players.retain(|p| p.id != *player_id);
                self.add_event(GameEvent::PlayerLeft {
                    player_id: *player_id,
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
                self.big_blind = *big_blind;
                self.hole_cards = None;
                self.community_cards.clear();
                self.pot = small_blind + big_blind;
                self.stage = "Preflop".to_string();
                self.is_our_turn = false;
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
                self.add_event(GameEvent::PlayerActed {
                    player_id: *player_id,
                    action: *action,
                    amount: *amount,
                });
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
                changed.players = true;
            }
            ServerMessage::Showdown { hands } => {
                self.add_event(GameEvent::Showdown {
                    hands: hands.clone(),
                });
                changed.phase = true;
            }
            ServerMessage::AllInShowdown {
                hands,
                community_cards,
            } => {
                self.community_cards = community_cards.clone();
                self.add_event(GameEvent::AllInShowdown {
                    hands: hands.clone(),
                });
                changed.cards = true;
                changed.phase = true;
            }
            ServerMessage::RoundWinner { winners } => {
                for (player_id, amount, hand) in winners {
                    self.add_event(GameEvent::RoundWinner {
                        player_id: *player_id,
                        amount: *amount,
                        hand: hand.clone(),
                    });
                }
            }
            ServerMessage::PlayerEliminated { player_id } => {
                self.add_event(GameEvent::PlayerEliminated {
                    player_id: *player_id,
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
                changed.actions = true;
                changed.phase = true;
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
            ServerMessage::RoomJoined { room_id } => {
                self.room_id = room_id.clone();
            }
            ServerMessage::RoomError { message } => {
                self.add_event(GameEvent::ServerError {
                    message: message.clone(),
                });
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

