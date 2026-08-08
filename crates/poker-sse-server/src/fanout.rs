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

//! Room-aware render / fanout glue.
//!
//! Bridges the [`Room`] (data layer) and [`render`] (pure `GameState` → events)
//! by building a render [`Ctx`] under the room lock and fanning rendered events
//! out to per-player channels.

use std::sync::Arc;

use poker_core::game_logic::TURN_TIMEOUT_SECS;
use tokio::sync::Mutex;

use crate::render::{self, Ctx};
use crate::room::{Fanout, Room};

/// Build the render context for a room: wraps the room's [`GameState`] plus the
/// absolute epoch-ms deadline of the current turn (stable across reconnects).
///
/// Unlike a remaining-seconds value, a deadline is stable across renders: a
/// reconnect at T=10 of a 30s turn gets the same deadline as the original
/// render at T=0, so the client-side ring (driven by `deadline - Date.now()`)
/// resumes at the right fraction instead of restarting from full. Computed with
/// sub-second precision so the ring doesn't read up to ~1s too long after a
/// reconnect.
pub(crate) fn ctx_of<'a>(room: &'a Room, room_id: &'a str) -> Ctx<'a> {
    let turn_deadline_ms = room.turn_started_at.map(|t| {
        let elapsed_ms = t.elapsed().as_millis();
        let remaining_ms = u128::from(TURN_TIMEOUT_SECS)
            .saturating_mul(1000)
            .saturating_sub(elapsed_ms);
        render::epoch_ms_deadline(remaining_ms)
    });
    Ctx::new(&room.game_state, room_id, turn_deadline_ms)
}

/// Render the full settled state for every connected player (a fat-morph of
/// `#game-root`) and fan out. Each viewer's state regions are recomputed from
/// the final `GameState` at a point where the game is about to wait.
pub(crate) fn broadcast_state(room: &mut Room, room_id: &str) {
    let per_viewer: Vec<(u32, Vec<datastar::DatastarEvent>)> = {
        let ctx = ctx_of(room, room_id);
        room.players
            .keys()
            .map(|&viewer| (viewer, render::state_events(&ctx, viewer)))
            .collect()
    };
    let mut fan = Fanout::new(room);
    for (viewer, events) in per_viewer {
        fan.send_to(viewer, &events);
    }
}

/// Surface a transient error to one player via the in-table `#toast` region
/// (see [`render::toast_events`]). For pre-action rejections like "Not your
/// turn"; only fires after the player is connected, so the region is mounted.
pub(crate) fn send_error(room: &mut Room, _room_id: &str, viewer: u32, detail: &str) {
    let evs = render::toast_events(detail);
    let mut fan = Fanout::new(room);
    fan.send_to(viewer, &evs);
}

/// Render the full state snapshot for `pid` under the room lock. `ctx` borrows
/// the guard, so it can't be dropped before this returns — hence the allow.
#[allow(clippy::significant_drop_tightening)]
pub(crate) async fn render_full_snapshot(
    room_arc: &Arc<Mutex<Room>>,
    room_id: &str,
    pid: u32,
) -> Vec<datastar::DatastarEvent> {
    let room = room_arc.lock().await;
    let ctx = ctx_of(&room, room_id);
    render::full_snapshot(&ctx, pid)
}
