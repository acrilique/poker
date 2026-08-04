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

//! HTTP handlers: the CQRS write side.
//!
//! Each action POST mutates the room's [`GameState`] under the room `Mutex`,
//! then fans the resulting [`DatastarEvent`]s to the right players' SSE
//! streams.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};

use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{Html, IntoResponse, Sse};
use datastar::axum::ReadSignals;
use futures_util::{Stream, StreamExt};
use poker_core::poker::Hand;
use poker_core::protocol::{BlindConfig, CardInfo, PlayerAction, card_to_info};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::render::{self, Ctx};
use crate::room::{CallerCtx, Fanout, Room, RoomManager, remove_player_now, resolve_caller};
use poker_core::game_logic::{GamePhase, PlayerStatus, TURN_TIMEOUT_SECS};

use crate::AppState;

/// Clamp a finite, non-negative `f64` into a `u64` without silent `as`
/// truncation/sign loss. NaN/negatives → 0, out-of-range → `u64::MAX`.
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn f64_to_u64(x: f64) -> u64 {
    if x.is_nan() || x < 0.0 {
        0
    } else if x >= u64::MAX as f64 {
        u64::MAX
    } else {
        x as u64
    }
}

/// Clamp a finite, non-negative `f64` into a `u32`.
fn f64_to_u32(x: f64) -> u32 {
    u32::try_from(f64_to_u64(x)).unwrap_or(u32::MAX)
}

/// Clamp a `u64` down into a `u32` (out-of-range → `u32::MAX`).
fn u64_to_u32(x: u64) -> u32 {
    u32::try_from(x).unwrap_or(u32::MAX)
}

/// Coerce a signal `Value` (number or string from a bound input) into an `f64`.
/// Strings are trimmed before parsing; unparseable/other types → `0.0`.
fn value_as_f64(v: &serde_json::Value) -> f64 {
    match v {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
        serde_json::Value::String(s) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Shell page
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "shell.html")]
struct ShellTpl;

// `async` keeps this shaped like the other axum handlers; the body has no
// await point today, which trips `unused_async`. Kept async deliberately.
#[allow(clippy::unused_async)]
pub async fn shell() -> Html<String> {
    Html(ShellTpl.render().unwrap_or_default())
}

/// `GET /poker/manifest.json` — the PWA web app manifest. Served at the app
/// root rather than under `/poker/static/` so its `scope: "/poker/"` is
/// honored by the browser. Embedded at compile time; no runtime file IO.
#[allow(clippy::unused_async)]
pub async fn manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("../static/manifest.json"),
    )
}

/// `GET /poker/sw.js` — the service worker. Served at the app root so its
/// registration scope can be `/poker/` (a worker under `/poker/static/` would
/// be capped to that sub-path and could not control the shell). Embedded at
/// compile time; compressed by the router's `CompressionLayer`.
#[allow(clippy::unused_async)]
pub async fn service_worker() -> impl IntoResponse {
    const SW: &str = include_str!("../static/sw.js");
    let stamped = SW.replace(
        "__POKER_CACHE_VERSION__",
        env!("POKER_CACHE_VERSION"),
    );
    ([(header::CONTENT_TYPE, "application/javascript")], stamped)
}

// ---------------------------------------------------------------------------
// Signals payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct SessionSignals {
    #[serde(rename = "roomid")]
    pub room_id: String,
    #[serde(rename = "sessiontoken")]
    pub session_token: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct CreateSignals {
    #[serde(rename = "roomid")]
    pub room_id: String,
    pub name: String,
    #[serde(rename = "blindmins")]
    pub blind_mins: serde_json::Value,
    #[serde(rename = "blindpct")]
    pub blind_pct: serde_json::Value,
    #[serde(rename = "stackbbs")]
    pub stack_bbs: serde_json::Value,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct JoinSignals {
    #[serde(rename = "roomid")]
    pub room_id: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct RaiseSignals {
    #[serde(rename = "roomid")]
    pub room_id: String,
    #[serde(rename = "sessiontoken")]
    pub session_token: String,
    #[serde(rename = "raiseamt")]
    pub raise_amt: serde_json::Value,
}

/// Host-only settings update (mirrors [`CreateSignals`], sent from the
/// in-game controls panel). All fields are `serde_json::Value` because the
/// connect-screen inputs bind to string-valued signals.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct UpdateSettingsSignals {
    #[serde(rename = "roomid")]
    pub room_id: String,
    #[serde(rename = "sessiontoken")]
    pub session_token: String,
    #[serde(rename = "blindmins")]
    pub blind_mins: serde_json::Value,
    #[serde(rename = "blindpct")]
    pub blind_pct: serde_json::Value,
    #[serde(rename = "stackbbs")]
    pub stack_bbs: serde_json::Value,
}

// ---------------------------------------------------------------------------
// SSE stream (the read side)
// ---------------------------------------------------------------------------

/// Runs teardown on client disconnect. On reload/tab-close, hyper drops the
/// response future, so the `stream::once` finalizer below never runs. Cleanup
/// belongs in `Drop`, which spawns [`RoomManager::disconnect_player`]. That's
/// idempotent (generation guard), so this drop path and the server-side path
/// can both fire.
struct DisconnectGuard {
    inner: Option<Pin<Box<dyn Stream<Item = SseItem> + Send>>>,
    /// `Some` until `Drop` takes it to spawn the teardown task.
    teardown: Option<(
        Arc<RoomManager>,
        String, // room_id
        u32,    // player_id
        u64,    // connection generation
    )>,
}

/// Item type of the `/poker/events` SSE stream: an axum `Event`, or an
/// infallible error (the inner stream never errors).
type SseItem = Result<axum::response::sse::Event, std::convert::Infallible>;

impl Stream for DisconnectGuard {
    type Item = SseItem;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        this.inner
            .as_mut()
            .map_or_else(|| Poll::Ready(None), |s| s.as_mut().poll_next(cx))
    }
}

impl Drop for DisconnectGuard {
    fn drop(&mut self) {
        if let Some((manager, room_id, pid, generation)) = self.teardown.take() {
            tracing::info!(
                room = %room_id,
                player = pid,
                generation,
                "SSE stream dropped — spawning disconnect teardown"
            );
            tokio::spawn(async move {
                manager.disconnect_player(&room_id, pid, generation).await;
            });
        }
    }
}

pub async fn events(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    let manager = state.room_manager.clone();
    let room_id = signals.room_id.clone();
    let token = signals.session_token.clone();

    // Resolve the session; on failure stream a single error event and end.
    // The error becomes a browser alert (targets <body>, so it works before
    // #game-root exists).
    let caller = match resolve_caller(&manager, &room_id, &token).await {
        Ok(c) => c,
        Err(e) => {
            let err = render::error_events_pub(&e);
            let stream = futures_util::stream::iter(
                err.into_iter()
                    .map(|ev| Ok::<_, std::convert::Infallible>(ev.write_as_axum_sse_event())),
            );
            return Sse::new(stream).into_response();
        }
    };

    // If the room already finished (GameOver broadcast), tear it down and
    // tell the client to reset.
    let teardown = {
        let room = caller.room_arc.lock().await;
        room.game_over
    };
    if teardown {
        manager.remove_room(&room_id).await;
        // Notify via alert, then blank the session signals; pokerHandleFetch
        // clears localStorage.
        let mut evs = render::notice_events("This game has ended. Thanks for playing!");
        evs.push(render::patch_signals(
            &serde_json::json!({ "sessiontoken": "", "roomid": "" }),
        ));
        let stream = futures_util::stream::iter(
            evs.into_iter()
                .map(|ev| Ok::<_, std::convert::Infallible>(ev.write_as_axum_sse_event())),
        );
        return Sse::new(stream).into_response();
    }

    let (rx, initial, generation) =
        RoomManager::attach_stream(&caller.room_arc, &room_id, caller.player_id, false).await;

    let pid = caller.player_id;
    let manager2 = manager.clone();
    let rid = room_id.clone();

    let stream = futures_util::stream::iter(
        initial
            .into_iter()
            .map(|ev| Ok::<_, std::convert::Infallible>(ev.write_as_axum_sse_event())),
    )
    .chain(
        tokio_stream::wrappers::ReceiverStream::new(rx).map(|ev| Ok(ev.write_as_axum_sse_event())),
    )
    .chain(futures_util::stream::once(async move {
        // Server-side close path: fires when the inner channel sender is
        // dropped (e.g. the seat is reclaimed elsewhere). Doesn't fire on
        // client disconnect — see [`DisconnectGuard`].
        manager2.detach_stream(&rid, pid, generation).await;
        tracing::info!(room = %rid, player = pid, generation, "SSE stream closed (server-side)");
        // Yield nothing further; this once() exists only for the side effect.
        Ok(datastar::prelude::PatchSignals::new("{}").write_as_axum_sse_event())
    }));

    let guarded = DisconnectGuard {
        inner: Some(Box::pin(stream)),
        teardown: Some((manager.clone(), room_id.clone(), pid, generation)),
    };

    Sse::new(guarded)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
}

// ---------------------------------------------------------------------------
// Room create / join
// ---------------------------------------------------------------------------

pub async fn room_create(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<CreateSignals>,
) -> impl IntoResponse {
    let blind_config = BlindConfig {
        interval_secs: f64_to_u64(value_as_f64(&signals.blind_mins)).saturating_mul(60),
        increase_percent: f64_to_u32(value_as_f64(&signals.blind_pct)),
    };
    let starting_bbs = f64_to_u32(value_as_f64(&signals.stack_bbs)).max(1);

    match state
        .room_manager
        .create_room(&signals.room_id, blind_config, starting_bbs)
        .await
    {
        Ok(()) => {}
        Err(e) => return events_response(render::error_events_pub(&e.to_string())),
    }

    join_common(state, signals.room_id, signals.name).await
}

pub async fn room_join(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<JoinSignals>,
) -> impl IntoResponse {
    join_common(state, signals.room_id, signals.name).await
}

/// `POST /poker/room/leave` — explicit "Exit Game". Distinct from an SSE drop
/// (reload / tab close / network blip): a leave frees the seat as soon as it
/// is safe, with no grace-period ghost. See [`RoomManager::leave_room`].
///
/// If the leaving player is mid-hand and it is currently their turn, fold via
/// the normal action path first so the betting loop advances correctly; the
/// actual seat removal is deferred to the next hand boundary.
pub async fn room_leave(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> axum::response::Response {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };

    // If it's this player's turn mid-hand, fold through the real action path
    // so the turn advances (and any single-remaining-player / betting-complete
    // resolution fires) before we mark them as leaving.
    let is_their_turn = {
        let room = ctx.room_arc.lock().await;
        room.game_state.game_started
            && room.game_state.current_player_id() == Some(ctx.player_id)
    };
    if is_their_turn {
        process_action(
            ctx.player_id,
            PlayerAction::Fold,
            0,
            &ctx.room_arc,
            &ctx.room_id,
        )
        .await;
    }

    let outcome = state.room_manager.leave_room(&ctx.room_id, ctx.player_id).await;
    tracing::info!(
        room = %ctx.room_id,
        player = ctx.player_id,
        ?outcome,
        "Player left room"
    );
    no_content()
}

async fn join_common(state: AppState, room_id: String, name: String) -> axum::response::Response {
    let name = name.trim().to_string();
    if name.is_empty() {
        return events_response(render::error_events_pub("Player name cannot be empty"));
    }
    if name.len() > 16 {
        return events_response(render::error_events_pub(
            "Player name must be at most 16 characters",
        ));
    }

    match state.room_manager.join_room(&room_id, &name).await {
        Ok((pid, token, room_arc)) => {
            let events = {
                // sessiontoken/roomid are persisted via signals (pokerHandleFetch
                // saves them to localStorage on each patch).
                let mut evs = vec![render::patch_signals(&serde_json::json!(
                    { "sessiontoken": token, "roomid": room_id }
                ))];
                // Render the snapshot under the room lock. `ctx` borrows the
                // lock guard, so it can't be dropped sooner than this block —
                // hence the `significant_drop_tightening` allow in
                // `render_full_snapshot`.
                let snapshot = render_full_snapshot(&room_arc, &room_id, pid).await;
                evs.extend(snapshot);
                // Order matters: the signal patch must land before this trigger
                // fires, so `$sessiontoken` is set when the GET is sent.
                evs.extend(render::attach_events_stream_trigger());
                evs
            };
            events_response(events)
        }
        Err(e) => events_response(render::error_events_pub(&e.to_string())),
    }
}

/// Render events into a short-lived SSE response (for POST handlers).
fn events_response(events: Vec<datastar::DatastarEvent>) -> axum::response::Response {
    let stream = futures_util::stream::iter(
        events
            .into_iter()
            .map(|ev| Ok::<_, std::convert::Infallible>(ev.write_as_axum_sse_event())),
    );
    Sse::new(stream).into_response()
}

/// 204 No Content — the action's effects are pushed down the stream.
fn no_content() -> axum::response::Response {
    axum::http::StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// Action POSTs
// ---------------------------------------------------------------------------

pub async fn action_start(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    start_game(ctx).await;
    no_content()
}

pub async fn action_fold(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    process_action(
        ctx.player_id,
        PlayerAction::Fold,
        0,
        &ctx.room_arc,
        &ctx.room_id,
    )
    .await;
    maybe_cleanup_after_action(&state, &ctx).await;
    no_content()
}

pub async fn action_check(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    process_action(
        ctx.player_id,
        PlayerAction::Check,
        0,
        &ctx.room_arc,
        &ctx.room_id,
    )
    .await;
    maybe_cleanup_after_action(&state, &ctx).await;
    no_content()
}

pub async fn action_call(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    process_action(
        ctx.player_id,
        PlayerAction::Call,
        0,
        &ctx.room_arc,
        &ctx.room_id,
    )
    .await;
    maybe_cleanup_after_action(&state, &ctx).await;
    no_content()
}

pub async fn action_allin(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    process_action(
        ctx.player_id,
        PlayerAction::AllIn,
        0,
        &ctx.room_arc,
        &ctx.room_id,
    )
    .await;
    maybe_cleanup_after_action(&state, &ctx).await;
    no_content()
}

pub async fn action_raise(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<RaiseSignals>,
) -> impl IntoResponse {
    let session = SessionSignals {
        room_id: signals.room_id.clone(),
        session_token: signals.session_token.clone(),
    };
    let Some(ctx) = authorize(&state, &session).await else {
        return no_content();
    };
    let amount = match &signals.raise_amt {
        serde_json::Value::Number(n) => u64_to_u32(n.as_u64().unwrap_or(0)),
        serde_json::Value::String(s) => f64_to_u32(s.trim().parse::<f64>().unwrap_or(0.0)),
        _ => 0,
    };
    process_action(
        ctx.player_id,
        PlayerAction::Raise,
        amount,
        &ctx.room_arc,
        &ctx.room_id,
    )
    .await;
    maybe_cleanup_after_action(&state, &ctx).await;
    no_content()
}

pub async fn action_sitin(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    let room_arc = ctx.room_arc.clone();
    // Decide under the room lock whether to start a new hand, then release
    // before awaiting (so we don't hold it across the deal/timer setup).
    let start_new_hand = {
        let mut room = room_arc.lock().await;
        let pid = ctx.player_id;
        if !room
            .game_state
            .players
            .get(&pid)
            .is_some_and(|p| p.sitting_out)
        {
            return no_content();
        }
        room.game_state.set_sitting_in(pid);
        // Re-render (player list + controls changed). The branch below renders
        // again if it un-pauses the game and starts a hand.
        broadcast_state(&mut room, &ctx.room_id);

        // If the game was paused waiting for players, maybe start a new hand.
        if room.game_state.waiting_for_players {
            let active_count = room
                .game_state
                .player_order
                .iter()
                .filter(|id| {
                    room.game_state
                        .players
                        .get(id)
                        .is_some_and(|p| !p.sitting_out && p.chips > 0)
                })
                .count();
            if active_count >= 2 {
                room.game_state.waiting_for_players = false;
                true
            } else {
                false
            }
        } else {
            false
        }
    };

    if start_new_hand {
        let sitting_out = maybe_start_new_hand(&room_arc, &ctx.room_id).await;
        if let Some((pid2, act)) = sitting_out {
            process_action(pid2, act, 0, &room_arc, &ctx.room_id).await;
        }
    }
    no_content()
}

pub async fn action_toggle_late_entry(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    let mut room = ctx.room_arc.lock().await;
    if room.game_state.host_id != ctx.player_id {
        send_error(
            &mut room,
            &ctx.room_id,
            ctx.player_id,
            "Only the host can perform this action",
        );
        return no_content();
    }
    room.game_state.allow_late_entry = !room.game_state.allow_late_entry;
    // Late-entry toggle only changes the controls panel; re-render state.
    broadcast_state(&mut room, &ctx.room_id);
    drop(room);
    no_content()
}

pub async fn action_update_settings(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<UpdateSettingsSignals>,
) -> impl IntoResponse {
    let session = SessionSignals {
        room_id: signals.room_id.clone(),
        session_token: signals.session_token.clone(),
    };
    let Some(ctx) = authorize(&state, &session).await else {
        return no_content();
    };
    let mut room = ctx.room_arc.lock().await;
    if room.game_state.host_id != ctx.player_id {
        send_error(
            &mut room,
            &ctx.room_id,
            ctx.player_id,
            "Only the host can perform this action",
        );
        return no_content();
    }

    let new_config = BlindConfig {
        interval_secs: f64_to_u64(value_as_f64(&signals.blind_mins)).saturating_mul(60),
        increase_percent: f64_to_u32(value_as_f64(&signals.blind_pct)),
    };
    room.game_state.blind_config = new_config;
    // Room.blind_config mirrors game_state's.
    room.blind_config = new_config;

    // `starting_bbs` is frozen into `starting_chips` at game start, so only
    // apply it pre-game.
    if !room.game_state.game_started {
        let new_bbs = f64_to_u32(value_as_f64(&signals.stack_bbs)).max(1);
        room.game_state.starting_bbs = new_bbs;
        // Chips are frozen into each player at join time
        // (`add_player_with_chips`), so `starting_bbs` alone doesn't reach
        // already-seated players. Pre-game no chips have been won or lost, so
        // every player is still at the now-stale buy-in — rebuy them at the
        // new amount so existing players match those who join afterwards.
        // `big_blind` is the right multiplier here: `starting_big_blind` is
        // only frozen at game start, so it's still 0 pre-game (matching the
        // `bb` selection in `add_player_with_chips`).
        let new_stack = new_bbs.saturating_mul(room.game_state.big_blind);
        for player in room.game_state.players.values_mut() {
            player.chips = new_stack;
        }
    }

    // Re-anchor the schedule so the catch-up loop in start_new_hand doesn't
    // step blinds repeatedly when the interval changes.
    if room.game_state.game_started && new_config.is_enabled() {
        room.game_state.last_blind_increase = Some(std::time::Instant::now());
    }

    broadcast_state(&mut room, &ctx.room_id);
    drop(room);
    no_content()
}

// ---------------------------------------------------------------------------
// Authorization helper
// ---------------------------------------------------------------------------

async fn authorize(state: &AppState, signals: &SessionSignals) -> Option<CallerCtx> {
    match resolve_caller(
        &state.room_manager,
        &signals.room_id,
        &signals.session_token,
    )
    .await
    {
        Ok(c) => Some(c),
        Err(e) => {
            // We can't push to the caller's stream without a valid session;
            // log and drop.
            tracing::warn!(error = %e, "unauthorized action POST");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Message fanout helpers (all under the room lock)
// ---------------------------------------------------------------------------

/// Render the full state snapshot for `pid` under the room lock. `ctx` borrows
/// the guard, so it can't be dropped before this returns — hence the allow.
#[allow(clippy::significant_drop_tightening)]
async fn render_full_snapshot(
    room_arc: &Arc<Mutex<Room>>,
    room_id: &str,
    pid: u32,
) -> Vec<datastar::DatastarEvent> {
    let room = room_arc.lock().await;
    let ctx = ctx_of(&room, room_id);
    render::full_snapshot(&ctx, pid)
}

pub fn ctx_of<'a>(room: &'a Room, room_id: &'a str) -> Ctx<'a> {
    let turn_remaining = room
        .turn_started_at
        .map_or(TURN_TIMEOUT_SECS, |t| {
            u64_to_u32(u64::from(TURN_TIMEOUT_SECS).saturating_sub(t.elapsed().as_secs()))
        })
        .max(1);
    Ctx::new(&room.game_state, room_id, turn_remaining)
}

/// Render the full settled state for every connected player (a fat-morph of
/// `#game-root`) and fan out. Each viewer's state regions are recomputed from
/// the final `GameState` at a point where the game is about to wait.
pub fn broadcast_state(room: &mut Room, room_id: &str) {
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
fn send_error(room: &mut Room, _room_id: &str, viewer: u32, detail: &str) {
    let evs = render::toast_events(detail);
    let mut fan = Fanout::new(room);
    fan.send_to(viewer, &evs);
}

/// After an action, if the room hit `GameOver`, push the notice down every
/// live stream and clear the session signals so a reload returns to the
/// connect screen.
async fn maybe_cleanup_after_action(state: &AppState, ctx: &CallerCtx) {
    let _ = state; // reserved for future cleanup hooks
    let mut room = ctx.room_arc.lock().await;
    if room.game_state.is_game_over() {
        room.game_over = true;
        // Surface the notice and blank session signals on every connected
        // client now, not on the next attach.
        let mut evs = render::notice_events("This game has ended. Thanks for playing!");
        evs.push(render::patch_signals(
            &serde_json::json!({ "sessiontoken": "", "roomid": "" }),
        ));
        let per_viewer: Vec<u32> = room.players.keys().copied().collect();
        let mut fan = Fanout::new(&mut room);
        for viewer in per_viewer {
            fan.send_to(viewer, &evs);
        }
    }
    drop(room);
}

// ---------------------------------------------------------------------------
// Game flow (ported from ws_handler.rs)
// ---------------------------------------------------------------------------

async fn start_game(ctx: CallerCtx) {
    let room_arc = ctx.room_arc.clone();
    let mut room = room_arc.lock().await;
    let pid = ctx.player_id;

    if room.game_state.game_started {
        send_error(&mut room, &ctx.room_id, pid, "Game already started");
        return;
    }
    if room.game_state.player_count() < 2 {
        send_error(
            &mut room,
            &ctx.room_id,
            pid,
            "Need at least 2 players to start",
        );
        return;
    }
    if room.game_state.host_id != pid {
        send_error(
            &mut room,
            &ctx.room_id,
            pid,
            "Only the host can perform this action",
        );
        return;
    }

    room.game_state.game_started = true;

    // Freeze the starting chip amount and big blind for late entries.
    room.game_state.starting_big_blind = room.game_state.big_blind;
    room.game_state.starting_chips = room
        .game_state
        .starting_bbs
        .saturating_mul(room.game_state.big_blind);

    // Initialise the blind increase timer if configured.
    if room.game_state.blind_config.is_enabled() {
        room.game_state.last_blind_increase = Some(std::time::Instant::now());
    }

    // Start the first hand. State regions are rendered once below by
    // notify_turn_and_start_timer from this settled state.
    let _hand_msgs = room.game_state.start_new_hand();

    // Notify the current player it's their turn, render state, and start the
    // timer.
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
pub async fn process_action(
    player_id: u32,
    action: PlayerAction,
    amount: u32,
    room_arc: &Arc<Mutex<Room>>,
    room_id: &str,
) {
    process_action_with_room(room_arc, player_id, action, amount, room_id).await;
}

/// See [`process_action`]. The match arms mutate shared `room`/`player` state
/// with early `return`s, so they don't decompose into helpers without
/// duplicating the pre-checks — hence the length.
#[allow(clippy::too_many_lines)]
async fn process_action_with_room(
    room_arc: &Arc<Mutex<Room>>,
    player_id: u32,
    action: PlayerAction,
    amount: u32,
    room_id: &str,
) {
    let mut room = room_arc.lock().await;

    // ── Pre-checks ───────────────────────────────────────────────────
    if !room.game_state.game_started {
        send_error(&mut room, room_id, player_id, "Game not started");
        return;
    }

    if room.game_state.current_player_id() != Some(player_id) {
        send_error(&mut room, room_id, player_id, "Not your turn");
        return;
    }

    let valid = room.game_state.valid_actions(player_id);
    if !valid.contains(&action) {
        send_error(&mut room, room_id, player_id, "Invalid action");
        return;
    }

    let Some(player) = room.game_state.players.get(&player_id).cloned() else {
        send_error(&mut room, room_id, player_id, "Player not found");
        return;
    };

    let to_call = room
        .game_state
        .current_bet
        .saturating_sub(player.current_bet);

    // ── Apply the action ─────────────────────────────────────────────
    match action {
        PlayerAction::Fold => {
            if let Some(p) = room.game_state.players.get_mut(&player_id) {
                p.status = PlayerStatus::Folded;
            }
        }
        PlayerAction::Check => {
            if to_call != 0 {
                send_error(
                    &mut room,
                    room_id,
                    player_id,
                    "Cannot check, must call or raise",
                );
                return;
            }
            if room.game_state.phase == GamePhase::PreFlop && room.game_state.big_blind_option {
                room.game_state.big_blind_option = false;
                room.game_state.last_raiser_index = None;
            }
        }
        PlayerAction::Call => {
            let call_amount = to_call.min(player.chips);
            if let Some(p) = room.game_state.players.get_mut(&player_id) {
                p.chips = p.chips.saturating_sub(call_amount);
                p.current_bet = p.current_bet.saturating_add(call_amount);
                if p.chips == 0 {
                    p.status = PlayerStatus::AllIn;
                }
            }
            room.game_state.pot = room.game_state.pot.saturating_add(call_amount);
            let entry = room
                .game_state
                .pot_contributions
                .entry(player_id)
                .or_insert(0);
            *entry = entry.saturating_add(call_amount);
        }
        PlayerAction::Raise => {
            let raise_total = to_call.saturating_add(amount);
            if raise_total > player.chips {
                send_error(
                    &mut room,
                    room_id,
                    player_id,
                    &format!(
                        "Not enough chips. Have {}, need {raise_total}",
                        player.chips
                    ),
                );
                return;
            }
            let min_raise = room.game_state.min_raise;
            if amount < min_raise && raise_total < player.chips {
                send_error(
                    &mut room,
                    room_id,
                    player_id,
                    &format!("Minimum raise is {min_raise}"),
                );
                return;
            }

            let new_bet;
            if let Some(p) = room.game_state.players.get_mut(&player_id) {
                p.chips = p.chips.saturating_sub(raise_total);
                p.current_bet = p.current_bet.saturating_add(raise_total);
                new_bet = p.current_bet;
                if p.chips == 0 {
                    p.status = PlayerStatus::AllIn;
                }
            } else {
                new_bet = player.current_bet.saturating_add(raise_total);
            }
            room.game_state.pot = room.game_state.pot.saturating_add(raise_total);
            let entry = room
                .game_state
                .pot_contributions
                .entry(player_id)
                .or_insert(0);
            *entry = entry.saturating_add(raise_total);
            room.game_state.current_bet = new_bet;
            room.game_state.min_raise = room.game_state.big_blind;
            room.game_state.last_raiser_index = Some(room.game_state.current_player_index);
            room.game_state.big_blind_option = false;
        }
        PlayerAction::AllIn => {
            let all_in = player.chips;
            let new_bet;
            if let Some(p) = room.game_state.players.get_mut(&player_id) {
                p.chips = 0;
                p.current_bet = p.current_bet.saturating_add(all_in);
                new_bet = p.current_bet;
                p.status = PlayerStatus::AllIn;
            } else {
                new_bet = player.current_bet.saturating_add(all_in);
            }
            room.game_state.pot = room.game_state.pot.saturating_add(all_in);
            let entry = room
                .game_state
                .pot_contributions
                .entry(player_id)
                .or_insert(0);
            *entry = entry.saturating_add(all_in);
            if new_bet > room.game_state.current_bet {
                // Only reopen betting (set last_raiser_index) if the all-in
                // constitutes a full legal raise.
                let raise_increment = new_bet.saturating_sub(room.game_state.current_bet);
                if raise_increment >= room.game_state.min_raise {
                    room.game_state.last_raiser_index = Some(room.game_state.current_player_index);
                }
                room.game_state.current_bet = new_bet;
            }
        }
    }

    room.game_state.has_acted_this_round = true;
    room.game_state.next_player();

    // ── Post-action: check hand / betting status ─────────────────────
    //
    // The action bar, player list, and pot are only rendered at the terminal
    // points below (notify_turn_and_start_timer / resolve_hand /
    // advance_phase) — never mid-transition.
    loop {
        if room.game_state.active_player_count() == 1 {
            let _msgs = room.game_state.resolve_hand();
            broadcast_state(&mut room, room_id);
            drop(room);
            if let Some((pid, act)) = maybe_start_new_hand(room_arc, room_id).await {
                room = room_arc.lock().await;
                apply_sitting_out_action(&mut room, pid, act);
                continue;
            }
            return;
        }

        if room.game_state.is_betting_complete() {
            if room.game_state.phase == GamePhase::River {
                let _msgs = room.game_state.resolve_hand();
                broadcast_state(&mut room, room_id);
                drop(room);
                if let Some((pid, act)) = maybe_start_new_hand(room_arc, room_id).await {
                    room = room_arc.lock().await;
                    apply_sitting_out_action(&mut room, pid, act);
                    continue;
                }
                return;
            }
            // Advance to next phase and render the new community cards.
            let _phase_msgs = room.game_state.advance_phase();
            broadcast_state(&mut room, room_id);

            // If only all-in players remain, run it out.
            if room.game_state.actionable_players().is_empty() {
                drop(room);
                broadcast_allin_showdown(room_arc, room_id).await;
                run_out_board(room_arc, room_id).await;
                return;
            }

            if let Some((pid, act)) = notify_turn_and_start_timer(&mut room, room_arc, room_id) {
                apply_sitting_out_action(&mut room, pid, act);
                continue;
            }
            return;
        }

        if let Some((pid, act)) = notify_turn_and_start_timer(&mut room, room_arc, room_id) {
            apply_sitting_out_action(&mut room, pid, act);
            continue;
        }
        return;
    }
}

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

        let active_count = room
            .game_state
            .player_order
            .iter()
            .filter(|id| {
                room.game_state
                    .players
                    .get(id)
                    .is_some_and(|p| !p.sitting_out && p.chips > 0)
            })
            .count();

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

    let active_count = room
        .game_state
        .player_order
        .iter()
        .filter(|id| {
            room.game_state
                .players
                .get(id)
                .is_some_and(|p| !p.sitting_out && p.chips > 0)
        })
        .count();

    if active_count < 2 {
        room.game_state.waiting_for_players = true;
        broadcast_state(&mut room, room_id);
        return None;
    }

    // Hand-boundary sweep: hard-remove any player who explicitly left
    // ([`RoomManager::leave_room`]) during the previous hand. This is the one
    // index-safe point — `start_new_hand` recomputes every positional index
    // (dealer / blinds / current actor) from the surviving `player_order`
    // immediately after, so removing entries here can't desync the betting
    // loop the way a mid-hand `remove_player` would.
    let leavers: Vec<u32> = room
        .players
        .iter()
        .filter(|(_, c)| c.wants_leave)
        .map(|(id, _)| *id)
        .collect();
    let mut any_removed = false;
    for pid in &leavers {
        remove_player_now(&mut room, room_id, *pid);
        any_removed = true;
    }
    if any_removed {
        broadcast_state(&mut room, room_id);
        // If the sweep emptied the room, tear it down (no one left to play).
        let any_connected = room.players.values().any(|c| c.tx.is_some());
        if !any_connected {
            drop(room);
            // Best-effort: the outer RoomManager owns the map, but this helper
            // only has the room Arc. The room will be reclaimed when the last
            // stream's Drop runs `disconnect_player` → `remove_room_if_empty`.
            return None;
        }
        // After removal, re-check whether enough active players remain.
        let active_after = room
            .game_state
            .player_order
            .iter()
            .filter(|id| {
                room.game_state
                    .players
                    .get(id)
                    .is_some_and(|p| !p.sitting_out && p.chips > 0)
            })
            .count();
        if active_after < 2 {
            room.game_state.waiting_for_players = true;
            broadcast_state(&mut room, room_id);
            return None;
        }
    }

    // notify_turn_and_start_timer renders state from this settled snapshot.
    let _hand_msgs = room.game_state.start_new_hand();
    notify_turn_and_start_timer(&mut room, room_arc, room_id)
}

/// Run out the remaining community cards when all players are all-in.
async fn run_out_board(room_arc: &Arc<Mutex<Room>>, room_id: &str) {
    'run_out: loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

        let mut room = room_arc.lock().await;

        let _phase_msgs = room.game_state.advance_phase();
        broadcast_state(&mut room, room_id);

        if room.game_state.phase == GamePhase::Showdown {
            let _msgs = room.game_state.resolve_hand();
            broadcast_state(&mut room, room_id);
            drop(room);
            if let Some((mut pid, mut act)) = maybe_start_new_hand(room_arc, room_id).await {
                let mut room = room_arc.lock().await;
                loop {
                    apply_sitting_out_action(&mut room, pid, act);

                    if room.game_state.active_player_count() == 1 {
                        let _hand_msgs = room.game_state.resolve_hand();
                        broadcast_state(&mut room, room_id);
                        drop(room);
                        if let Some((np, na)) = maybe_start_new_hand(room_arc, room_id).await {
                            room = room_arc.lock().await;
                            pid = np;
                            act = na;
                            continue;
                        }
                        break;
                    }

                    if room.game_state.is_betting_complete() {
                        if room.game_state.phase == GamePhase::River {
                            let _hand_msgs = room.game_state.resolve_hand();
                            broadcast_state(&mut room, room_id);
                            drop(room);
                            if let Some((np, na)) = maybe_start_new_hand(room_arc, room_id).await {
                                room = room_arc.lock().await;
                                pid = np;
                                act = na;
                                continue;
                            }
                            break;
                        }
                        let _phase_msgs = room.game_state.advance_phase();
                        broadcast_state(&mut room, room_id);
                        if room.game_state.actionable_players().is_empty() {
                            drop(room);
                            broadcast_allin_showdown(room_arc, room_id).await;
                            continue 'run_out;
                        }
                        if let Some((np, na)) =
                            notify_turn_and_start_timer(&mut room, room_arc, room_id)
                        {
                            pid = np;
                            act = na;
                            continue;
                        }
                        break;
                    }

                    if let Some((np, na)) =
                        notify_turn_and_start_timer(&mut room, room_arc, room_id)
                    {
                        pid = np;
                        act = na;
                        continue;
                    }
                    break;
                }
            }
            return;
        }
    }
}

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
        let valid = room.game_state.valid_actions(current_id);
        let action = if valid.contains(&PlayerAction::Check) {
            PlayerAction::Check
        } else {
            PlayerAction::Fold
        };
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
    // the countdown ring (--timer-duration on the active player's row) are both
    // produced by state_events here.
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

/// Apply a sitting-out player's auto-action (Check or Fold) inline.
fn apply_sitting_out_action(room: &mut Room, player_id: u32, action: PlayerAction) {
    match action {
        PlayerAction::Fold => {
            if let Some(p) = room.game_state.players.get_mut(&player_id) {
                p.status = PlayerStatus::Folded;
            }
        }
        PlayerAction::Check => {
            if room.game_state.phase == GamePhase::PreFlop && room.game_state.big_blind_option {
                room.game_state.big_blind_option = false;
                room.game_state.last_raiser_index = None;
            }
        }
        _ => {
            tracing::error!(?action, "Unexpected sitting-out auto-action");
            return;
        }
    }

    // The caller's post-action loop will land on a real terminal (turn /
    // resolve / new hand) that renders state.
    room.game_state.has_acted_this_round = true;
    room.game_state.next_player();
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

        let valid = room.game_state.valid_actions(player_id);
        let act = if valid.contains(&PlayerAction::Check) {
            PlayerAction::Check
        } else {
            PlayerAction::Fold
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

/// Broadcast an all-in showdown with equity percentages. Locks are scoped to
/// the data-extraction / fanout phases and released before any await; the
/// remaining flagged gap is the borrow feeding `ctx` into the per-viewer render.
#[allow(clippy::significant_drop_tightening)]
async fn broadcast_allin_showdown(room_arc: &Arc<Mutex<Room>>, room_id: &str) {
    // --- 1. Extract data while holding the lock (cheap) ----------------
    let (player_hands, hands_for_calc, board) = {
        let room = room_arc.lock().await;
        let mut player_hands: Vec<(u32, [CardInfo; 2], Hand)> = Vec::new();

        for &id in &room.game_state.player_order {
            if let Some(player) = room.game_state.players.get(&id)
                && (player.status == PlayerStatus::Active || player.status == PlayerStatus::AllIn)
                && let Some((c1, c2)) = player.hole_cards
            {
                let cards = [card_to_info(&c1), card_to_info(&c2)];
                player_hands.push((id, cards, Hand(c1, c2)));
            }
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
    let hands_with_equity: Vec<(u32, [CardInfo; 2], f64)> = player_hands
        .iter()
        .enumerate()
        .map(|(i, (id, cards, _))| (*id, *cards, equities.get(i).copied().unwrap_or(0.0)))
        .collect();

    let mut room = room_arc.lock().await;

    // Equity isn't in GameState, so render the all-in reveal table per-viewer
    // (is_us is viewer-relative). The only UI not derivable from the snapshot.
    let per_viewer: Vec<(u32, Vec<datastar::DatastarEvent>)> = {
        let ctx = ctx_of(&room, room_id);
        room.players
            .keys()
            .map(|&viewer| {
                let events = vec![render::equity_table_events(
                    &ctx,
                    viewer,
                    &hands_with_equity,
                )];
                (viewer, events)
            })
            .collect()
    };
    let mut fan = Fanout::new(&mut room);
    for (viewer, events) in per_viewer {
        fan.send_to(viewer, &events);
    }
}
