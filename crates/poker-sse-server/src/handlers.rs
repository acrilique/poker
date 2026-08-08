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
//! Each action POST extracts signals, authorizes, then delegates the game
//! mutation to [`crate::flow`] (which drives [`GameState::apply_action`] and
//! the post-action loop) and the render/fanout glue below. The SSE read side
//! lives in [`crate::sse`].

use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Sse};
use datastar::axum::ReadSignals;
use poker_core::protocol::{BlindConfig, GameError, PlayerAction};
use serde::Deserialize;

use crate::fanout::{broadcast_state, render_full_snapshot, send_error};
use crate::flow;
use crate::render;
use crate::room::{CallerCtx, Fanout, resolve_caller};

use crate::AppState;

// ---------------------------------------------------------------------------
// Signal coercion helpers
// ---------------------------------------------------------------------------

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

/// Build a [`BlindConfig`] from the `blindmins` / `blindpct` signal values that
/// the connect-screen and in-game settings panel both send. Shared by
/// [`room_create`] and [`action_update_settings`].
fn blind_config_from_signals(
    blind_mins: &serde_json::Value,
    blind_pct: &serde_json::Value,
) -> BlindConfig {
    BlindConfig {
        interval_secs: f64_to_u64(value_as_f64(blind_mins)).saturating_mul(60),
        increase_percent: f64_to_u32(value_as_f64(blind_pct)),
    }
}

/// Parse the `stackbbs` signal into a starting-stack size, clamped to ≥ 1 BB.
fn starting_bbs_from_signals(stack_bbs: &serde_json::Value) -> u32 {
    f64_to_u32(value_as_f64(stack_bbs)).max(1)
}

// ---------------------------------------------------------------------------
// Shell page
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "shell.html")]
struct ShellTpl;

/// `GET /poker` — the shell page.
///
/// Sets a strict Content-Security-Policy. Rationale for each directive:
/// - `script-src 'self' 'unsafe-eval'` — `'self'` for datastar.js / poker.js /
///   sw-register.js; `'unsafe-eval'` is mandatory because Datastar evaluates
///   expressions via a `Function()` constructor (see the Datastar Security
///   docs). Deliberately *no* `'unsafe-inline'`: blocks `<script>`/`onerror=`
///   reflected+stored XSS, at the cost of externalizing the SW-registration
///   snippet into `static/sw-register.js`.
/// - `style-src 'self' 'unsafe-inline'` — kept permissive for future inline
///   styles; the turn-timer ring is now driven by a JS-set custom property on
///   a class rather than an inline `style=`. Style injection is low-risk.
/// - `connect-src 'self'` — every SSE stream, action POST, and the
///   `sendBeacon` leave are same-origin; blocks future exfiltration.
/// - the rest (`object-src 'none'`, `base-uri`, `form-action`,
///   `frame-ancestors 'none'`) is standard hardening; `frame-ancestors` also
///   gives clickjacking protection.
///
/// Applied here rather than in nginx so it covers poker-desktop's LAN host
/// mode too (which has no nginx in the path); nginx proxies the header
/// through unchanged for the public deployment.
#[allow(clippy::unused_async)]
pub async fn shell() -> impl IntoResponse {
    const CSP: &str = "default-src 'self'; \
        script-src 'self' 'unsafe-eval'; \
        style-src 'self' 'unsafe-inline'; \
        img-src 'self' data:; \
        font-src 'self'; \
        connect-src 'self'; \
        manifest-src 'self'; \
        object-src 'none'; \
        base-uri 'self'; \
        form-action 'self'; \
        frame-ancestors 'none'";
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, CSP),
        ],
        ShellTpl.render().unwrap_or_default(),
    )
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
    let stamped = SW.replace("__POKER_CACHE_VERSION__", env!("POKER_CACHE_VERSION"));
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

/// A signal payload that carries a room id + session token. Implemented by the
/// signal structs that share those fields ([`SessionSignals`] itself,
/// [`RaiseSignals`], [`UpdateSettingsSignals`]) so handlers can authorize from
/// any of them without rebuilding a `SessionSignals` by hand each time.
pub trait HasSession {
    /// Borrowed view of the room id / session token carried by this payload.
    fn session(&self) -> SessionSignals;
}

impl HasSession for SessionSignals {
    fn session(&self) -> Self {
        Self {
            room_id: self.room_id.clone(),
            session_token: self.session_token.clone(),
        }
    }
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

impl HasSession for RaiseSignals {
    fn session(&self) -> SessionSignals {
        SessionSignals {
            room_id: self.room_id.clone(),
            session_token: self.session_token.clone(),
        }
    }
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

impl HasSession for UpdateSettingsSignals {
    fn session(&self) -> SessionSignals {
        SessionSignals {
            room_id: self.room_id.clone(),
            session_token: self.session_token.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Room create / join / leave
// ---------------------------------------------------------------------------

pub async fn room_create(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<CreateSignals>,
) -> impl IntoResponse {
    let blind_config = blind_config_from_signals(&signals.blind_mins, &signals.blind_pct);
    let starting_bbs = starting_bbs_from_signals(&signals.stack_bbs);

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
        room.game_state.game_started && room.game_state.current_player_id() == Some(ctx.player_id)
    };
    if is_their_turn {
        flow::process_action(
            ctx.player_id,
            PlayerAction::Fold,
            0,
            &ctx.room_arc,
            &ctx.room_id,
        )
        .await;
    }

    let outcome = state
        .room_manager
        .leave_room(&ctx.room_id, ctx.player_id)
        .await;
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
    flow::start_game(ctx).await;
    no_content()
}

pub async fn action_fold(
    state: State<AppState>,
    signals: ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    dispatch_simple_action(state, signals, PlayerAction::Fold).await
}

pub async fn action_check(
    state: State<AppState>,
    signals: ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    dispatch_simple_action(state, signals, PlayerAction::Check).await
}

pub async fn action_call(
    state: State<AppState>,
    signals: ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    dispatch_simple_action(state, signals, PlayerAction::Call).await
}

pub async fn action_allin(
    state: State<AppState>,
    signals: ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    dispatch_simple_action(state, signals, PlayerAction::AllIn).await
}

/// Authorize, apply a no-amount action (`Fold` / `Check` / `Call` / `AllIn`),
/// then run the post-action `GameOver` cleanup. Returns `204` either way: the
/// action's effects land on the caller's SSE stream, not in this response.
async fn dispatch_simple_action(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
    action: PlayerAction,
) -> axum::response::Response {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    flow::process_action(ctx.player_id, action, 0, &ctx.room_arc, &ctx.room_id).await;
    maybe_cleanup_after_action(&state, &ctx).await;
    no_content()
}

pub async fn action_raise(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<RaiseSignals>,
) -> impl IntoResponse {
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    let amount = match &signals.raise_amt {
        serde_json::Value::Number(n) => u64_to_u32(n.as_u64().unwrap_or(0)),
        serde_json::Value::String(s) => f64_to_u32(s.trim().parse::<f64>().unwrap_or(0.0)),
        _ => 0,
    };
    flow::process_action(
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
    // Decide under the room lock whether to resume, then release before
    // awaiting (so we don't hold it across the deal/timer setup).
    let resume = {
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
        // Re-render (player list + controls changed). resume_after_sit_in
        // renders again if it un-pauses the game and starts a hand.
        broadcast_state(&mut room, &ctx.room_id);

        // If the game was paused waiting for players, hand off to
        // resume_after_sit_in → maybe_start_new_hand, which re-evaluates the
        // dealable count and re-pauses if the sit-in didn't reach ≥2.
        room.game_state.waiting_for_players
    };

    if resume {
        flow::resume_after_sit_in(&room_arc, &ctx.room_id).await;
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
            &GameError::NotHost.to_string(),
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
    let Some(ctx) = authorize(&state, &signals).await else {
        return no_content();
    };
    let mut room = ctx.room_arc.lock().await;
    if room.game_state.host_id != ctx.player_id {
        send_error(
            &mut room,
            &ctx.room_id,
            ctx.player_id,
            &GameError::NotHost.to_string(),
        );
        return no_content();
    }

    let config = blind_config_from_signals(&signals.blind_mins, &signals.blind_pct);
    let starting_bbs = starting_bbs_from_signals(&signals.stack_bbs);
    room.game_state.apply_settings(config, starting_bbs);

    broadcast_state(&mut room, &ctx.room_id);
    drop(room);
    no_content()
}

// ---------------------------------------------------------------------------
// Authorization helper
// ---------------------------------------------------------------------------

async fn authorize(state: &AppState, signals: &(impl HasSession + Sync)) -> Option<CallerCtx> {
    let session = signals.session();
    match resolve_caller(
        &state.room_manager,
        &session.room_id,
        &session.session_token,
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
// GameOver cleanup (HTTP-specific)
// ---------------------------------------------------------------------------

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
