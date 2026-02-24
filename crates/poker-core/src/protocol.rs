use serde::{Deserialize, Serialize};
use std::fmt;

use crate::poker::{Card, CardSuit};

/// Serializable card representation
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct CardInfo {
    pub rank: u8, // 2-14 (14 = Ace)
    pub suit: u8, // 0-3 (Diamonds, Spades, Clubs, Hearts)
}

/// Convert an internal [`Card`] to the wire-level [`CardInfo`].
pub fn card_to_info(card: &Card) -> CardInfo {
    CardInfo {
        rank: card.number() as u8,
        suit: match card.suit() {
            CardSuit::Diamonds => 0,
            CardSuit::Spades => 1,
            CardSuit::Clubs => 2,
            CardSuit::Hearts => 3,
        },
    }
}

impl CardInfo {
    /// Convert this wire-level card into an internal [`Card`](crate::poker::Card).
    pub fn to_card(self) -> crate::poker::Card {
        use crate::poker::{Card, CardNumber, CardSuit};
        let number = match self.rank {
            2 => CardNumber::Two,
            3 => CardNumber::Three,
            4 => CardNumber::Four,
            5 => CardNumber::Five,
            6 => CardNumber::Six,
            7 => CardNumber::Seven,
            8 => CardNumber::Eight,
            9 => CardNumber::Nine,
            10 => CardNumber::Ten,
            11 => CardNumber::Jack,
            12 => CardNumber::Queen,
            13 => CardNumber::King,
            14 => CardNumber::Ace,
            _ => CardNumber::Two, // fallback
        };
        let suit = match self.suit {
            0 => CardSuit::Diamonds,
            1 => CardSuit::Spades,
            2 => CardSuit::Clubs,
            3 => CardSuit::Hearts,
            _ => CardSuit::Diamonds, // fallback
        };
        Card(number, suit)
    }

    pub fn rank_str(&self) -> &'static str {
        match self.rank {
            2 => "2",
            3 => "3",
            4 => "4",
            5 => "5",
            6 => "6",
            7 => "7",
            8 => "8",
            9 => "9",
            10 => "10",
            11 => "J",
            12 => "Q",
            13 => "K",
            14 => "A",
            _ => "?",
        }
    }

    pub fn suit_str(&self) -> &'static str {
        match self.suit {
            0 => "♦",
            1 => "♠",
            2 => "♣",
            3 => "♥",
            _ => "?",
        }
    }
}

impl fmt::Display for CardInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.rank_str(), self.suit_str())
    }
}

/// Serializable player info for the wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerInfo {
    pub id: u32,
    pub name: String,
    pub chips: u32,
}

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
    pub fn label(self) -> &'static str {
        match self {
            PlayerAction::Fold => "Fold",
            PlayerAction::Check => "Check",
            PlayerAction::Call => "Call",
            PlayerAction::Raise => "Raise",
            PlayerAction::AllIn => "All-In",
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
    pub fn is_enabled(&self) -> bool {
        self.interval_secs > 0 && self.increase_percent > 0
    }
}

fn default_starting_bbs() -> u32 {
    100
}

/// Messages sent from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// Join the game with a player name (sent automatically on connect)
    Join { name: String },

    /// Create a new room with the given ID and optional blind config.
    CreateRoom {
        room_id: String,
        #[serde(default)]
        blind_config: BlindConfig,
        /// Number of big blinds each player starts with (default: 50).
        #[serde(default = "default_starting_bbs")]
        starting_bbs: u32,
    },

    /// Join an existing room with the given ID and player name.
    JoinRoom { room_id: String, name: String },

    /// Request list of current players
    GetPlayers,

    /// Send a chat message
    Chat { message: String },

    /// Request to start the game
    StartGame,

    /// Fold current hand
    Fold,

    /// Check (pass without betting)
    Check,

    /// Call the current bet
    Call,

    /// Raise by a specific amount
    Raise { amount: u32 },

    /// Go all-in
    AllIn,

    /// Request to sit out (auto-fold/check each turn).
    SitOut,

    /// Request to sit back in.
    SitIn,

    /// Toggle late entry (host only).
    ToggleLateEntry,

    /// Re-join a room after a disconnect using a previously issued session token.
    Rejoin {
        room_id: String,
        session_token: String,
    },

    /// Ping to check connection
    Ping,
}

/// Messages sent from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Welcome message on connection
    Welcome { message: String },

    /// Confirmation of joining
    JoinedGame {
        player_id: u32,
        chips: u32,
        player_count: usize,
        /// Session token for reconnection after a disconnect.
        #[serde(default)]
        session_token: String,
        /// Whether this player is the room host.
        #[serde(default)]
        is_host: bool,
        /// Whether late entry is currently allowed.
        #[serde(default)]
        allow_late_entry: bool,
    },

    /// A new player joined
    PlayerJoined { player_id: u32, name: String },

    /// A player left
    PlayerLeft { player_id: u32 },

    /// List of all players
    PlayerList { players: Vec<PlayerInfo> },

    /// Chat message from a player
    ChatMessage { player_id: u32, message: String },

    /// Game has started
    GameStarted,

    /// New hand/round is starting
    NewHand {
        hand_number: u32,
        dealer_id: u32,
        small_blind_id: u32,
        big_blind_id: u32,
        small_blind: u32,
        big_blind: u32,
    },

    /// Your hole cards (private, only sent to the specific player)
    HoleCards { cards: [CardInfo; 2] },

    /// Community cards revealed
    CommunityCards {
        stage: String, // "flop", "turn", "river"
        cards: Vec<CardInfo>,
    },

    /// It's your turn to act
    YourTurn {
        current_bet: u32,
        your_bet: u32,
        pot: u32,
        min_raise: u32,
        valid_actions: Vec<PlayerAction>,
    },

    /// A player performed an action
    PlayerActed {
        player_id: u32,
        action: PlayerAction,
        amount: Option<u32>,
    },

    /// Pot update
    PotUpdate { pot: u32 },

    /// Player chip update
    ChipUpdate { player_id: u32, chips: u32 },

    /// Showdown - reveal all remaining players' hands
    Showdown {
        hands: Vec<(u32, [CardInfo; 2], String)>, // (player_id, cards, hand_rank)
    },

    /// All-in showdown (flip) - reveal hands and equity before running out the board
    AllInShowdown {
        hands: Vec<(u32, [CardInfo; 2], f64)>, // (player_id, cards, equity percentage)
        community_cards: Vec<CardInfo>,
    },

    /// Round winner(s)
    RoundWinner {
        winners: Vec<(u32, u32, String)>, // (player_id, amount_won, hand_description)
    },

    /// Player eliminated (out of chips)
    PlayerEliminated { player_id: u32 },

    /// Game over - tournament finished
    GameOver { winner_id: u32, winner_name: String },

    /// Blinds have increased at the start of a new hand.
    BlindsIncreased { small_blind: u32, big_blind: u32 },

    /// A player's turn timer has started.
    ///
    /// Broadcast to all players so UIs can show a countdown.
    TurnTimerStarted { player_id: u32, timeout_secs: u32 },

    /// A player is now sitting out.
    PlayerSatOut { player_id: u32 },

    /// A player is back in (no longer sitting out).
    PlayerSatIn { player_id: u32 },

    /// Late-entry setting changed.
    LateEntryChanged { allowed: bool },

    /// The game is paused waiting for enough active players to continue.
    WaitingForPlayers,

    /// A room was successfully created.
    RoomCreated { room_id: String },

    /// Successfully joined a room.
    RoomJoined {
        room_id: String,
        #[serde(default)]
        blind_config: BlindConfig,
    },

    /// Full state snapshot sent on successful rejoin.
    Rejoined {
        room_id: String,
        player_id: u32,
        session_token: String,
        chips: u32,
        game_started: bool,
        hand_number: u32,
        pot: u32,
        stage: String,
        community_cards: Vec<CardInfo>,
        hole_cards: Option<[CardInfo; 2]>,
        players: Vec<PlayerInfo>,
        sitting_out: Vec<u32>,
        #[serde(default)]
        folded: Vec<u32>,
        #[serde(default)]
        blind_config: BlindConfig,
        #[serde(default)]
        allow_late_entry: bool,
        #[serde(default)]
        is_host: bool,
        dealer_id: u32,
        small_blind_id: u32,
        big_blind_id: u32,
        small_blind: u32,
        big_blind: u32,
    },

    /// Room-related error (e.g. "room ID taken", "room not found").
    RoomError { message: String },

    /// Generic OK response
    Ok,

    /// Pong response to ping
    Pong,

    /// Error message
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Room ID validation
// ---------------------------------------------------------------------------

/// Validate a room ID.
///
/// Room IDs must be non-empty, alphanumeric, and fewer than 20 characters.
pub fn validate_room_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Room ID cannot be empty".to_string());
    }
    if id.len() >= 20 {
        return Err("Room ID must be fewer than 20 characters".to_string());
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("Room ID must be alphanumeric".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_room_ids() {
        assert!(validate_room_id("abc123").is_ok());
        assert!(validate_room_id("A").is_ok());
        assert!(validate_room_id("Room42").is_ok());
        assert!(validate_room_id("1234567890123456789").is_ok()); // 19 chars
    }

    #[test]
    fn invalid_room_ids() {
        assert!(validate_room_id("").is_err());
        assert!(validate_room_id("12345678901234567890").is_err()); // 20 chars
        assert!(validate_room_id("hello world").is_err());
        assert!(validate_room_id("room-1").is_err());
        assert!(validate_room_id("room_1").is_err());
    }
}
