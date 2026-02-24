//! Room manager for the multi-room poker server.
//!
//! Each room contains an independent [`GameState`] and a set of connected
//! players, each with their own [`mpsc`] sender for targeted message delivery
//! (no broadcast fan-out of private data).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use crate::game_logic::{GamePhase, GameState, PlayerStatus};
use poker_core::protocol::{
    BlindConfig, CardInfo, PlayerInfo, ServerMessage, card_to_info, validate_room_id,
};
use tokio::sync::{Mutex, RwLock, mpsc};

/// How long a disconnected player's seat is held before permanent removal.
const SESSION_GRACE_PERIOD: Duration = Duration::from_secs(5 * 60); // 5 minutes

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
            game_state: Arc::new(Mutex::new(gs)),
            player_senders: HashMap::new(),
            blind_config,
            turn_counter: Arc::new(AtomicU64::new(0)),
            sessions: HashMap::new(),
            player_sessions: HashMap::new(),
            disconnected_at: HashMap::new(),
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

    /// Register a session token for a player.
    pub fn register_session(&mut self, player_id: u32, token: String) {
        self.sessions.insert(token.clone(), player_id);
        self.player_sessions.insert(player_id, token);
    }

    /// Build a full state snapshot [`ServerMessage::Rejoined`] for a
    /// reconnecting player.
    pub fn build_rejoin_snapshot(
        &self,
        gs: &GameState,
        room_id: &str,
        player_id: u32,
        session_token: &str,
    ) -> ServerMessage {
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
            GamePhase::Lobby => "Waiting",
            GamePhase::PreFlop => "Preflop",
            GamePhase::Flop => "Flop",
            GamePhase::Turn => "Turn",
            GamePhase::River => "River",
            GamePhase::Showdown => "Showdown",
        }
        .to_string();

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
        starting_bbs: u32,
    ) -> Result<(), String> {
        validate_room_id(room_id)?;

        let mut rooms = self.rooms.write().await;
        if rooms.contains_key(room_id) {
            return Err(format!("Room '{}' already exists", room_id));
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
    ) -> Result<(u32, String, usize, PlayerRx, Arc<Mutex<Room>>), String> {
        let room_arc = self
            .get_room(room_id)
            .await
            .ok_or_else(|| format!("Room '{}' not found", room_id))?;

        let mut room = room_arc.lock().await;

        // Lock game_state, validate, add player, then drop before
        // mutating player_senders to avoid overlapping borrows.
        let (player_id, player_count) = {
            let mut game_state = room.game_state.lock().await;
            if game_state.game_started && !game_state.allow_late_entry {
                return Err("Game already in progress".to_string());
            }
            let player = if game_state.game_started {
                // Late entry: give the frozen starting chip amount.
                let chips = game_state.starting_chips;
                let p = game_state.add_player_with_chips(player_name.to_string(), Some(chips));
                // Late-joiners sit out until the next hand.
                game_state.set_sitting_out(p.id);
                p
            } else {
                game_state.add_player(player_name.to_string())
            };
            // First player to join becomes the host.
            if game_state.host_id == 0 {
                game_state.host_id = player.id;
            }
            (player.id, game_state.player_count())
        };

        let session_token = generate_session_token();
        room.register_session(player_id, session_token.clone());

        let (tx, rx) = mpsc::unbounded_channel();
        room.player_senders.insert(player_id, tx);

        // Notify existing players about the new player.
        let join_msg = ServerMessage::PlayerJoined {
            player_id,
            name: player_name.to_string(),
        };
        room.broadcast_except(&join_msg, player_id);

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
    ) -> Result<(u32, PlayerRx, Arc<Mutex<Room>>), String> {
        let room_arc = self
            .get_room(room_id)
            .await
            .ok_or_else(|| format!("Room '{}' not found", room_id))?;

        let mut room = room_arc.lock().await;

        let player_id = *room
            .sessions
            .get(session_token)
            .ok_or_else(|| "Invalid or expired session token".to_string())?;

        // Verify the player still exists in game state.
        let player_exists = {
            let gs = room.game_state.lock().await;
            gs.players.contains_key(&player_id)
        };
        if !player_exists {
            // Token was valid but player was already fully removed.
            room.sessions.remove(session_token);
            room.player_sessions.remove(&player_id);
            return Err("Session expired — player was removed".to_string());
        }

        // Clear the disconnected-at timestamp (cancel grace period).
        room.disconnected_at.remove(&player_id);

        // Replace the sender channel.
        let (tx, rx) = mpsc::unbounded_channel();
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

        let game_in_progress = {
            let mut gs = room.game_state.lock().await;
            if gs.game_started && gs.players.contains_key(&player_id) {
                // Sit the player out so auto-check/fold kicks in.
                if !gs
                    .players
                    .get(&player_id)
                    .map(|p| p.sitting_out)
                    .unwrap_or(true)
                {
                    gs.set_sitting_out(player_id);
                    room.broadcast(&ServerMessage::PlayerSatOut { player_id });
                }
                true
            } else {
                false
            }
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
                        {
                            let mut gs = room.game_state.lock().await;
                            gs.remove_player(player_id);
                        }
                        room.broadcast(&ServerMessage::PlayerLeft { player_id });
                        tracing::info!(
                            room = %rid,
                            player = player_id,
                            "Grace period expired — player permanently removed"
                        );
                    }
                });
            }
        } else {
            // Game hasn't started — remove immediately.
            if let Some(token) = room.player_sessions.remove(&player_id) {
                room.sessions.remove(&token);
            }
            {
                let mut gs = room.game_state.lock().await;
                gs.remove_player(player_id);
            }
            room.broadcast(&ServerMessage::PlayerLeft { player_id });

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
