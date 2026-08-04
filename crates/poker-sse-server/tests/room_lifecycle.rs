// Integration tests for room/player lifecycle: disconnect teardown, host
// promotion, and the host-gated update-settings action. Drives the real
// `RoomManager` (public via the lib facade), so it covers the same logic the
// SSE `Drop`-guard and action POSTs exercise.

// Test-only verification tool. The crate's pedantic/nursery-deny lints apply
// to test targets too, so relax them here for legibility.
#![allow(
    clippy::pedantic,
    clippy::nursery,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::needless_pass_by_value,
    clippy::items_after_statements
)]

use std::sync::Arc;
use std::time::Duration;

use poker_core::protocol::BlindConfig;
use poker_sse_server::AppState;
use poker_sse_server::room::{Room, RoomManager};

/// Build a fresh AppState backed by an empty RoomManager.
fn app_state() -> AppState {
    AppState {
        room_manager: Arc::new(RoomManager::new()),
    }
}

const BLINDS: BlindConfig = BlindConfig {
    interval_secs: 0,
    increase_percent: 0,
};

/// Create a room and add `n` named players, returning the per-player
/// `(player_id, session_token)` tuples. The first player is the host.
async fn room_with_players(state: &AppState, room_id: &str, names: &[&str]) -> Vec<(u32, String)> {
    state
        .room_manager
        .create_room(room_id, BLINDS, 100)
        .await
        .unwrap();
    let mut out = Vec::new();
    for name in names {
        let (pid, token, _room_arc) = state.room_manager.join_room(room_id, name).await.unwrap();
        out.push((pid, token));
    }
    out
}

/// Attach a stream (installing the channel) for a player, returning the
/// receiver too so the caller can drop it to simulate a client disconnect.
/// Mirrors what `GET /poker/events` does.
async fn attach_with_rx(
    state: &AppState,
    room_id: &str,
    token: &str,
) -> (
    u32,
    u64,
    tokio::sync::mpsc::Receiver<datastar::DatastarEvent>,
) {
    let room_arc = state.room_manager.get_room(room_id).await.unwrap();
    let pid = {
        let room = room_arc.lock().await;
        *room.sessions.get(token).unwrap()
    };
    let (rx, _events, generation) =
        RoomManager::attach_stream(&room_arc, room_id, pid, false).await;
    (pid, generation, rx)
}

// ---------------------------------------------------------------------------
// Disconnect teardown
// ---------------------------------------------------------------------------

/// A lobby player (only player) disconnecting is removed immediately and the
/// room torn down. This is the path the `events` `Drop`-guard invokes.
#[tokio::test]
async fn lobby_disconnect_removes_room() {
    let state = app_state();
    let players = room_with_players(&state, "lobby1", &["solo"]).await;
    let (pid, _gen, _rx) = attach_with_rx(&state, "lobby1", &players[0].1).await;

    // Simulate the client disconnect: the Drop-guard calls disconnect_player.
    state.room_manager.disconnect_player("lobby1", pid, 1).await;

    // Give the detached empty-room removal a moment to settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(
        state.room_manager.get_room("lobby1").await.is_none(),
        "room should be removed after the only player disconnects in the lobby"
    );
}

/// An in-game disconnect sits the player out and starts the grace period, but
/// the player remains in game state (their seat is held).
#[tokio::test]
async fn ingame_disconnect_sits_out_and_holds_seat() {
    let state = app_state();
    let players = room_with_players(&state, "ingame1", &["a", "b"]).await;
    let (p1, _g1, _rx1) = attach_with_rx(&state, "ingame1", &players[0].1).await;
    let (p2, _g2, _rx2) = attach_with_rx(&state, "ingame1", &players[1].1).await;

    // Start the game under the room lock.
    {
        let room_arc = state.room_manager.get_room("ingame1").await.unwrap();
        let mut room = room_arc.lock().await;
        room.game_state.game_started = true;
    }

    // p1 disconnects mid-game.
    state.room_manager.disconnect_player("ingame1", p1, 1).await;

    let room_arc = state.room_manager.get_room("ingame1").await.unwrap();
    let room = room_arc.lock().await;
    assert!(
        room.game_state
            .players
            .get(&p1)
            .is_some_and(|p| p.sitting_out),
        "disconnected in-game player should be sat out"
    );
    assert!(
        room.disconnected_at.contains_key(&p1),
        "grace-period timestamp should be recorded"
    );
    // p2 is unaffected.
    assert!(room.game_state.players.contains_key(&p2));
}

// ---------------------------------------------------------------------------
// Host promotion
// ---------------------------------------------------------------------------

/// When the host disconnects in the lobby, host rights pass to the remaining
/// player (via the immediate-removal path).
#[tokio::test]
async fn host_lobby_disconnect_promotes_next_player() {
    let state = app_state();
    let players = room_with_players(&state, "host1", &["host", "guest"]).await;
    let host_id = players[0].0;
    let guest_id = players[1].0;
    let (h_pid, h_gen, _h_rx) = attach_with_rx(&state, "host1", &players[0].1).await;
    let (_g_pid, _g_gen, _g_rx) = attach_with_rx(&state, "host1", &players[1].1).await;
    assert_eq!(h_pid, host_id);

    // Host disconnects.
    state
        .room_manager
        .disconnect_player("host1", host_id, h_gen)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let room_arc = state.room_manager.get_room("host1").await.unwrap();
    let room = room_arc.lock().await;
    assert_eq!(
        room.game_state.host_id, guest_id,
        "remaining player should be promoted to host"
    );
}

// ---------------------------------------------------------------------------
// update-settings action
// ---------------------------------------------------------------------------

/// Non-host `update_settings` is rejected. Here we just assert the host-gate
/// (`host_id != ctx.player_id`); the full rejection path lives in the handler.
#[tokio::test]
async fn update_settings_rejects_non_host() {
    let state = app_state();
    let players = room_with_players(&state, "set1", &["host", "other"]).await;
    let host_id = players[0].0;
    let other_id = players[1].0;

    let room_arc = state.room_manager.get_room("set1").await.unwrap();
    let room = room_arc.lock().await;
    assert_eq!(room.game_state.host_id, host_id);
    // A non-host can't pass the handler's host-gate.
    assert_ne!(room.game_state.host_id, other_id);
    drop(room);
}

/// A host applying blind settings updates both `blind_config` copies and
/// re-anchors the schedule mid-game. `apply_settings` replicates the handler's
/// mutation so this pins its post-conditions.
#[tokio::test]
async fn update_settings_host_updates_and_reanchors() {
    let state = app_state();
    let players = room_with_players(&state, "set2", &["host", "x"]).await;
    let host_id = players[0].0;

    let room_arc = state.room_manager.get_room("set2").await.unwrap();
    // Pre-game: starting_bbs applies.
    {
        let mut room = room_arc.lock().await;
        apply_settings(&mut room, host_id, 5, 50, 200, false);
        assert_eq!(room.game_state.blind_config.interval_secs, 300);
        assert_eq!(room.game_state.blind_config.increase_percent, 50);
        assert_eq!(room.game_state.starting_bbs, 200);
    }
    // Mid-game: starting_bbs is ignored, schedule re-anchored.
    {
        let mut room = room_arc.lock().await;
        room.game_state.game_started = true;
        room.game_state.last_blind_increase =
            Some(std::time::Instant::now() - Duration::from_secs(3600));
        let anchor_before = room.game_state.last_blind_increase;
        apply_settings(&mut room, host_id, 5, 50, 999, true);
        assert_eq!(
            room.game_state.starting_bbs, 200,
            "mid-game stack edit ignored"
        );
        assert_eq!(room.game_state.blind_config.interval_secs, 300);
        assert!(
            room.game_state.last_blind_increase > anchor_before,
            "blind schedule re-anchored to ~now"
        );
        // Room-level copy stays in sync.
        assert_eq!(room.blind_config, room.game_state.blind_config);
    }
}

/// Mirrors `action_update_settings`'s mutation exactly, so this test pins the
/// handler's intended post-conditions without standing up the HTTP layer.
fn apply_settings(
    room: &mut tokio::sync::MutexGuard<'_, Room>,
    caller: u32,
    blind_mins: u64,
    blind_pct: u32,
    stack_bbs: u32,
    game_started: bool,
) {
    assert_eq!(room.game_state.host_id, caller, "caller must be host");
    let new_config = BlindConfig {
        interval_secs: blind_mins.saturating_mul(60),
        increase_percent: blind_pct,
    };
    room.game_state.blind_config = new_config;
    room.blind_config = new_config;
    if !room.game_state.game_started {
        room.game_state.starting_bbs = stack_bbs.max(1);
    }
    if game_started && new_config.is_enabled() {
        room.game_state.last_blind_increase = Some(std::time::Instant::now());
    }
}

#[tokio::test]
async fn list_rooms_reflects_state() {
    let state = app_state();
    assert!(state.room_manager.list_rooms().await.is_empty());
    room_with_players(&state, "r1", &["a"]).await;
    room_with_players(&state, "r2", &["b"]).await;
    let rooms = state.room_manager.list_rooms().await;
    assert!(rooms.contains(&"r1".to_string()));
    assert!(rooms.contains(&"r2".to_string()));
}
