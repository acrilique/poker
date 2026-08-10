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

//! Game-flow orchestration: the post-action state machine, turn timers, and
//! the all-in run-out.
//!
//! Each action mutates the room's [`GameState`] via
//! [`GameState::apply_action`] (`poker-core`), then drives the transport-side
//! post-action loop (phase advance, hand resolution, next hand, turn timer,
//! fanout) until the game reaches a stable wait point. This is the CQRS write
//! side's "what happens after an action" logic, separated from HTTP extraction
//! (see [`crate::handlers`]) and SSE plumbing (see [`crate::sse`]).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use poker_core::game_logic::{BlindConfig, GamePhase, PlayerAction, TURN_TIMEOUT_SECS};
use poker_core::poker::{Card, Hand};
use tokio::sync::Mutex;

use crate::fanout::{broadcast_state, send_all, send_error};
use crate::render;
use crate::room::{CallerCtx, Room, remove_player_now};

// ---------------------------------------------------------------------------
// Post-action state machine
// ---------------------------------------------------------------------------

/// One step of the post-action loop (see [`drive_post_action`]). The loop
/// advances phases, resolves hands, and starts new hands until the game
/// reaches a stable wait point — a live turn, a pause (waiting for players),
/// or the all-in run-out handoff.
enum PostAction {
    /// Reached a live turn (or paused / game over). The caller is done.
    Done,
    /// Betting closed with only all-in players remaining. The caller must
    /// release the room lock, broadcast the all-in showdown, and run out the
    /// board. How the caller continues afterwards differs (see
    /// [`process_action_with_room`] vs [`run_out_board`]), which is why this
    /// isn't handled inside [`drive_post_action`].
    AllIn,
}

/// Drive the post-action state machine to its next stable wait point, starting
/// from the currently-locked settled state.
///
/// Renders state only at terminal points (a live turn via
/// [`notify_turn_and_start_timer`], a resolve, a phase advance, or a pause) —
/// never mid-transition. Loops internally over sitting-out auto-actions and
/// back-to-back new hands (each of which may itself land on another
/// sitting-out player), so callers don't hand-roll that recursion.
///
/// Returns [`PostAction::AllIn`] when betting closes with only all-in players
/// left mid-phase; the caller owns the all-in showdown + board run-out because
/// its continuation semantics differ between the live-action and run-out paths.
/// Returns [`PostAction::Done`] once a live turn is pending or the game has
/// paused / ended.
async fn drive_post_action(room_arc: &Arc<Mutex<Room>>, room_id: &str) -> PostAction {
    let mut room = room_arc.lock().await;
    loop {
        // Lone survivor (everyone else folded) → resolve and maybe deal again.
        if room.game_state.active_player_count() == 1 {
            room.game_state.resolve_hand();
            broadcast_state(&mut room, room_id);
            drop(room);
            if let Some((pid, act)) = maybe_start_new_hand(room_arc, room_id).await {
                room = room_arc.lock().await;
                apply_sitting_out_action(&mut room, pid, act);
                continue;
            }
            return PostAction::Done;
        }

        if room.game_state.is_betting_complete() {
            // River + betting complete → showdown, resolve, maybe deal again.
            if room.game_state.phase == GamePhase::River {
                room.game_state.resolve_hand();
                broadcast_state(&mut room, room_id);
                drop(room);
                if let Some((pid, act)) = maybe_start_new_hand(room_arc, room_id).await {
                    room = room_arc.lock().await;
                    apply_sitting_out_action(&mut room, pid, act);
                    continue;
                }
                return PostAction::Done;
            }
            // Betting complete mid-hand → advance the phase and render the new
            // community cards.
            room.game_state.advance_phase();
            broadcast_state(&mut room, room_id);

            // Only all-in players remain → hand off to the run-out path.
            if room.game_state.actionable_players().is_empty() {
                drop(room);
                return PostAction::AllIn;
            }

            if let Some((pid, act)) = notify_turn_and_start_timer(&mut room, room_arc, room_id) {
                apply_sitting_out_action(&mut room, pid, act);
                continue;
            }
            return PostAction::Done;
        }

        // Still action to take → notify the next player's turn.
        if let Some((pid, act)) = notify_turn_and_start_timer(&mut room, room_arc, room_id) {
            apply_sitting_out_action(&mut room, pid, act);
            continue;
        }
        return PostAction::Done;
    }
}

// ---------------------------------------------------------------------------
// Game start / action application
// ---------------------------------------------------------------------------

pub(crate) async fn start_game(ctx: CallerCtx) {
    let room_arc = ctx.room_arc.clone();
    let mut room = room_arc.lock().await;
    let pid = ctx.player_id;

    if let Err(e) = room.game_state.try_start(pid) {
        send_error(&mut room, &ctx.room_id, pid, &e.to_string());
        return;
    }

    // Notify the current player it's their turn, render state, and start the
    // timer. State regions are rendered from the settled post-try_start state.
    let sitting_out = notify_turn_and_start_timer(&mut room, &room_arc, &ctx.room_id);
    drop(room);
    if let Some((spid, act)) = sitting_out {
        process_action(spid, act, 0, &room_arc, &ctx.room_id).await;
    }
}

/// Apply a betting action: validate it, mutate chips/bets, then drive the
/// post-action loop (advance phase, resolve hand, start next hand) until the
/// game reaches a stable wait point. `room_id` is passed explicitly because
/// every call site already has it.
pub(crate) async fn process_action(
    player_id: u32,
    action: PlayerAction,
    amount: u32,
    room_arc: &Arc<Mutex<Room>>,
    room_id: &str,
) {
    process_action_with_room(room_arc, player_id, action, amount, room_id).await;
}

/// See [`process_action`]. The action mutation itself lives in
/// [`GameState::apply_action`] (`poker-core`); this function applies it, then
/// hands the settled state to [`drive_post_action`]. On the all-in handoff it
/// runs the board out to showdown.
async fn process_action_with_room(
    room_arc: &Arc<Mutex<Room>>,
    player_id: u32,
    action: PlayerAction,
    amount: u32,
    room_id: &str,
) {
    {
        let mut room = room_arc.lock().await;
        // ── Apply the action (validation + betting mutation in poker-core) ───
        if let Err(err) = room.game_state.apply_action(player_id, action, amount) {
            send_error(&mut room, room_id, player_id, &err.to_string());
            return;
        }
    }

    match drive_post_action(room_arc, room_id).await {
        PostAction::Done => {}
        PostAction::AllIn => {
            broadcast_allin_showdown(room_arc, room_id).await;
            run_out_board(room_arc, room_id).await;
        }
    }
}

/// After a player sits back in and un-pauses a waiting game, start the next
/// hand. If the new hand's first turn lands on a sitting-out player, drive
/// their auto-action through the normal [`process_action`] path so the turn
/// advances (and any terminal resolution fires). This is the only caller of
/// [`maybe_start_new_hand`] outside the post-action loop.
pub(crate) async fn resume_after_sit_in(room_arc: &Arc<Mutex<Room>>, room_id: &str) {
    if let Some((pid, act)) = maybe_start_new_hand(room_arc, room_id).await {
        process_action(pid, act, 0, room_arc, room_id).await;
    }
}

// ---------------------------------------------------------------------------
// Host-only actions
// ---------------------------------------------------------------------------

/// A sitting-out player sits back in. Re-renders immediately (player list +
/// controls changed), then — if the game was paused waiting for players —
/// hands off to [`resume_after_sit_in`] → [`maybe_start_new_hand`], which
/// re-evaluates the dealable count and re-pauses if the sit-in didn't reach
/// ≥2. The room lock is scoped so it isn't held across that await.
pub(crate) async fn sitin(room_arc: &Arc<Mutex<Room>>, room_id: &str, player_id: u32) {
    let resume = {
        let mut room = room_arc.lock().await;
        if !room
            .game_state
            .players
            .get(&player_id)
            .is_some_and(|p| p.sitting_out)
        {
            return;
        }
        room.game_state.set_sitting_in(player_id);
        broadcast_state(&mut room, room_id);
        room.game_state.waiting_for_players
    };

    if resume {
        resume_after_sit_in(room_arc, room_id).await;
    }
}

/// Host-only: toggle the late-entry flag. Rejects non-host callers with the
/// same [`poker_core::game_logic::StartGameError::NotHost`] toast the rest of
/// the flow uses.
pub(crate) async fn toggle_late_entry(room_arc: &Arc<Mutex<Room>>, room_id: &str, caller_id: u32) {
    let mut room = room_arc.lock().await;
    if let Err(e) = room.game_state.require_host(caller_id) {
        send_error(&mut room, room_id, caller_id, &e.to_string());
        return;
    }
    room.game_state.allow_late_entry = !room.game_state.allow_late_entry;
    // Late-entry toggle only changes the controls panel; re-render state.
    broadcast_state(&mut room, room_id);
    drop(room);
}

/// Host-only: apply the blind schedule and (pre-game) starting stack settings.
/// Rejects non-host callers with the same [`StartGameError::NotHost`] toast
/// the rest of the flow uses. The signal → domain coercion stays in the HTTP
/// layer; this receives typed values.
pub(crate) async fn update_settings(
    room_arc: &Arc<Mutex<Room>>,
    room_id: &str,
    caller_id: u32,
    blind_config: BlindConfig,
    starting_bbs: u32,
) {
    let mut room = room_arc.lock().await;
    if let Err(e) = room.game_state.require_host(caller_id) {
        send_error(&mut room, room_id, caller_id, &e.to_string());
        return;
    }
    room.game_state.apply_settings(blind_config, starting_bbs);
    broadcast_state(&mut room, room_id);
    drop(room);
}

// ---------------------------------------------------------------------------
// Hand-boundary sweep
// ---------------------------------------------------------------------------

/// Hand-boundary sweep: hard-remove every player flagged `wants_leave` from an
/// explicit mid-hand "Exit Game" ([`crate::room::RoomManager::leave_room`]).
/// This is the only index-safe removal point — callers invoke it between
/// hands, where the next `start_new_hand` recomputes every positional index
/// (dealer / blinds / current actor) from the surviving `player_order`, so
/// removing entries here can't desync the betting loop the way a mid-hand
/// `remove_player` would. Broadcasts once after the whole batch. Returns
/// whether any player was removed, so the caller can re-check the active count
/// / tear the room down.
pub fn sweep_leavers(room: &mut Room, room_id: &str) -> bool {
    let leavers: Vec<u32> = room
        .players
        .iter()
        .filter(|(_, c)| c.wants_leave)
        .map(|(id, _)| *id)
        .collect();
    if leavers.is_empty() {
        return false;
    }
    for pid in &leavers {
        remove_player_now(room, room_id, *pid);
    }
    broadcast_state(room, room_id);
    true
}

// ---------------------------------------------------------------------------
// Next-hand scheduling
// ---------------------------------------------------------------------------

/// If ≥ 2 active players remain, start the next hand after a short delay.
/// Otherwise pause and wait for players to sit back in.
///
/// Locks are already scoped as tightly as the borrow checker allows; the
/// remaining flagged gap is the value flowing straight into the return.
#[allow(clippy::significant_drop_tightening)]
async fn maybe_start_new_hand(
    room_arc: &Arc<Mutex<Room>>,
    room_id: &str,
) -> Option<(u32, PlayerAction)> {
    let should_start = {
        let mut room = room_arc.lock().await;
        if !room.game_state.game_started {
            return None;
        }

        // Sweep first — before the active-count / pause decision — so a leave
        // that drops the room below 2 active (→ pause) is still cleaned up. If
        // the sweep emptied the room, let the last stream's Drop tear it down.
        if sweep_leavers(&mut room, room_id) && !room.players.values().any(|c| c.tx.is_some()) {
            return None;
        }

        let active_count = room.game_state.dealable_player_count();

        if active_count >= 2 {
            room.game_state.waiting_for_players = false;
            true
        } else {
            room.game_state.waiting_for_players = true;
            // Paused: render state (no turn pending).
            broadcast_state(&mut room, room_id);
            false
        }
    }; // lock released

    if !should_start {
        return None;
    }

    // Delay without holding the room lock so other actions can still process.
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    // Re-acquire the lock and re-check conditions.
    let mut room = room_arc.lock().await;

    if !room.game_state.game_started {
        return None;
    }

    if room.game_state.dealable_player_count() < 2 {
        room.game_state.waiting_for_players = true;
        broadcast_state(&mut room, room_id);
        return None;
    }

    room.game_state.start_new_hand();
    notify_turn_and_start_timer(&mut room, room_arc, room_id)
}

// ---------------------------------------------------------------------------
// All-in run-out
// ---------------------------------------------------------------------------

/// Run out the remaining community cards when all players are all-in.
async fn run_out_board(room_arc: &Arc<Mutex<Room>>, room_id: &str) {
    'run_out: loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

        let mut room = room_arc.lock().await;

        room.game_state.advance_phase();
        broadcast_state(&mut room, room_id);

        if room.game_state.phase == GamePhase::Showdown {
            room.game_state.resolve_hand();
            broadcast_state(&mut room, room_id);
            drop(room);

            // Start the next hand (if enough active players remain). When its
            // first turn lands on a sitting-out player, apply their auto-action
            // before driving the post-action loop — drive_post_action evaluates
            // state first, so the seed action must land beforehand.
            if let Some((pid, act)) = maybe_start_new_hand(room_arc, room_id).await {
                {
                    let mut room = room_arc.lock().await;
                    apply_sitting_out_action(&mut room, pid, act);
                }
                match drive_post_action(room_arc, room_id).await {
                    PostAction::Done => {}
                    // Still all-in after the new hand → re-run the board out.
                    PostAction::AllIn => {
                        broadcast_allin_showdown(room_arc, room_id).await;
                        continue 'run_out;
                    }
                }
            }
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Turn notification / timer
// ---------------------------------------------------------------------------

/// Notify the player whose turn it is **and** start the turn timer.
///
/// Returns `Some((player_id, action))` when the current player is sitting
/// out so the caller can process their auto-action synchronously.
fn notify_turn_and_start_timer(
    room: &mut Room,
    room_arc: &Arc<Mutex<Room>>,
    room_id: &str,
) -> Option<(u32, PlayerAction)> {
    let current_id = room.game_state.current_player_id()?;

    // Bump the turn counter (invalidates stale timer tasks) and stamp when
    // this turn began (for mid-turn reconnects), before rendering.
    let turn = room
        .turn_counter
        .fetch_add(1, Ordering::SeqCst)
        .saturating_add(1);
    room.turn_started_at = Some(std::time::Instant::now());

    if room.game_state.is_current_player_sitting_out() {
        let action = room
            .game_state
            .auto_action(current_id)
            .unwrap_or(PlayerAction::Fold);
        tracing::info!(
            player = current_id,
            ?action,
            "Sitting-out player, auto-acting"
        );
        // Do NOT render state here: the caller will loop and process this
        // auto-action, landing on a real terminal (another turn / resolve /
        // new hand) that renders state once.
        return Some((current_id, action));
    }

    // Terminal point for a real turn: the active action bar (per-viewer) and
    // the countdown ring (data-turn-deadline on the active player's row, ticked
    // client-side by pokerTurnTick) are both produced by state_events here.
    broadcast_state(room, room_id);

    // Spawn a task to force an action after the timeout.
    let counter = Arc::clone(&room.turn_counter);
    let room_arc_clone = Arc::clone(room_arc);
    let rid = room_id.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(TURN_TIMEOUT_SECS.into())).await;
        if counter.load(Ordering::SeqCst) == turn {
            force_timeout_action(room_arc_clone, turn, current_id, &rid).await;
        }
    });

    None
}

/// Apply a sitting-out player's auto-action (Check or Fold) inline. The action
/// was pre-decided by [`GameState::auto_action`] in the caller; we run it
/// through [`GameState::apply_action`] so the betting mutation is shared with
/// the interactive path. The caller's post-action loop then lands on a real
/// terminal (turn / resolve / new hand) that renders state.
fn apply_sitting_out_action(room: &mut Room, player_id: u32, action: PlayerAction) {
    if !matches!(action, PlayerAction::Fold | PlayerAction::Check) {
        tracing::error!(?action, "Unexpected sitting-out auto-action");
        return;
    }
    if let Err(err) = room.game_state.apply_action(player_id, action, 0) {
        // The action was derived from valid_actions, so an error here means a
        // race (the turn moved underneath us). Log rather than render: the real
        // turn holder's render will cover the table.
        tracing::warn!(player = player_id, %err, "sitting-out auto-action rejected");
    }
}

/// Force a check-or-fold for a player whose turn timer has expired. The guard
/// is released before `process_action_with_room` re-locks; the lint flags the
/// trivial gap to the block end, which holds no contention.
#[allow(clippy::significant_drop_tightening)]
async fn force_timeout_action(
    room_arc: Arc<Mutex<Room>>,
    expected_turn: u64,
    player_id: u32,
    room_id: &str,
) {
    let action = {
        let mut room = room_arc.lock().await;

        if room.turn_counter.load(Ordering::SeqCst) != expected_turn {
            return;
        }
        if !room.game_state.game_started {
            return;
        }
        if room.game_state.current_player_id() != Some(player_id) {
            return;
        }

        let Some(act) = room.game_state.auto_action(player_id) else {
            return;
        };

        // If forced to fold, sit the player out. The away state is rendered by
        // the process_action_with_room call below at its terminal point.
        if act == PlayerAction::Fold
            && matches!(
                room.game_state.players.get(&player_id),
                Some(p) if !p.sitting_out
            )
        {
            room.game_state.set_sitting_out(player_id);
            tracing::info!(player = player_id, "Auto sitting out after timeout fold");
        }

        act
    }; // lock released

    tracing::info!(
        player = player_id,
        ?action,
        "Turn timer expired, forcing action"
    );

    process_action_with_room(&room_arc, player_id, action, 0, room_id).await;
}

// ---------------------------------------------------------------------------
// All-in showdown broadcast (equity overlay)
// ---------------------------------------------------------------------------

/// Broadcast an all-in showdown with equity percentages. Locks are scoped to
/// the data-extraction / fanout phases and released before any await; the
/// remaining flagged gap is the borrow feeding `ctx` into the per-viewer render.
#[allow(clippy::significant_drop_tightening)]
async fn broadcast_allin_showdown(room_arc: &Arc<Mutex<Room>>, room_id: &str) {
    // --- 1. Extract data while holding the lock (cheap) ----------------
    let (player_hands, hands_for_calc, board) = {
        let room = room_arc.lock().await;
        let mut player_hands: Vec<(u32, [Card; 2], Hand)> = Vec::new();

        for (id, hand) in room.game_state.live_hands() {
            player_hands.push((id, [hand.0, hand.1], hand));
        }

        if player_hands.len() < 2 {
            return;
        }

        let board = room.game_state.build_board();
        let hands_for_calc: Vec<Hand> = player_hands
            .iter()
            .map(|(_, _, h)| Hand(h.0, h.1))
            .collect();

        (player_hands, hands_for_calc, board)
    }; // lock released

    // --- 2. Run the CPU-heavy equity simulation off the async runtime --
    // If the task panics (e.g. a bug in calculate_equity_multi), log and fall
    // back to no equity overlay rather than tearing down this all-in run-out.
    let equities = match tokio::task::spawn_blocking(move || {
        poker_core::poker::calculate_equity_multi(&hands_for_calc, &board, 1000)
    })
    .await
    {
        Ok(v) => v,
        Err(join_err) => {
            tracing::error!(error = %join_err, "equity calculation task panicked");
            return;
        }
    };

    // --- 3. Re-acquire the lock and broadcast the result ---------------
    let hands_with_equity: Vec<(u32, [Card; 2], f64)> = player_hands
        .iter()
        .enumerate()
        .map(|(i, (id, cards, _))| (*id, *cards, equities.get(i).copied().unwrap_or(0.0)))
        .collect();

    let mut room = room_arc.lock().await;

    // Equity isn't in GameState, so render the all-in reveal table per-viewer
    // (is_us is viewer-relative). The only UI not derivable from the snapshot.
    send_all(&mut room, room_id, |ctx, viewer| {
        vec![render::equity_table_events(ctx, viewer, &hands_with_equity)]
    });
}
