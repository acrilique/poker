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

//! Room manager for the multi-room poker server.
//!
//! Each room contains an independent [`GameState`] and a set of connected
//! players, each with their own [`mpsc`] sender for targeted message delivery
//! (no broadcast fan-out of private data).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use poker_core::game_logic::{GamePhase, GameState, PlayerStatus};
use poker_core::protocol::{
    BlindConfig, CardInfo, PlayerInfo, RoomErrorKind, RoomIdError, ServerMessage, Stage,
    card_to_info, validate_room_id,
};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock, mpsc};

/// How long a disconnected player's seat is held before permanent removal.
const SESSION_GRACE_PERIOD: Duration = Duration::from_secs(5 * 60); // 5 minutes

/// Maximum number of players allowed in a single room.
///
/// A standard 52-card deck can deal at most 23 two-card hands (with 5
/// community cards and a burn card per street), but real poker tables
/// seat at most 9–10 players.  Capping here prevents a deck-exhaustion
/// panic in `start_new_hand`.
const MAX_PLAYERS_PER_ROOM: usize = 9;

/// Maximum number of rooms that can exist at the same time.
///
/// Without this, a malicious client could loop `CreateRoom` and exhaust
/// server memory.  The value is generous for legitimate use while still
/// bounding resource consumption.
const MAX_ACTIVE_ROOMS: usize = 100;

/// Maximum number of outbound messages buffered per player before the
/// connection is considered too slow and the sender is dropped.
const PLAYER_CHANNEL_CAPACITY: usize = 128;

/// Errors returned by [`RoomManager`] operations.
#[derive(Debug, Error)]
pub enum RoomError {
    /// The room ID failed validation.
    #[error(transparent)]
    InvalidRoomId(#[from] RoomIdError),

    /// The server-wide room limit has been reached.
    #[error("Server room limit reached (max {MAX_ACTIVE_ROOMS}). Try again later.")]
    ServerFull,

    /// A room with the requested ID already exists.
    #[error("Room '{0}' already exists")]
    RoomAlreadyExists(String),

    /// No room exists with the given ID.
    #[error("Room '{0}' not found")]
    RoomNotFound(String),

    /// The room has no open seats.
    #[error("Room is full (max {MAX_PLAYERS_PER_ROOM} players)")]
    RoomFull,

    /// The game is already running and late entry is disabled.
    #[error("Game already in progress")]
    GameInProgress,

    /// The session token does not match any known session.
    #[error("Invalid or expired session token")]
    InvalidSession,

    /// The player's session was valid but the player was already removed.
    #[error("Session expired — player was removed")]
    SessionExpired,
}

impl From<&RoomError> for RoomErrorKind {
    fn from(err: &RoomError) -> Self {
        match err {
            RoomError::InvalidRoomId(e) => match e {
                RoomIdError::Empty => RoomErrorKind::RoomIdEmpty,
                RoomIdError::TooLong => RoomErrorKind::RoomIdTooLong,
                RoomIdError::InvalidChars => RoomErrorKind::RoomIdInvalidChars,
            },
            RoomError::ServerFull => RoomErrorKind::ServerFull,
            RoomError::RoomAlreadyExists(id) => RoomErrorKind::RoomAlreadyExists {
                room_id: id.clone(),
            },
            RoomError::RoomNotFound(id) => RoomErrorKind::RoomNotFound {
                room_id: id.clone(),
            },
            RoomError::RoomFull => RoomErrorKind::RoomFull,
            RoomError::GameInProgress => RoomErrorKind::GameInProgress,
            RoomError::InvalidSession => RoomErrorKind::InvalidSession,
            RoomError::SessionExpired => RoomErrorKind::SessionExpired,
        }
    }
}

/// Handle to a per-player outbound channel.
///
/// The WebSocket write loop drains this receiver and forwards messages as
/// text frames.  The channel is **bounded** so that a slow or malicious
/// client cannot cause unbounded memory growth on the server.
pub type PlayerTx = mpsc::Sender<ServerMessage>;
pub type PlayerRx = mpsc::Receiver<ServerMessage>;

/// A single poker room.
pub struct Room {
    /// Server-side game state (deck, hands, betting, etc.).
    pub game_state: GameState,
    /// Per-player outbound senders keyed by player ID.
    pub player_senders: HashMap<u32, PlayerTx>,
    /// Blind increase configuration for this room.
    pub blind_config: BlindConfig,
    /// Monotonically increasing counter incremented every time a new turn
    /// starts.  Used to invalidate stale turn-timer tasks.
    pub turn_counter: Arc<AtomicU64>,
    /// Maps session tokens to player IDs for reconnection.
    pub sessions: HashMap<String, u32>,
    /// Maps player IDs to session tokens (reverse lookup).
    pub player_sessions: HashMap<u32, String>,
    /// Tracks when disconnected players should be permanently removed.
    pub disconnected_at: HashMap<u32, Instant>,
}

impl Room {
    fn new(blind_config: BlindConfig, starting_bbs: u32) -> Self {
        let mut gs = GameState::new();
        gs.blind_config = blind_config;
        gs.starting_bbs = starting_bbs;
        Self {
            game_state: gs,
            player_senders: HashMap::new(),
            blind_config,
            turn_counter: Arc::new(AtomicU64::new(0)),
            sessions: HashMap::new(),
            player_sessions: HashMap::new(),
            disconnected_at: HashMap::new(),
        }
    }
}

/// Send a message to a specific player.
///
/// Uses `try_send` to avoid blocking. If the channel is full the
/// player's sender is dropped, which will cause the write task to
/// terminate and trigger a disconnect.
pub fn send_to_player(senders: &mut HashMap<u32, PlayerTx>, player_id: u32, msg: &ServerMessage) {
    if let Some(tx) = senders.get(&player_id)
        && tx.try_send(msg.clone()).is_err()
    {
        tracing::warn!(
            player = player_id,
            "Channel full or closed — dropping sender"
        );
        senders.remove(&player_id);
    }
}

/// Broadcast a message to **all** connected players in this room.
///
/// Senders whose channels are full are removed (see [`send_to_player`]).
pub fn broadcast(senders: &mut HashMap<u32, PlayerTx>, msg: &ServerMessage) {
    senders.retain(|&pid, tx| {
        if tx.try_send(msg.clone()).is_err() {
            tracing::warn!(player = pid, "Channel full or closed — dropping sender");
            false
        } else {
            true
        }
    });
}

/// Broadcast a message to all connected players **except** `exclude_id`.
pub fn broadcast_except(
    senders: &mut HashMap<u32, PlayerTx>,
    msg: &ServerMessage,
    exclude_id: u32,
) {
    senders.retain(|&pid, tx| {
        if pid == exclude_id {
            return true; // keep but skip
        }
        if tx.try_send(msg.clone()).is_err() {
            tracing::warn!(player = pid, "Channel full or closed — dropping sender");
            false
        } else {
            true
        }
    });
}

impl Room {
    /// Register a session token for a player.
    pub fn register_session(&mut self, player_id: u32, token: String) {
        self.sessions.insert(token.clone(), player_id);
        self.player_sessions.insert(player_id, token);
    }

    /// Build a full state snapshot [`ServerMessage::Rejoined`] for a
    /// reconnecting player.
    pub fn build_rejoin_snapshot(
        &self,
        room_id: &str,
        player_id: u32,
        session_token: &str,
    ) -> ServerMessage {
        let gs = &self.game_state;
        let players: Vec<PlayerInfo> = gs
            .players
            .values()
            .map(|p| PlayerInfo {
                id: p.id,
                name: p.name.clone(),
                chips: p.chips,
            })
            .collect();

        let sitting_out: Vec<u32> = gs
            .players
            .values()
            .filter(|p| p.sitting_out)
            .map(|p| p.id)
            .collect();

        let folded: Vec<u32> = gs
            .players
            .values()
            .filter(|p| p.status == PlayerStatus::Folded)
            .map(|p| p.id)
            .collect();

        let community_cards: Vec<CardInfo> = gs.community_cards.iter().map(card_to_info).collect();

        let hole_cards = gs
            .players
            .get(&player_id)
            .and_then(|p| p.hole_cards)
            .map(|(c1, c2)| [card_to_info(&c1), card_to_info(&c2)]);

        let chips = gs.players.get(&player_id).map(|p| p.chips).unwrap_or(0);

        let stage = match gs.phase {
            GamePhase::Lobby => Stage::Preflop,
            GamePhase::PreFlop => Stage::Preflop,
            GamePhase::Flop => Stage::Flop,
            GamePhase::Turn => Stage::Turn,
            GamePhase::River => Stage::River,
            GamePhase::Showdown => Stage::Showdown,
        };

        // Determine blind positions from current hand state.
        let n = gs.player_order.len();
        let (dealer_id, sb_id, bb_id) = if n >= 2 {
            let d = gs.player_order[gs.dealer_index % n];
            let sb = gs.player_order[(gs.dealer_index + 1) % n];
            let bb = gs.player_order[(gs.dealer_index + 2) % n];
            (d, sb, bb)
        } else {
            (0, 0, 0)
        };

        ServerMessage::Rejoined {
            room_id: room_id.to_string(),
            player_id,
            session_token: session_token.to_string(),
            chips,
            game_started: gs.game_started,
            hand_number: gs.hand_number,
            pot: gs.pot,
            stage,
            community_cards,
            hole_cards,
            players,
            sitting_out,
            folded,
            blind_config: self.blind_config,
            allow_late_entry: gs.allow_late_entry,
            is_host: gs.host_id == player_id,
            dealer_id,
            small_blind_id: sb_id,
            big_blind_id: bb_id,
            small_blind: gs.small_blind,
            big_blind: gs.big_blind,
        }
    }
}

/// Manages all active rooms.
///
/// Thread-safe: the outer `RwLock` allows concurrent reads (e.g. looking up
/// rooms) while writes (create / remove) take exclusive access.  Each room
/// is individually `Mutex`-protected so independent rooms never contend.
pub struct RoomManager {
    rooms: Arc<RwLock<HashMap<String, Arc<Mutex<Room>>>>>,
}

impl RoomManager {
    pub fn new() -> Self {
        Self {
            rooms: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new room with the given ID.
    ///
    /// Returns an error if the room ID is invalid or already taken.
    pub async fn create_room(
        &self,
        room_id: &str,
        blind_config: BlindConfig,
        starting_bbs: u32,
    ) -> Result<(), RoomError> {
        validate_room_id(room_id)?;

        let mut rooms = self.rooms.write().await;
        if rooms.len() >= MAX_ACTIVE_ROOMS {
            return Err(RoomError::ServerFull);
        }
        if rooms.contains_key(room_id) {
            return Err(RoomError::RoomAlreadyExists(room_id.to_string()));
        }
        rooms.insert(
            room_id.to_string(),
            Arc::new(Mutex::new(Room::new(blind_config, starting_bbs))),
        );
        Ok(())
    }

    /// Look up a room by ID.
    pub async fn get_room(&self, room_id: &str) -> Option<Arc<Mutex<Room>>> {
        let rooms = self.rooms.read().await;
        rooms.get(room_id).cloned()
    }

    /// Add a player to a room.
    ///
    /// Returns `(player_id, session_token, PlayerRx)` on success so the caller
    /// can wire up the WebSocket write loop.
    pub async fn join_room(
        &self,
        room_id: &str,
        player_name: &str,
    ) -> Result<(u32, String, usize, PlayerRx, Arc<Mutex<Room>>), RoomError> {
        let room_arc = self
            .get_room(room_id)
            .await
            .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?;

        let mut room = room_arc.lock().await;

        if room.game_state.player_count() >= MAX_PLAYERS_PER_ROOM {
            return Err(RoomError::RoomFull);
        }
        if room.game_state.game_started && !room.game_state.allow_late_entry {
            return Err(RoomError::GameInProgress);
        }
        let player = if room.game_state.game_started {
            // Late entry: give the frozen starting chip amount.
            let chips = room.game_state.starting_chips;
            let p = room
                .game_state
                .add_player_with_chips(player_name.to_string(), Some(chips));
            // Late-joiners sit out until the next hand.
            room.game_state.set_sitting_out(p.id);
            p
        } else {
            room.game_state.add_player(player_name.to_string())
        };
        // First player to join becomes the host.
        if room.game_state.host_id == 0 {
            room.game_state.host_id = player.id;
        }
        let player_id = player.id;
        let player_count = room.game_state.player_count();

        let session_token = generate_session_token();
        room.register_session(player_id, session_token.clone());

        let (tx, rx) = mpsc::channel(PLAYER_CHANNEL_CAPACITY);
        room.player_senders.insert(player_id, tx);

        // Notify existing players about the new player.
        let join_msg = ServerMessage::PlayerJoined {
            player_id,
            name: player_name.to_string(),
        };
        broadcast_except(&mut room.player_senders, &join_msg, player_id);

        drop(room);

        Ok((player_id, session_token, player_count, rx, room_arc))
    }

    /// Reconnect a previously-disconnected player using their session token.
    ///
    /// Returns the player_id and a new `PlayerRx` on success.
    pub async fn rejoin_room(
        &self,
        room_id: &str,
        session_token: &str,
    ) -> Result<(u32, PlayerRx, Arc<Mutex<Room>>), RoomError> {
        let room_arc = self
            .get_room(room_id)
            .await
            .ok_or_else(|| RoomError::RoomNotFound(room_id.to_string()))?;

        let mut room = room_arc.lock().await;

        let player_id = *room
            .sessions
            .get(session_token)
            .ok_or(RoomError::InvalidSession)?;

        // Verify the player still exists in game state.
        if !room.game_state.players.contains_key(&player_id) {
            // Token was valid but player was already fully removed.
            room.sessions.remove(session_token);
            room.player_sessions.remove(&player_id);
            return Err(RoomError::SessionExpired);
        }

        // Clear the disconnected-at timestamp (cancel grace period).
        room.disconnected_at.remove(&player_id);

        // Replace the sender channel.
        let (tx, rx) = mpsc::channel(PLAYER_CHANNEL_CAPACITY);
        room.player_senders.insert(player_id, tx);

        drop(room);
        Ok((player_id, rx, room_arc))
    }

    /// Soft-disconnect a player during a game: mark them as sitting out and
    /// start a grace period.  Their game state is preserved.
    pub async fn disconnect_player(&self, room_id: &str, player_id: u32) {
        let rooms = self.rooms.read().await;
        let Some(room_arc) = rooms.get(room_id) else {
            return;
        };

        let mut room = room_arc.lock().await;
        room.player_senders.remove(&player_id);

        let game_in_progress =
            if room.game_state.game_started && room.game_state.players.contains_key(&player_id) {
                // Sit the player out so auto-check/fold kicks in.
                if !room
                    .game_state
                    .players
                    .get(&player_id)
                    .map(|p| p.sitting_out)
                    .unwrap_or(true)
                {
                    room.game_state.set_sitting_out(player_id);
                    broadcast(
                        &mut room.player_senders,
                        &ServerMessage::PlayerSatOut { player_id },
                    );
                }
                true
            } else {
                false
            };

        if game_in_progress {
            // Keep the player in game state; start the grace-period countdown.
            room.disconnected_at.insert(player_id, Instant::now());
            tracing::info!(
                room = room_id,
                player = player_id,
                "Player disconnected — seat held for {:?}",
                SESSION_GRACE_PERIOD,
            );

            // Spawn a task that will permanently remove the player if they
            // don't reconnect within the grace period.
            let rm = self_ref(room_id, &self.rooms).await;
            let rid = room_id.to_string();
            let grace = SESSION_GRACE_PERIOD;
            let rooms_ref = Arc::clone(&self.rooms);
            drop(room);
            drop(rooms);

            if let Some(rm) = rm {
                tokio::spawn(async move {
                    tokio::time::sleep(grace).await;
                    let mut room = rm.lock().await;
                    // Only remove if they're still marked as disconnected.
                    if room
                        .disconnected_at
                        .get(&player_id)
                        .is_some_and(|t| t.elapsed() >= grace)
                    {
                        room.disconnected_at.remove(&player_id);
                        if let Some(token) = room.player_sessions.remove(&player_id) {
                            room.sessions.remove(&token);
                        }
                        room.game_state.remove_player(player_id);
                        broadcast(
                            &mut room.player_senders,
                            &ServerMessage::PlayerLeft { player_id },
                        );
                        tracing::info!(
                            room = %rid,
                            player = player_id,
                            "Grace period expired — player permanently removed"
                        );

                        // If no connected players remain, remove the room
                        // entirely so it doesn't leak memory.
                        if room.player_senders.is_empty() {
                            drop(room);
                            let mut rooms = rooms_ref.write().await;
                            if let Some(room_arc) = rooms.get(&rid) {
                                let r = room_arc.lock().await;
                                if r.player_senders.is_empty() {
                                    drop(r);
                                    rooms.remove(&rid);
                                    tracing::info!(room = %rid, "Removed empty room after grace period");
                                }
                            }
                        }
                    }
                });
            }
        } else {
            // Game hasn't started — remove immediately.
            if let Some(token) = room.player_sessions.remove(&player_id) {
                room.sessions.remove(&token);
            }
            room.game_state.remove_player(player_id);
            broadcast(
                &mut room.player_senders,
                &ServerMessage::PlayerLeft { player_id },
            );

            let is_empty = room.player_senders.is_empty();
            drop(room);
            drop(rooms);

            if is_empty {
                let mut rooms = self.rooms.write().await;
                if let Some(room_arc) = rooms.get(room_id) {
                    let room = room_arc.lock().await;
                    if room.player_senders.is_empty() {
                        drop(room);
                        rooms.remove(room_id);
                        tracing::info!(room_id, "Removed empty room");
                    }
                }
            }
        }
    }

    /// List active room IDs (for debugging / future API).
    pub async fn list_rooms(&self) -> Vec<String> {
        let rooms = self.rooms.read().await;
        rooms.keys().cloned().collect()
    }
}

/// Generate a random session token (32-char hex string).
fn generate_session_token() -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    let bytes: [u8; 16] = rng.random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Helper: get an `Arc<Mutex<Room>>` reference from `rooms` RwLock.
async fn self_ref(
    room_id: &str,
    rooms: &RwLock<HashMap<String, Arc<Mutex<Room>>>>,
) -> Option<Arc<Mutex<Room>>> {
    let rooms = rooms.read().await;
    rooms.get(room_id).cloned()
}
