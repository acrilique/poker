//! Room manager for the multi-room poker server.
//!
//! Each room contains an independent [`GameState`] and a set of connected
//! players, each with their own [`mpsc`] sender for targeted message delivery
//! (no broadcast fan-out of private data).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use poker_core::game_logic::GameState;
use poker_core::protocol::{BlindConfig, ServerMessage, validate_room_id};
use tokio::sync::{Mutex, RwLock, mpsc};

/// Handle to a per-player outbound channel.
///
/// The WebSocket write loop drains this receiver and forwards messages as
/// text frames.
pub type PlayerTx = mpsc::UnboundedSender<ServerMessage>;
pub type PlayerRx = mpsc::UnboundedReceiver<ServerMessage>;

/// A single poker room.
pub struct Room {
    /// Server-side game state (deck, hands, betting, etc.).
    pub game_state: Arc<Mutex<GameState>>,
    /// Per-player outbound senders keyed by player ID.
    pub player_senders: HashMap<u32, PlayerTx>,
    /// Blind increase configuration for this room.
    pub blind_config: BlindConfig,
    /// Monotonically increasing counter incremented every time a new turn
    /// starts.  Used to invalidate stale turn-timer tasks.
    pub turn_counter: Arc<AtomicU64>,
}

impl Room {
    fn new(blind_config: BlindConfig) -> Self {
        let mut gs = GameState::new();
        gs.blind_config = blind_config;
        Self {
            game_state: Arc::new(Mutex::new(gs)),
            player_senders: HashMap::new(),
            blind_config,
            turn_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Send a message to a specific player.
    pub fn send_to_player(&self, player_id: u32, msg: &ServerMessage) {
        if let Some(tx) = self.player_senders.get(&player_id) {
            // Ignore send failure — the player may have just disconnected.
            let _ = tx.send(msg.clone());
        }
    }

    /// Broadcast a message to **all** connected players in this room.
    pub fn broadcast(&self, msg: &ServerMessage) {
        for tx in self.player_senders.values() {
            let _ = tx.send(msg.clone());
        }
    }

    /// Broadcast a message to all connected players **except** `exclude_id`.
    pub fn broadcast_except(&self, msg: &ServerMessage, exclude_id: u32) {
        for (&pid, tx) in &self.player_senders {
            if pid != exclude_id {
                let _ = tx.send(msg.clone());
            }
        }
    }
}

/// Manages all active rooms.
///
/// Thread-safe: the outer `RwLock` allows concurrent reads (e.g. looking up
/// rooms) while writes (create / remove) take exclusive access.  Each room
/// is individually `Mutex`-protected so independent rooms never contend.
pub struct RoomManager {
    rooms: RwLock<HashMap<String, Arc<Mutex<Room>>>>,
}

impl RoomManager {
    pub fn new() -> Self {
        Self {
            rooms: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new room with the given ID.
    ///
    /// Returns an error string if the room ID is invalid or already taken.
    pub async fn create_room(
        &self,
        room_id: &str,
        blind_config: BlindConfig,
    ) -> Result<(), String> {
        validate_room_id(room_id)?;

        let mut rooms = self.rooms.write().await;
        if rooms.contains_key(room_id) {
            return Err(format!("Room '{}' already exists", room_id));
        }
        rooms.insert(
            room_id.to_string(),
            Arc::new(Mutex::new(Room::new(blind_config))),
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
    /// Returns `(player_id, PlayerRx)` on success so the caller can wire
    /// up the WebSocket write loop.
    pub async fn join_room(
        &self,
        room_id: &str,
        player_name: &str,
    ) -> Result<(u32, usize, PlayerRx, Arc<Mutex<Room>>), String> {
        let room_arc = self
            .get_room(room_id)
            .await
            .ok_or_else(|| format!("Room '{}' not found", room_id))?;

        let mut room = room_arc.lock().await;

        // Lock game_state, validate, add player, then drop before
        // mutating player_senders to avoid overlapping borrows.
        let (player_id, player_count) = {
            let mut game_state = room.game_state.lock().await;
            if game_state.game_started {
                return Err("Game already in progress".to_string());
            }
            let player = game_state.add_player(player_name.to_string());
            (player.id, game_state.player_count())
        };

        let (tx, rx) = mpsc::unbounded_channel();
        room.player_senders.insert(player_id, tx);

        // Notify existing players about the new player.
        let join_msg = ServerMessage::PlayerJoined {
            player_id,
            name: player_name.to_string(),
        };
        room.broadcast_except(&join_msg, player_id);

        drop(room);

        Ok((player_id, player_count, rx, room_arc))
    }

    /// Remove a player from a room.
    ///
    /// If the room becomes empty, it is automatically cleaned up.
    pub async fn remove_player(&self, room_id: &str, player_id: u32) {
        let should_remove_room;
        {
            let rooms = self.rooms.read().await;
            let Some(room_arc) = rooms.get(room_id) else {
                return;
            };
            let mut room = room_arc.lock().await;

            // Remove sender and game-state entry.
            room.player_senders.remove(&player_id);
            {
                let mut gs = room.game_state.lock().await;
                gs.remove_player(player_id);
            }

            // Notify remaining players.
            let leave_msg = ServerMessage::PlayerLeft { player_id };
            room.broadcast(&leave_msg);

            should_remove_room = room.player_senders.is_empty();
        }

        if should_remove_room {
            let mut rooms = self.rooms.write().await;
            // Double-check: another player may have joined between the read
            // lock being dropped and the write lock being acquired.
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

    /// List active room IDs (for debugging / future API).
    pub async fn list_rooms(&self) -> Vec<String> {
        let rooms = self.rooms.read().await;
        rooms.keys().cloned().collect()
    }
}
