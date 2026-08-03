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

//! Room manager for the Datastar (SSE) poker server.
//!
//! Each room is `Mutex`-protected; per-player channels carry rendered
//! [`DatastarEvent`]s. SSE is the only channel that pushes UI, so ordering is
//! per-connection consistent.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use datastar::DatastarEvent;
use poker_core::protocol::{BlindConfig, RoomErrorKind, RoomIdError, validate_room_id};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock, mpsc};

use poker_core::game_logic::GameState;
use crate::render;

/// How long a disconnected player's seat is held before permanent removal.
const SESSION_GRACE_PERIOD: Duration = Duration::from_secs(5 * 60); // 5 minutes

/// Max players per room. A 52-card deck caps two-card hands at 23, but real
/// tables seat 9–10. Capping here also prevents a deck-exhaustion panic in
/// `start_new_hand`.
const MAX_PLAYERS_PER_ROOM: usize = 9;

/// Max concurrent rooms. Bounds memory against a client looping `CreateRoom`.
const MAX_ACTIVE_ROOMS: usize = 100;

/// Maximum number of outbound SSE events buffered per player before the
/// connection is considered too slow and the sender is dropped.
const PLAYER_CHANNEL_CAPACITY: usize = 256;

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
                RoomIdError::Empty => Self::RoomIdEmpty,
                RoomIdError::TooLong => Self::RoomIdTooLong,
                RoomIdError::InvalidChars => Self::RoomIdInvalidChars,
            },
            RoomError::ServerFull => Self::ServerFull,
            RoomError::RoomAlreadyExists(id) => Self::RoomAlreadyExists {
                room_id: id.clone(),
            },
            RoomError::RoomNotFound(id) => Self::RoomNotFound {
                room_id: id.clone(),
            },
            RoomError::RoomFull => Self::RoomFull,
            RoomError::GameInProgress => Self::GameInProgress,
            RoomError::InvalidSession => Self::InvalidSession,
            RoomError::SessionExpired => Self::SessionExpired,
        }
    }
}

/// Handle to a per-player outbound channel of rendered SSE events.
///
/// The `/poker/events` stream drains this receiver and forwards each event
/// as an SSE frame.  The channel is **bounded** so that a slow client
/// cannot cause unbounded memory growth on the server.
pub type PlayerTx = mpsc::Sender<DatastarEvent>;
pub type PlayerRx = mpsc::Receiver<DatastarEvent>;

/// One player's connection state inside a room.
pub struct PlayerConn {
    /// Current SSE delivery channel; `None` while offline (within the grace
    /// period).
    pub tx: Option<PlayerTx>,
    /// Monotonic generation, bumped on each attach. A stale teardown (from an
    /// older connection) must not clear the current `tx`.
    pub generation: u64,
}

/// A single poker room.
pub struct Room {
    /// Server-side game state (deck, hands, betting, etc.).
    pub game_state: GameState,
    /// Per-player connection state keyed by player ID.
    pub players: HashMap<u32, PlayerConn>,
    /// Blind increase configuration for this room.
    pub blind_config: BlindConfig,
    /// Monotonically increasing counter incremented every time a new turn
    /// starts.  Used to invalidate stale turn-timer tasks.
    pub turn_counter: Arc<AtomicU64>,
    /// When the current turn's timer started.  Used to render the
    /// *remaining* countdown in snapshots (join / reconnect).
    pub turn_started_at: Option<Instant>,
    /// Maps session tokens to player IDs for reconnection.
    pub sessions: HashMap<String, u32>,
    /// Maps player IDs to session tokens (reverse lookup).
    pub player_sessions: HashMap<u32, String>,
    /// Tracks when disconnected players should be permanently removed.
    pub disconnected_at: HashMap<u32, Instant>,
    /// Set once the room has broadcast `GameOver`; the next SSE attach
    /// tears the room down so a finished room can't be rejoined.
    pub game_over: bool,
}

impl Room {
    fn new(blind_config: BlindConfig, starting_bbs: u32) -> Self {
        let mut gs = GameState::new();
        gs.blind_config = blind_config;
        gs.starting_bbs = starting_bbs;
        Self {
            game_state: gs,
            players: HashMap::new(),
            blind_config,
            turn_counter: Arc::new(AtomicU64::new(0)),
            turn_started_at: None,
            sessions: HashMap::new(),
            player_sessions: HashMap::new(),
            disconnected_at: HashMap::new(),
            game_over: false,
        }
    }

    /// Register a session token for a player.
    pub fn register_session(&mut self, player_id: u32, token: String) {
        self.sessions.insert(token.clone(), player_id);
        self.player_sessions.insert(player_id, token);
    }
}

/// Fanout hub: routes rendered [`DatastarEvent`]s to per-player channels.
///
/// Constructed from the room under the room lock, used, and dropped — never
/// stored.
pub struct Fanout<'a> {
    room: &'a mut Room,
}

impl<'a> Fanout<'a> {
    pub const fn new(room: &'a mut Room) -> Self {
        Self { room }
    }

    /// Send rendered events to one player. Delivered only while the player has
    /// a live channel; an offline player (within the grace period) receives
    /// nothing until they reattach, at which point a fresh full snapshot is
    /// rendered — strictly more consistent than any diff backlog.
    pub fn send_to(&mut self, player_id: u32, events: &[DatastarEvent]) {
        if events.is_empty() {
            return;
        }
        if let Some(conn) = self.room.players.get_mut(&player_id)
            && let Some(tx) = &conn.tx
        {
            for ev in events {
                match tx.try_send(clone_event(ev)) {
                    Ok(()) => {}
                    // Transient backpressure: the stream is alive but full. Skip
                    // this event and keep the sender — the next settled
                    // broadcast re-renders from GameState, so a dropped frame
                    // self-heals; severing the sender would not.
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!(
                            player = player_id,
                            "Player channel full — dropping one event frame"
                        );
                    }
                    // Receiver gone: the SSE stream is closed. Drop the sender so
                    // teardown/grace-period logic can reclaim the seat.
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::warn!(
                            player = player_id,
                            "Player channel closed — dropping sender"
                        );
                        conn.tx = None;
                        return;
                    }
                }
            }
        }
    }
}

/// Clone a [`DatastarEvent`] (the SDK type isn't `Clone`). The exhaustive
/// match is deliberate: the SDK has two variants (`PatchElements`,
/// `PatchSignals`; `ExecuteScript` is a `PatchElements` event under the hood).
/// A future third variant will fail to compile here, forcing an explicit clone
/// decision rather than silently falling through.
fn clone_event(ev: &DatastarEvent) -> DatastarEvent {
    DatastarEvent {
        event: match ev.event {
            datastar::consts::EventType::PatchElements => {
                datastar::consts::EventType::PatchElements
            }
            datastar::consts::EventType::PatchSignals => datastar::consts::EventType::PatchSignals,
        },
        id: ev.id.clone(),
        retry: ev.retry,
        data: ev.data.clone(),
    }
}

/// Manages all active rooms. The outer `RwLock` allows concurrent reads while
/// writes (create / remove) take exclusive access; each room is individually
/// `Mutex`-protected, so independent rooms never contend.
pub struct RoomManager {
    rooms: Arc<RwLock<HashMap<String, Arc<Mutex<Room>>>>>,
}

impl Default for RoomManager {
    fn default() -> Self {
        Self::new()
    }
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

        // Hold the write guard only across the map mutation.
        {
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
        }
        Ok(())
    }

    /// Look up a room by ID.
    pub async fn get_room(&self, room_id: &str) -> Option<Arc<Mutex<Room>>> {
        let rooms = self.rooms.read().await;
        rooms.get(room_id).cloned()
    }

    /// Remove a room entirely (used after `GameOver`).
    pub async fn remove_room(&self, room_id: &str) {
        let mut rooms = self.rooms.write().await;
        rooms.remove(room_id);
    }

    /// Add a player to a room.
    ///
    /// Returns `(player_id, session_token, Arc<Mutex<Room>>)` on success.
    /// Unlike the WS server no channel is created here — that happens when
    /// the player's SSE stream attaches.
    pub async fn join_room(
        &self,
        room_id: &str,
        player_name: &str,
    ) -> Result<(u32, String, Arc<Mutex<Room>>), RoomError> {
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

        let session_token = generate_session_token();
        room.register_session(player_id, session_token.clone());

        room.players.insert(
            player_id,
            PlayerConn {
                tx: None,
                generation: 0,
            },
        );

        // Notify existing players about the new player. Rendered per viewer so
        // each still sees themselves highlighted.
        crate::handlers::broadcast_state(&mut room, room_id);

        drop(room);

        Ok((player_id, session_token, room_arc))
    }

    /// Validate a session token and return the room + player it maps to.
    pub fn lookup_session(room: &Room, session_token: &str) -> Result<u32, RoomError> {
        Ok(*room
            .sessions
            .get(session_token)
            .ok_or(RoomError::InvalidSession)?)
    }

    /// Attach an SSE stream for the player: installs a fresh channel and
    /// returns the receiver, a full state snapshot, and a connection
    /// generation.
    ///
    /// Always attaches (last-tab-wins): a new attach replaces any existing
    /// channel, orphaning the old tab's receiver (its stream ends, its teardown
    /// is a no-op via the generation guard). No single-session enforcement is
    /// attempted here — reliably distinguishing a duplicate tab from a
    /// legitimate reconnect isn't feasible at this layer, and the strategies
    /// tried (evict+reload, evict+no-reload, reject) all thrashed or broke
    /// reconnects.
    ///
    /// The returned generation must be presented at teardown; a stale teardown
    /// is a no-op and leaves the current connection alone.
    pub async fn attach_stream(
        room_arc: &Arc<Mutex<Room>>,
        room_id: &str,
        player_id: u32,
        _was_offline: bool,
    ) -> (PlayerRx, Vec<DatastarEvent>, u64) {
        let mut room = room_arc.lock().await;

        let (tx, rx) = mpsc::channel(PLAYER_CHANNEL_CAPACITY);
        let new_gen = room
            .players
            .get(&player_id)
            .map_or(0, |c| c.generation.wrapping_add(1));

        room.disconnected_at.remove(&player_id);
        let ctx = crate::handlers::ctx_of(&room, room_id);
        let events = render::full_snapshot(ctx, player_id);

        if let Some(conn) = room.players.get_mut(&player_id) {
            conn.tx = Some(tx);
            conn.generation = new_gen;
        }
        drop(room);

        (rx, events, new_gen)
    }

    /// Detach the SSE stream for a player (channel closed by the stream task).
    /// Starts the disconnect grace period if a game is running. A stale
    /// teardown (newer attach has replaced this connection) is a no-op.
    pub async fn detach_stream(&self, room_id: &str, player_id: u32, generation: u64) {
        self.disconnect_player(room_id, player_id, generation).await;
    }

    /// Soft-disconnect a player during a game: sit them out and start a grace
    /// period (game state preserved). A stale teardown is a no-op.
    pub async fn disconnect_player(&self, room_id: &str, player_id: u32, generation: u64) {
        let rooms = self.rooms.read().await;
        let Some(room_arc) = rooms.get(room_id).cloned() else {
            return;
        };
        drop(rooms);

        let mut room = room_arc.lock().await;
        // A stale teardown must be a complete no-op: not just the `tx` clear,
        // but the sit-out, grace period, and room removal below — otherwise the
        // evicted tab's teardown would later clobber the new tab's seat.
        let is_current = room
            .players
            .get(&player_id)
            .is_some_and(|c| c.generation == generation);
        if !is_current {
            return;
        }
        if let Some(conn) = room.players.get_mut(&player_id) {
            conn.tx = None;
        }

        let game_in_progress =
            if room.game_state.game_started && room.game_state.players.contains_key(&player_id) {
                // Sit the player out so auto-check/fold kicks in (only if they
                // aren't already sitting out).
                if matches!(
                    room.game_state.players.get(&player_id),
                    Some(p) if !p.sitting_out
                ) {
                    room.game_state.set_sitting_out(player_id);
                    crate::handlers::broadcast_state(&mut room, room_id);
                }
                true
            } else {
                false
            };

        if game_in_progress {
            self.start_grace_period(room, room_id, player_id).await;
        } else {
            // Game hasn't started — remove immediately.
            if let Some(token) = room.player_sessions.remove(&player_id) {
                room.sessions.remove(&token);
            }
            room.players.remove(&player_id);
            room.game_state.remove_player(player_id);
            crate::handlers::broadcast_state(&mut room, room_id);

            let any_connected = room.players.values().any(|c| c.tx.is_some());
            drop(room);

            if !any_connected {
                remove_room_if_empty(&self.rooms, room_id).await;
            }
        }
    }

    /// Start (and spawn) the disconnect grace-period countdown. Holds no lock
    /// after returning — the removal runs in a detached task after
    /// [`SESSION_GRACE_PERIOD`] elapses. Factored out of
    /// [`disconnect_player`] to keep that handler readable.
    #[allow(clippy::too_many_lines)]
    async fn start_grace_period(
        &self,
        mut room: tokio::sync::MutexGuard<'_, Room>,
        room_id: &str,
        player_id: u32,
    ) {
        // Start the grace-period countdown; the player stays in game state.
        room.disconnected_at.insert(player_id, Instant::now());
        tracing::info!(
            room = room_id,
            player = player_id,
            "Player disconnected — seat held for {:?}",
            SESSION_GRACE_PERIOD,
        );

        let rm = self_ref(room_id, &self.rooms).await;
        let rid = room_id.to_string();
        let grace = SESSION_GRACE_PERIOD;
        let rooms_ref = Arc::clone(&self.rooms);
        drop(room);

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
                    room.players.remove(&player_id);
                    room.game_state.remove_player(player_id);
                    crate::handlers::broadcast_state(&mut room, &rid);
                    tracing::info!(
                        room = %rid,
                        player = player_id,
                        "Grace period expired — player permanently removed"
                    );

                    // If no connected players remain, remove the room
                    // entirely so it doesn't leak memory.
                    let any_connected = room.players.values().any(|c| c.tx.is_some());
                    if !any_connected {
                        drop(room);
                        remove_room_if_empty_owned(rooms_ref, &rid).await;
                    }
                }
            });
        }
    }

    /// List active room IDs (for debugging / future API).
    pub async fn list_rooms(&self) -> Vec<String> {
        let rooms = self.rooms.read().await;
        rooms.keys().cloned().collect()
    }
}

/// Re-check (under the room lock) that a room is still empty, then drop it.
/// Guards against a reconnect landing between the outer check and the removal.
async fn remove_room_if_empty(
    rooms: &RwLock<HashMap<String, Arc<Mutex<Room>>>>,
    room_id: &str,
) {
    let mut rooms = rooms.write().await;
    if let Some(room_arc) = rooms.get(room_id) {
        let room = room_arc.lock().await;
        let still_empty = !room.players.values().any(|c| c.tx.is_some());
        if still_empty {
            drop(room);
            rooms.remove(room_id);
            drop(rooms);
            tracing::info!(room_id, "Removed empty room");
        }
    }
}

/// Owned-`Arc` variant of [`remove_room_if_empty`] for use from detached
/// tasks that can't borrow the `RoomManager`.
async fn remove_room_if_empty_owned(
    rooms: Arc<RwLock<HashMap<String, Arc<Mutex<Room>>>>>,
    room_id: &str,
) {
    let mut rooms = rooms.write().await;
    if let Some(room_arc) = rooms.get(room_id) {
        let r = room_arc.lock().await;
        let still_empty = !r.players.values().any(|c| c.tx.is_some());
        if still_empty {
            drop(r);
            rooms.remove(room_id);
            drop(rooms);
            tracing::info!(room = %room_id, "Removed empty room after grace period");
        }
    }
}

/// Generate a random session token (32-char hex string).
fn generate_session_token() -> String {
    use rand::RngExt;
    use std::fmt::Write;
    let mut rng = rand::rng();
    let bytes: [u8; 16] = rng.random();
    let mut out = String::with_capacity(32);
    for b in bytes {
        // {b:02x} is infallible for a `String` formatter, so discarding the
        // `fmt::Result` is safe here.
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Helper: get an `Arc<Mutex<Room>>` reference from `rooms` `RwLock`.
async fn self_ref(
    room_id: &str,
    rooms: &RwLock<HashMap<String, Arc<Mutex<Room>>>>,
) -> Option<Arc<Mutex<Room>>> {
    let rooms = rooms.read().await;
    rooms.get(room_id).cloned()
}

/// Everything an action POST handler needs to know about the caller.
pub struct CallerCtx {
    pub room_arc: Arc<Mutex<Room>>,
    pub player_id: u32,
    pub room_id: String,
}

/// Resolve the caller from `room_id` + `session_token` signals.
///
/// Returns an error string suitable for a log line on failure.
pub async fn resolve_caller(
    manager: &RoomManager,
    room_id: &str,
    session_token: &str,
) -> Result<CallerCtx, String> {
    let Some(room_arc) = manager.get_room(room_id).await else {
        return Err(format!("Room '{room_id}' not found"));
    };
    // Hold the room lock only for the lookup, then release before
    // constructing the cheap `CallerCtx`.
    let pid = {
        let room = room_arc.lock().await;
        match RoomManager::lookup_session(&room, session_token) {
            Ok(pid) if room.game_state.players.contains_key(&pid) => pid,
            Ok(_) => return Err("Session expired — player was removed".to_string()),
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok(CallerCtx {
        room_arc,
        player_id: pid,
        room_id: room_id.to_string(),
    })
}
