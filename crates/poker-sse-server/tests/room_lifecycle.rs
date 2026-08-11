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

use poker_core::game_logic::BlindConfig;
use poker_sse_server::AppState;
use poker_sse_server::room::{LeaveOutcome, RoomManager};

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

/// When the **last** connected player disconnects mid-game, the room is now
/// held for a short [`LAST_PLAYER_GRACE_PERIOD`] (not torn down instantly).
/// The old behavior removed the room immediately, which broke rejoin-on-reload
/// in heads-up / small games: a reload mid-hand with no one else connected
/// destroyed the room before the token could re-attach. The short window covers
/// any realistic reload without holding a dead room hostage.
#[tokio::test]
async fn ingame_last_disconnect_holds_room_for_short_grace() {
    let state = app_state();
    let players = room_with_players(&state, "ingame2", &["solo"]).await;
    let (pid, _gen, _rx) = attach_with_rx(&state, "ingame2", &players[0].1).await;

    {
        let room_arc = state.room_manager.get_room("ingame2").await.unwrap();
        let mut room = room_arc.lock().await;
        room.game_state.game_started = true;
    }

    // The only connected player disconnects mid-game (simulating a reload).
    state
        .room_manager
        .disconnect_player("ingame2", pid, 1)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Within the grace window the room and seat must still exist — this is the
    // rejoin-on-reload window for heads-up / single-player games.
    let room_arc = state.room_manager.get_room("ingame2").await;
    assert!(
        room_arc.is_some(),
        "room should survive the last-player disconnect within the grace window"
    );
    let room_arc = room_arc.unwrap();
    let room = room_arc.lock().await;
    assert!(
        room.disconnected_at.contains_key(&pid),
        "a short grace-period timestamp should be recorded for the last player"
    );
    assert!(
        room.game_state.players.contains_key(&pid),
        "the disconnected player's seat should still be held"
    );
}

/// Reconnecting within the grace window (the reload round-trip) restores the
/// seat and cancels the pending removal. This is the core rejoin-on-reload
/// path for heads-up games that previously had no test coverage.
#[tokio::test]
async fn heads_up_reload_rejoins_within_grace() {
    let state = app_state();
    let players = room_with_players(&state, "hu1", &["solo"]).await;
    let pid = players[0].0;
    let token = players[0].1.clone();
    let (pid_attached, _gen1, _rx1) = attach_with_rx(&state, "hu1", &token).await;
    assert_eq!(pid_attached, pid);

    {
        let room_arc = state.room_manager.get_room("hu1").await.unwrap();
        let mut room = room_arc.lock().await;
        room.game_state.game_started = true;
    }

    // Player reloads: the SSE stream drops, then re-attaches with the same token.
    state.room_manager.disconnect_player("hu1", pid, 1).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    // The reload re-attaches the events stream before the short grace expires.
    let room_arc = state
        .room_manager
        .get_room("hu1")
        .await
        .expect("room must still exist right after a heads-up disconnect (short grace)");
    let (_rx2, _events, gen2) = RoomManager::attach_stream(&room_arc, "hu1", pid, false).await;

    let room_arc = state.room_manager.get_room("hu1").await.unwrap();
    let room = room_arc.lock().await;
    assert!(
        !room.disconnected_at.contains_key(&pid),
        "re-attach must cancel the grace-period removal"
    );
    assert_eq!(
        room.players.get(&pid).map(|c| c.generation),
        Some(gen2),
        "the re-attached connection is the current generation"
    );
    assert!(
        room.players.get(&pid).is_some_and(|c| c.tx.is_some()),
        "the player should have a live channel again"
    );
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
// Explicit leave (POST /poker/room/leave — "Exit Game")
// ---------------------------------------------------------------------------

/// Leaving from the lobby (game not started) removes the player immediately and
/// tears the room down if it's now empty. This is the deterministic
/// "hard-leave" path, distinct from a transient disconnect.
#[tokio::test]
async fn lobby_leave_removes_player_and_room() {
    let state = app_state();
    let players = room_with_players(&state, "leave1", &["solo"]).await;
    let (pid, _gen, _rx) = attach_with_rx(&state, "leave1", &players[0].1).await;

    let outcome = state.room_manager.leave_room("leave1", pid).await;
    assert_eq!(
        outcome,
        LeaveOutcome::RoomRemoved,
        "the last lobby player leaving should tear the room down"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        state.room_manager.get_room("leave1").await.is_none(),
        "room should be gone after the only player leaves"
    );
}

/// Leaving mid-hand does NOT remove the player immediately (mid-hand removal
/// would shift seat indices); instead they're sat out and flagged for removal
/// at the next hand boundary. They must not linger as a 5-minute ghost auto-
/// folding seat — the grace period does not apply to an explicit leave.
#[tokio::test]
async fn ingame_leave_sits_out_and_flags_for_boundary_removal() {
    let state = app_state();
    let players = room_with_players(&state, "leave2", &["a", "b"]).await;
    let (p1, _g1, _rx1) = attach_with_rx(&state, "leave2", &players[0].1).await;
    let (_p2, _g2, _rx2) = attach_with_rx(&state, "leave2", &players[1].1).await;

    {
        let room_arc = state.room_manager.get_room("leave2").await.unwrap();
        let mut room = room_arc.lock().await;
        room.game_state.game_started = true;
    }

    let outcome = state.room_manager.leave_room("leave2", p1).await;
    assert_eq!(
        outcome,
        LeaveOutcome::FoldedAndLeaving,
        "a mid-hand leave should flag the player, not remove them instantly"
    );

    let room_arc = state.room_manager.get_room("leave2").await.unwrap();
    let room = room_arc.lock().await;
    assert!(
        room.game_state
            .players
            .get(&p1)
            .is_some_and(|p| p.sitting_out),
        "leaving mid-hand should sit the player out"
    );
    assert!(
        room.players.get(&p1).is_some_and(|c| c.wants_leave),
        "leaving mid-hand should set wants_leave for boundary removal"
    );
    assert!(
        !room.disconnected_at.contains_key(&p1),
        "an explicit leave must not start a grace period (no ghost seat)"
    );
}

/// A leave from a room that no longer exists is reported as `RoomGone` and is
/// a no-op (e.g. a late/duplicate beacon arriving after teardown).
#[tokio::test]
async fn leave_when_room_gone_is_noop() {
    let state = app_state();
    let outcome = state.room_manager.leave_room("ghost", 1).await;
    assert_eq!(outcome, LeaveOutcome::RoomGone);
}

/// A duplicate leave (e.g. a second tab, or a beacon after the POST already
/// landed) is idempotent: a mid-hand leave keeps the player in state until the
/// hand boundary, so a second leave re-flags them (harmlessly) rather than
/// double-removing. Once the seat is actually gone, a further leave reports
/// `AlreadyLeft` (covered here via a lobby leave, which removes immediately).
#[tokio::test]
async fn duplicate_leave_is_idempotent() {
    let state = app_state();
    let players = room_with_players(&state, "leave3", &["a", "b"]).await;
    let (p1, _g1, _rx1) = attach_with_rx(&state, "leave3", &players[0].1).await;
    {
        let room_arc = state.room_manager.get_room("leave3").await.unwrap();
        let mut room = room_arc.lock().await;
        room.game_state.game_started = true;
    }

    // First mid-hand leave flags the player.
    let first = state.room_manager.leave_room("leave3", p1).await;
    assert_eq!(first, LeaveOutcome::FoldedAndLeaving);
    // Second leave is idempotent — the player is still seated (held for the
    // hand boundary), so this just re-asserts the flag.
    let second = state.room_manager.leave_room("leave3", p1).await;
    assert_eq!(
        second,
        LeaveOutcome::FoldedAndLeaving,
        "a second mid-hand leave re-flags harmlessly"
    );

    // A leave for a player who is already actually gone reports AlreadyLeft.
    // Keep another connected player so the room survives p2's lobby leave.
    let players2 = room_with_players(&state, "leave3b", &["x", "y"]).await;
    let (p2, _g2, _rx2) = attach_with_rx(&state, "leave3b", &players2[0].1).await;
    let (_p3, _g3, _rx3) = attach_with_rx(&state, "leave3b", &players2[1].1).await;
    state.room_manager.leave_room("leave3b", p2).await;
    // p2 is gone but the room survives; a beacon arriving after the leave:
    let late = state.room_manager.leave_room("leave3b", p2).await;
    assert_eq!(
        late,
        LeaveOutcome::AlreadyLeft,
        "a leave after the player is gone (room still alive) should report AlreadyLeft"
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

/// The strict-raises toggle is host-gated like the other host settings. Here
/// we just assert the host-gate (`host_id != ctx.player_id`) and the default
/// mode; the full rejection path lives in the handler, the floor semantics in
/// the engine tests.
#[tokio::test]
async fn strict_raises_toggle_rejects_non_host() {
    let state = app_state();
    let players = room_with_players(&state, "strict1", &["host", "other"]).await;
    let host_id = players[0].0;
    let other_id = players[1].0;

    let room_arc = state.room_manager.get_room("strict1").await.unwrap();
    let room = room_arc.lock().await;
    assert_eq!(room.game_state.host_id, host_id);
    // A non-host can't pass the handler's host-gate.
    assert_ne!(room.game_state.host_id, other_id);
    // Casual min-raise rules (floor always one BB) are the default.
    assert!(!room.game_state.strict_raises);
}

/// A host applying blind settings updates both `blind_config` copies and
/// re-anchors the schedule mid-game. Calls the real `GameState::apply_settings`
/// (the handler's mutation) directly, so this pins its post-conditions.
#[tokio::test]
async fn update_settings_host_updates_and_reanchors() {
    let state = app_state();
    let players = room_with_players(&state, "set2", &["host", "x"]).await;
    let host_id = players[0].0;
    let room_arc = state.room_manager.get_room("set2").await.unwrap();

    assert_eq!(room_arc.lock().await.game_state.host_id, host_id);

    // Pre-game: starting_bbs applies.
    {
        let mut room = room_arc.lock().await;
        let config = BlindConfig {
            interval_secs: 300,
            increase_percent: 50,
        };
        room.game_state.apply_settings(config, 200);
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
        let config = BlindConfig {
            interval_secs: 300,
            increase_percent: 50,
        };
        room.game_state.apply_settings(config, 999);
        assert_eq!(
            room.game_state.starting_bbs, 200,
            "mid-game stack edit ignored"
        );
        assert_eq!(room.game_state.blind_config.interval_secs, 300);
        assert!(
            room.game_state.last_blind_increase > anchor_before,
            "blind schedule re-anchored to ~now"
        );
    }
}

/// Pre-game, raising the starting stack must rebuy already-seated players to
/// match what a newly joining player would receive. This is the regression
/// where `starting_bbs` was updated but existing players' chips were left at
/// the stale join-time buy-in.
#[tokio::test]
async fn update_settings_pre_game_rebuys_existing_players() {
    let state = app_state();
    // `room_with_players` uses `create_room(.., 100)` BBs. With the default
    // 20 big blind that's 2000 chips per seated player.
    let players = room_with_players(&state, "set3", &["host", "p2", "p3"]).await;

    let room_arc = state.room_manager.get_room("set3").await.unwrap();
    {
        let room = room_arc.lock().await;
        let expected = room.game_state.starting_bbs * room.game_state.big_blind;
        assert_eq!(expected, 2_000);
        for pid in [players[0].0, players[1].0, players[2].0] {
            assert_eq!(
                room.game_state.players.get(&pid).unwrap().chips,
                expected,
                "players should start at the original buy-in"
            );
        }
    }

    // Host raises the stack to 300 BBs (pre-game).
    {
        let mut room = room_arc.lock().await;
        let config = BlindConfig {
            interval_secs: 300,
            increase_percent: 50,
        };
        room.game_state.apply_settings(config, 300);
    }

    let expected_new = 300 * 20;
    {
        let room = room_arc.lock().await;
        assert_eq!(room.game_state.starting_bbs, 300);
        for pid in [players[0].0, players[1].0, players[2].0] {
            assert_eq!(
                room.game_state.players.get(&pid).unwrap().chips,
                expected_new,
                "existing players must be rebought at the new stack"
            );
        }
    }

    // A player joining after the change lands on the same stack, proving the
    // existing players now match new joiners.
    let (new_pid, _token, _) = state.room_manager.join_room("set3", "late").await.unwrap();
    let room = room_arc.lock().await;
    assert_eq!(
        room.game_state.players.get(&new_pid).unwrap().chips,
        expected_new,
        "a new joiner should match the rebought existing players"
    );
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

// ---------------------------------------------------------------------------
// Regression: ghost seats from "Exit Game" while paused / at a hand boundary
// ---------------------------------------------------------------------------
//
// Bug: an "Exit Game" that ended a hand and dropped the room below 2 active
// (→ pause) left a permanent `wants_leave` "(away)" ghost, because the
// hand-boundary sweep sat *after* the pause early-return and never ran. Each
// rejoin stacked a fresh seat on top → player list grew without bound.

/// An explicit leave while the game is *paused* (`waiting_for_players`) is
/// removed immediately, like the lobby path — there is no live betting loop to
/// defer for, and no `start_new_hand` would ever sweep the seat. Before the
/// fix this stacked a permanent ghost on every exit-while-paused.
#[tokio::test]
async fn leave_while_paused_removes_immediately() {
    let state = app_state();
    let players = room_with_players(&state, "pause1", &["a", "b"]).await;
    let (p1, _g1, _rx1) = attach_with_rx(&state, "pause1", &players[0].1).await;
    let (p2, _g2, _rx2) = attach_with_rx(&state, "pause1", &players[1].1).await;

    {
        let room_arc = state.room_manager.get_room("pause1").await.unwrap();
        let mut room = room_arc.lock().await;
        room.game_state.game_started = true;
        // Game started but paused (not enough active players to deal).
        room.game_state.waiting_for_players = true;
    }

    // p1 exits. With no live hand, this is the lobby path: remove now.
    let outcome = state.room_manager.leave_room("pause1", p1).await;
    assert_eq!(
        outcome,
        LeaveOutcome::Left,
        "a leave while paused (no live hand) should remove immediately, not defer"
    );

    let room_arc = state.room_manager.get_room("pause1").await.unwrap();
    let room = room_arc.lock().await;
    assert!(
        !room.players.contains_key(&p1),
        "p1 must be fully removed, not a lingering ghost"
    );
    assert!(
        !room.game_state.players.contains_key(&p1),
        "p1 must be gone from game state too"
    );
    assert!(room.players.contains_key(&p2));
}

/// The hand-boundary sweep runs *before* the pause decision, so a mid-hand
/// leave that ends the hand and drops the room to < 2 active still reclaims
/// the seat at the boundary (→ pause, but a clean one). This is the path the
/// unbounded-ghost bug went through.
#[tokio::test]
async fn boundary_sweep_reclaims_leaver_even_when_result_is_pause() {
    let state = app_state();
    let players = room_with_players(&state, "sweep1", &["a", "b"]).await;
    let (p1, _g1, _rx1) = attach_with_rx(&state, "sweep1", &players[0].1).await;
    let (p2, _g2, _rx2) = attach_with_rx(&state, "sweep1", &players[1].1).await;

    {
        let room_arc = state.room_manager.get_room("sweep1").await.unwrap();
        let mut room = room_arc.lock().await;
        room.game_state.game_started = true;
        // p1 left mid-hand: sat out and flagged for boundary removal.
        room.game_state.set_sitting_out(p1);
        room.players.get_mut(&p1).unwrap().wants_leave = true;
    }

    // The hand boundary fires. Even though only p2 is active (→ pause), the
    // sweep must run first and reclaim p1's seat.
    let room_arc = state.room_manager.get_room("sweep1").await.unwrap();
    {
        let mut room = room_arc.lock().await;
        assert!(
            poker_sse_server::flow::sweep_leavers(&mut room, "sweep1"),
            "the sweep should reclaim the flagged leaver"
        );
    }

    let room = room_arc.lock().await;
    assert!(
        !room.players.contains_key(&p1),
        "p1 must be swept at the boundary, not left as a ghost"
    );
    assert!(
        room.game_state.waiting_for_players || room.players.contains_key(&p2),
        "p2 survives; the room is paused or waiting with the remaining player"
    );
    assert!(room.players.contains_key(&p2));
}

/// The full end-to-end reproduction: exit → rejoin must not stack a duplicate
/// seat. After the fix, the exit is cleaned up (here via the paused-game
/// immediate path) so the rejoin is the only seat — the player list stays at
/// two, not three.
#[tokio::test]
async fn exit_then_rejoin_does_not_stack_seat() {
    let state = app_state();
    let players = room_with_players(&state, "stack1", &["host", "p2"]).await;
    let (host, _gh, _rxh) = attach_with_rx(&state, "stack1", &players[0].1).await;
    let (p2, _g2, _rx2) = attach_with_rx(&state, "stack1", &players[1].1).await;

    {
        let room_arc = state.room_manager.get_room("stack1").await.unwrap();
        let mut room = room_arc.lock().await;
        room.game_state.game_started = true;
        room.game_state.allow_late_entry = true;
        room.game_state.waiting_for_players = true; // paused
    }

    // p2 exits to the connection screen, then rejoins via late entry.
    state.room_manager.leave_room("stack1", p2).await;
    let (p2b, _token_b, _) = state
        .room_manager
        .join_room("stack1", "p2")
        .await
        .expect("rejoin should succeed (late entry is on)");

    let room_arc = state.room_manager.get_room("stack1").await.unwrap();
    let room = room_arc.lock().await;
    assert!(
        !room.players.contains_key(&p2),
        "the original p2 seat must be gone, not lingering as a ghost"
    );
    assert!(
        room.players.contains_key(&p2b),
        "the rejoined p2 seat exists"
    );
    assert!(room.players.contains_key(&host), "the host is unaffected");
    assert_eq!(
        room.players.len(),
        2,
        "exactly two seats after rejoin — no stacked ghost (the bug grew this to 3+)"
    );
    assert_eq!(
        room.game_state.player_count(),
        2,
        "game-state seat count must also stay at 2"
    );
}
