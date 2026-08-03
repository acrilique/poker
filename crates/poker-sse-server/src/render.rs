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

//! Render layer: `GameState` → Datastar SSE events.
//!
//! State is rendered as morphs of the stable-ID regions (`#player-list`,
//! `#table`, `#hole-cards`, `#action-bar`, `#controls`, `#game-root`).
//! Private data (hole cards, the actionable action bar) is rendered per viewer
//! and sent only down that player's stream. Transient errors go via
//! `ExecuteScript` alerts (no morph target).

use askama::Template;
use datastar::DatastarEvent;
use datastar::consts::ElementPatchMode;
use datastar::prelude::{ExecuteScript, PatchElements, PatchSignals};
use poker_core::protocol::{CardInfo, PlayerAction};

use poker_core::game_logic::{GamePhase, GameState, PlayerStatus};

// ---------------------------------------------------------------------------
// View structs shared between render code and Askama templates
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CardView {
    pub label: String,
    pub red: bool,
}

impl From<&CardInfo> for CardView {
    fn from(c: &CardInfo) -> Self {
        Self {
            label: c.to_string(),
            red: c.suit == 0 || c.suit == 3, // Diamonds / Hearts
        }
    }
}

/// Which blind a seat is posting this hand (none/small/big). An enum (not two
/// bools) to keep `PlayerEntry` under the bool limit.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Blind {
    #[default]
    None,
    Small,
    Big,
}

/// Seat display status this hand — away / folded / out / playing. An enum
/// (not three bools) to keep `PlayerEntry` under the bool limit.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SeatStatus {
    #[default]
    Playing,
    Away,
    Folded,
    Out,
}

pub struct PlayerEntry {
    pub name: String,
    pub stack: String,
    pub bet: Option<String>,
    pub is_us: bool,
    pub blind: Blind,
    pub status: SeatStatus,
    pub is_turn: bool,
    pub timer_style: String,
}

#[derive(Clone)]
pub struct ShowdownHand {
    pub name: String,
    pub is_us: bool,
    pub cards: Vec<CardView>,
    pub line1: Option<String>,
    pub line2: Option<String>,
}

pub struct Preset {
    pub label: &'static str,
    pub amount: u32,
    pub is_allin: bool,
}

// ---------------------------------------------------------------------------
// Askama fragment templates
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "player_list.html")]
pub struct PlayerListTpl {
    pub players: Vec<PlayerEntry>,
}

/// Live blinds-timer view baked into the table header on each morph. The
/// countdown itself ticks client-side off `deadline_ms` (an absolute epoch
/// timestamp), so the `#table` fat-morph can't restart it — see
/// `pokerBlindsTick` in `static/poker.js`. `None`-valued fields mean "static
/// blinds / not yet anchored"; the template renders a static-level chip then.
#[derive(Clone)]
pub struct BlindsTimerView {
    pub small_blind: u32,
    pub big_blind: u32,
    pub next_small_blind: u32,
    pub next_big_blind: u32,
    /// Absolute deadline (epoch ms) for the next rise. Empty string when blinds
    /// are static or the anchor isn't set yet (lobby / pre-start).
    pub deadline_ms: String,
    /// True when rising blinds are configured and the anchor is set.
    pub enabled: bool,
}

#[derive(Template)]
#[template(path = "table.html")]
pub struct TableTpl {
    pub room_id: String,
    pub hand_number: u32,
    pub stage: String,
    pub community: Vec<Option<CardView>>,
    pub pot: String,
    pub showdown: Vec<ShowdownHand>,
    pub blinds_timer: Option<BlindsTimerView>,
}

#[derive(Template)]
#[template(path = "hole_cards.html")]
pub struct HoleCardsTpl {
    pub cards: Option<Vec<CardView>>,
    pub hand_rank: Option<String>,
}

/// Which betting actions the viewer may take. An enum-like struct (not three
/// bools) to keep `ActionBarTpl` under the bool limit.
#[derive(Clone, Copy, Default)]
pub struct ValidActions {
    pub check: bool,
    pub call: bool,
    pub raise: bool,
}

#[derive(Template)]
#[template(path = "action_bar.html")]
pub struct ActionBarTpl {
    pub active: bool,
    pub sitting_out: bool,
    pub valid: ValidActions,
    pub call_label: String,
    pub presets: Vec<Preset>,
    pub min_raise_label: String,
}

/// Host-only controls shown in the controls panel. Groups `is_host` and
/// `allow_late_entry` to keep `ControlsTpl` under the bool limit.
#[derive(Clone, Copy, Default)]
pub struct HostControls {
    pub is_host: bool,
    pub allow_late_entry: bool,
}

#[derive(Template)]
#[template(path = "controls.html")]
pub struct ControlsTpl {
    pub room_id: String,
    pub game_started: bool,
    pub host: HostControls,
    pub sitting_out: bool,
}

#[derive(Template)]
#[template(path = "game.html")]
pub struct GameTpl {
    pub player_list: String,
    pub table: String,
    pub hole_cards: String,
    pub action_bar: String,
    pub controls: String,
}

// ---------------------------------------------------------------------------
// Event constructors
// ---------------------------------------------------------------------------

fn morph(html: String) -> DatastarEvent {
    PatchElements::new(html).into_datastar_event()
}

/// Build a `PatchSignals` event from a serializable value. JSON-encoded because
/// the Datastar client parses signal patches as a JS object literal (JSON is a
/// strict subset), so escaping is always correct.
pub fn patch_signals(value: &impl serde::Serialize) -> DatastarEvent {
    match serde_json::to_string(value) {
        Ok(json) => PatchSignals::new(json).into_datastar_event(),
        // `serde_json` only errors on non-finite floats or recursion depth;
        // none of our callers hit either. Fall back to an empty patch to keep
        // the stream healthy.
        Err(e) => {
            tracing::error!(error = %e, "failed to serialize signal patch");
            PatchSignals::new("{}").into_datastar_event()
        }
    }
}

/// JS-escape a string so it's safe to splice between single quotes in a
/// `<script>`. Backslash and the quote are escaped; newlines are flattened.
/// Used only by [`alert_events`] for `alert('...')` payloads.
fn js_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "\\\\".into(),
            '\'' => "\\'".into(),
            '\n' => "\\n".into(),
            '\r' => "\\r".into(),
            _ => c.to_string(),
        })
        .collect()
}

/// Build an `ExecuteScript` event that runs `alert('...')`. Appended to
/// `<body>` by the SDK with no morph target, so it works before `#game-root`
/// exists. Returns a `Vec` to match the other render helpers.
fn alert_events(detail: &str) -> Vec<DatastarEvent> {
    let js = format!("alert('{}')", js_escape(detail));
    vec![ExecuteScript::new(js).into_datastar_event()]
}

// ---------------------------------------------------------------------------
// Region renderers (return the patch event for a single region)
// ---------------------------------------------------------------------------

/// Render an Askama template, logging and falling back to an empty string on
/// failure. Askama's `render()` is infallible for well-formed templates, so an
/// `Err` means a template/struct mismatch. Log it rather than silently
/// dropping the region.
fn render_or_log<T: Template>(tpl: T, region: &'static str) -> String {
    tpl.render().unwrap_or_else(|e| {
        tracing::error!(error = %e, region, "failed to render template");
        String::new()
    })
}


/// Render context: game state plus per-room metadata the templates need.
pub struct Ctx<'a> {
    pub gs: &'a GameState,
    pub room_id: &'a str,
    /// Seconds left on the current turn timer. Drives the CSS
    /// `--timer-duration` on the active player's row, so a reconnecting client
    /// resumes in sync instead of restarting.
    pub turn_remaining: u32,
    /// Live blinds-timer view for the table header. `None` when rising blinds
    /// aren't configured or the game hasn't started.
    pub blinds_timer: Option<BlindsTimerView>,
}

impl<'a> Ctx<'a> {
    pub fn new(gs: &'a GameState, room_id: &'a str, turn_remaining: u32) -> Self {
        Self {
            gs,
            room_id,
            turn_remaining,
            blinds_timer: build_blinds_timer(gs),
        }
    }
}

/// Derive the blinds-timer view from settled `GameState`. Returns `None` in the
/// lobby (no chip before the game starts). The deadline is an absolute epoch-ms
/// string the client ticks against; `enabled` is false for static-blinds rooms,
/// in which case the template renders the level with no countdown.
fn build_blinds_timer(gs: &GameState) -> Option<BlindsTimerView> {
    // No chip in the lobby: the timer is a live-game feature.
    if !gs.game_started {
        return None;
    }
    let (next_small, next_big) = gs.next_blinds();
    let remaining = gs.seconds_to_next_blind();
    let enabled = remaining.is_some();
    // An absolute deadline keeps reconnects and morphs in sync: the client
    // computes `deadline - Date.now()` each tick, so a re-baked deadline (same
    // value) never restarts the countdown.
    let deadline_ms = remaining.map_or_else(String::new, |secs| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let ms = u128::from(secs)
            .saturating_mul(1000)
            .saturating_add(now);
        ms.to_string()
    });
    Some(BlindsTimerView {
        small_blind: gs.small_blind,
        big_blind: gs.big_blind,
        next_small_blind: next_small,
        next_big_blind: next_big,
        deadline_ms,
        enabled,
    })
}

/// Build the per-viewer player rows shared by `player_list_events` and
/// `render_player_list`. Split out so the two public renderers stay in sync.
fn build_player_entries(ctx: &Ctx, viewer: u32) -> Vec<PlayerEntry> {
    let gs = ctx.gs;
    let turn_pid = if gs.game_started && !matches!(gs.phase, GamePhase::Lobby | GamePhase::Showdown)
    {
        gs.current_player_id()
    } else {
        None
    };

    gs.player_order
        .iter()
        .filter_map(|&pid| {
            let p = gs.players.get(&pid)?;
            let in_hand = gs.game_started && !matches!(gs.phase, GamePhase::Lobby);
            let bet = if in_hand {
                p.current_bet.min(p.chips)
            } else {
                0
            };
            let (stack, bet_text) = if in_hand {
                (
                    p.chips.saturating_sub(bet).to_string(),
                    if bet > 0 {
                        Some(format!("+{bet}"))
                    } else {
                        None
                    },
                )
            } else {
                (p.chips.to_string(), None)
            };
            let blinds_up = in_hand && gs.player_order.len() >= 2;
            let blind = if blinds_up {
                seat_blind(gs, pid)
            } else {
                Blind::None
            };
            let status = if p.sitting_out {
                SeatStatus::Away
            } else if p.status == PlayerStatus::Folded {
                SeatStatus::Folded
            } else if p.status == PlayerStatus::Out {
                SeatStatus::Out
            } else {
                SeatStatus::Playing
            };
            let is_turn = turn_pid == Some(pid);
            Some(PlayerEntry {
                name: p.name.clone(),
                stack,
                bet: bet_text,
                is_us: pid == viewer,
                blind,
                status,
                is_turn,
                timer_style: if is_turn {
                    format!("--timer-duration: {}s", ctx.turn_remaining)
                } else {
                    String::new()
                },
            })
        })
        .collect()
}

/// Safe modular seat lookup: returns the player ID `offset` seats clockwise
/// from the dealer, or `None` if the table is empty. Avoids unchecked indexing
/// and overflowing arithmetic.
fn seat_blind(gs: &GameState, pid: u32) -> Blind {
    let seats = gs.player_order.len();
    if seats < 2 {
        return Blind::None;
    }
    let sb = seat_at(gs, gs.dealer_index, 1, seats);
    let bb = seat_at(gs, gs.dealer_index, 2, seats);
    if sb == Some(pid) {
        Blind::Small
    } else if bb == Some(pid) {
        Blind::Big
    } else {
        Blind::None
    }
}

/// `(base + offset) % seats`, computed without overflowing arithmetic and
/// without indexing the seat vector.
fn seat_at(gs: &GameState, base: usize, offset: usize, seats: usize) -> Option<u32> {
    if seats == 0 {
        return None;
    }
    let idx = base.rem_euclid(seats).saturating_add(offset).rem_euclid(seats);
    gs.player_order.get(idx).copied()
}

fn render_player_list(ctx: &Ctx, viewer: u32) -> String {
    render_or_log(
        PlayerListTpl {
            players: build_player_entries(ctx, viewer),
        },
        "player_list",
    )
}

fn render_table(ctx: &Ctx, viewer: u32) -> String {
    // At showdown, reveal every non-folded player's hole cards and rank from
    // the settled state.
    let showdown = if matches!(ctx.gs.phase, GamePhase::Showdown) {
        build_showdown_overlay(ctx.gs, viewer)
    } else {
        Vec::new()
    };
    table_html(ctx, showdown)
}

/// Build the `#table` HTML from a render context and a precomputed showdown
/// overlay. Shared by [`render_table`] and [`equity_table_events`] so the
/// community-card / stage / pot logic lives in one place.
fn table_html(ctx: &Ctx, showdown: Vec<ShowdownHand>) -> String {
    let gs = ctx.gs;
    let mut community: Vec<Option<CardView>> = gs
        .community_cards
        .iter()
        .map(poker_core::protocol::card_to_info)
        .map(|c| Some(CardView::from(&c)))
        .collect();
    while community.len() < 5 {
        community.push(None);
    }

    let stage = match gs.phase {
        GamePhase::Lobby => "Lobby",
        GamePhase::PreFlop => "Preflop",
        GamePhase::Flop => "Flop",
        GamePhase::Turn => "Turn",
        GamePhase::River => "River",
        GamePhase::Showdown => "Showdown",
    }
    .to_string();

    TableTpl {
        room_id: ctx.room_id.to_string(),
        hand_number: gs.hand_number,
        stage,
        community,
        pot: gs.pot.to_string(),
        showdown,
        blinds_timer: ctx.blinds_timer.clone(),
    }
    .render()
    .unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to render table template");
        String::new()
    })
}

/// Build the showdown overlay from settled state: every Active/AllIn player's
/// revealed hole cards and made-hand rank. `line2` (the won amount) is left to
/// the winner log line — the per-winner split isn't derivable here.
fn build_showdown_overlay(gs: &GameState, viewer: u32) -> Vec<ShowdownHand> {
    let board = gs.build_board();
    gs.player_order
        .iter()
        .filter_map(|&pid| {
            let p = gs.players.get(&pid)?;
            if !matches!(
                p.status,
                PlayerStatus::Active | PlayerStatus::AllIn
            ) {
                return None;
            }
            let (c1, c2) = p.hole_cards?;
            let cards = vec![
                CardView::from(&poker_core::protocol::card_to_info(&c1)),
                CardView::from(&poker_core::protocol::card_to_info(&c2)),
            ];
            let hand = poker_core::poker::Hand(c1, c2);
            let line1 = hand
                .best(&board)
                .map(|full| format!("{}", full.rank()))
                .or_else(|| Some("Unknown".to_string()));
            Some(ShowdownHand {
                name: p.name.clone(),
                is_us: pid == viewer,
                cards,
                line1,
                line2: None,
            })
        })
        .collect()
}

fn render_hole_cards(gs: &GameState, viewer: u32) -> String {
    let cards = gs
        .players
        .get(&viewer)
        .and_then(|p| p.hole_cards)
        .map(|(c1, c2)| {
            vec![
                CardView::from(&poker_core::protocol::card_to_info(&c1)),
                CardView::from(&poker_core::protocol::card_to_info(&c2)),
            ]
        });
    let hand_rank = hole_hand_rank(gs, viewer);
    render_or_log(HoleCardsTpl { cards, hand_rank }, "hole_cards")
}

/// Compute the player's current made-hand rank (hole cards + board).
fn hole_hand_rank(gs: &GameState, viewer: u32) -> Option<String> {
    let (c1, c2) = gs.players.get(&viewer)?.hole_cards?;
    if gs.community_cards.len() < 3 {
        return None;
    }
    let board = gs.build_board();
    let hand = poker_core::poker::Hand(c1, c2);
    hand.best(&board).map(|full| format!("{}", full.rank()))
}

fn render_action_bar(gs: &GameState, viewer: u32) -> String {
    let sitting_out = gs.players.get(&viewer).is_some_and(|p| p.sitting_out);

    let is_turn = gs.game_started
        && matches!(
            gs.phase,
            GamePhase::PreFlop | GamePhase::Flop | GamePhase::Turn | GamePhase::River
        )
        && gs.current_player_id() == Some(viewer)
        && gs
            .players
            .get(&viewer)
            .is_some_and(|p| p.status == PlayerStatus::Active);

    if !is_turn {
        return inert_action_bar(sitting_out);
    }

    let Some(player) = gs.players.get(&viewer) else {
        // `is_turn` guaranteed the viewer is Active; if the map changed under
        // us, render an inert bar instead of panicking.
        return inert_action_bar(sitting_out);
    };
    let valid = gs.valid_actions(viewer);
    let to_call = gs.current_bet.saturating_sub(player.current_bet);

    let can_check = valid.contains(&PlayerAction::Check);
    let can_call = valid.contains(&PlayerAction::Call);
    let can_raise = valid.contains(&PlayerAction::Raise);
    let can_allin = valid.contains(&PlayerAction::AllIn);

    let call_label = format!("Call {to_call}");

    let max_raise = player.chips.saturating_sub(to_call);
    let pct_amount = |pct: u32| -> u32 {
        let raw = u64::from(gs.pot).saturating_mul(u64::from(pct)) / 100;
        u32::try_from(raw).unwrap_or(u32::MAX).max(gs.min_raise).min(max_raise)
    };
    let presets: Vec<Preset> = if can_raise || can_allin {
        vec![
            Preset {
                label: "35%",
                amount: pct_amount(35),
                is_allin: false,
            },
            Preset {
                label: "50%",
                amount: pct_amount(50),
                is_allin: false,
            },
            Preset {
                label: "75%",
                amount: pct_amount(75),
                is_allin: false,
            },
            Preset {
                label: "100%",
                amount: pct_amount(100),
                is_allin: false,
            },
            Preset {
                label: "All-In",
                amount: max_raise,
                is_allin: true,
            },
        ]
    } else {
        Vec::new()
    };

    let min_raise_label = if gs.min_raise > 0 && can_raise {
        format!("Min {}", gs.min_raise)
    } else {
        String::new()
    };

    render_or_log(
        ActionBarTpl {
            active: true,
            sitting_out,
            valid: ValidActions {
                check: can_check,
                call: can_call,
                raise: can_raise,
            },
            call_label,
            presets,
            min_raise_label,
        },
        "action_bar",
    )
}

/// Render an inactive action bar. Shared by the two early-return paths in
/// [`render_action_bar`].
fn inert_action_bar(sitting_out: bool) -> String {
    render_or_log(
        ActionBarTpl {
            active: false,
            sitting_out,
            valid: ValidActions::default(),
            call_label: String::new(),
            presets: Vec::new(),
            min_raise_label: String::new(),
        },
        "action_bar",
    )
}

fn render_controls(ctx: &Ctx, viewer: u32) -> String {
    let gs = ctx.gs;
    let sitting_out = gs.players.get(&viewer).is_some_and(|p| p.sitting_out);
    render_or_log(
        ControlsTpl {
            room_id: ctx.room_id.to_string(),
            game_started: gs.game_started,
            host: HostControls {
                is_host: gs.host_id == viewer,
                allow_late_entry: gs.allow_late_entry,
            },
            sitting_out,
        },
        "controls",
    )
}

/// Error helper for handlers without a room context (join/create errors).
/// Surfaces as a browser `alert` via [`datastar::prelude::ExecuteScript`], so
/// it works with no morph target — including on the connect screen.
pub fn error_events_pub(detail: &str) -> Vec<DatastarEvent> {
    alert_events(&format!("Error: {detail}"))
}

/// Like [`error_events_pub`] but for non-error notices (e.g. "This game has
/// ended"), shown without an "Error:" prefix.
pub fn notice_events(detail: &str) -> Vec<DatastarEvent> {
    alert_events(detail)
}

/// Show a transient in-the-table message in the non-modal `#toast` region.
/// Unlike [`error_events_pub`] (a blocking `alert`), the toast is an in-flow
/// element driven by the `_toast` local signal — it doesn't steal focus and is
/// replaced by the next state morph. Used for pre-action rejections that fire
/// after the player is connected, so the region is mounted.
pub fn toast_events(detail: &str) -> Vec<DatastarEvent> {
    vec![patch_signals(&serde_json::json!({ "_toast": detail }))]
}

/// Append a one-shot `<div data-init="@get('/poker/events')">` to `<body>`.
///
/// The create/join response patches the session signals in *first*, then
/// appends this trigger; Datastar's `MutationObserver` then fires its
/// `data-init`, which sees a valid `$sessiontoken` and opens the events
/// stream. The div has a stable `id="events-trigger"`; a `Remove` of any prior
/// instance is sent before the `Append` so repeated joins can't accumulate
/// duplicates. It lives on `<body>` (never re-morphed by `#game-root`), so it
/// opens the stream exactly once. `openWhenHidden: true` keeps the stream
/// alive when the tab is backgrounded.
pub fn attach_events_stream_trigger() -> Vec<DatastarEvent> {
    // Remove any prior trigger (no-op if none). `remove` mode needs a
    // selector or the client throws `PatchElementsExpectedSelector`, hence
    // `new_remove(selector)` rather than `new(html).mode(Remove)`.
    let remove = PatchElements::new_remove("#events-trigger").into_datastar_event();
    // Append a fresh one; the MutationObserver fires its data-init.
    let append = PatchElements::new(
        r#"<div id="events-trigger" style="display:none" data-init="@get('/poker/events', {openWhenHidden: true})"></div>"#,
    )
    .selector("body")
    .mode(ElementPatchMode::Append)
    .into_datastar_event();
    vec![remove, append]
}

/// Render the `#table` region with an all-in equity overlay for one viewer —
/// the one piece of UI not derivable from `GameState` (equities are computed
/// live for the all-in reveal and not stored). `hands_with_equity` is
/// `(player_id, cards, equity_percent)`. Used only by
/// `broadcast_allin_showdown`; everywhere else the showdown overlay is derived
/// from state in `render_table`.
pub fn equity_table_events(
    ctx: &Ctx,
    viewer: u32,
    hands_with_equity: &[(u32, [CardInfo; 2], f64)],
) -> DatastarEvent {
    let overlay: Vec<ShowdownHand> = hands_with_equity
        .iter()
        .map(|(pid, cards, equity)| ShowdownHand {
            name: player_name(ctx.gs, *pid),
            is_us: *pid == viewer,
            cards: cards.iter().map(CardView::from).collect(),
            line1: Some(format!("{equity:.1}%")),
            line2: None,
        })
        .collect();

    // Equity overlay replaces the state-derived showdown for the all-in reveal;
    // the rest of the table is shared with `render_table` via `table_html`.
    let html = table_html(ctx, overlay);
    morph(html)
}

// ---------------------------------------------------------------------------
// Full snapshot (initial join / reconnect)
// ---------------------------------------------------------------------------

/// Render the live state regions for one viewer as a fat-morph of `#game-root`
/// (player list, table, hole cards, action bar, controls). A pure function of
/// `(GameState, viewer)` — order-independent and idempotent.
pub fn state_events(ctx: &Ctx, viewer: u32) -> Vec<DatastarEvent> {
    let player_list = render_player_list(ctx, viewer);
    let table = render_table(ctx, viewer);
    let hole_cards = render_hole_cards(ctx.gs, viewer);
    let action_bar = render_action_bar(ctx.gs, viewer);
    let controls = render_controls(ctx, viewer);

    let game_html = render_or_log(
        GameTpl {
            player_list,
            table,
            hole_cards,
            action_bar,
            controls,
        },
        "game",
    );

    let events = vec![morph(game_html)];

    // No signals to patch: the countdown ring is driven entirely by the
    // `--timer-duration` CSS custom property on the active player's row above,
    // which the morph reapplies.

    events
}

/// Full state render for the initial join / reconnect. Alias for
/// [`state_events`], kept under this name so the join flow reads naturally.
pub fn full_snapshot(ctx: &Ctx, viewer: u32) -> Vec<DatastarEvent> {
    state_events(ctx, viewer)
}

pub fn player_name(gs: &GameState, player_id: u32) -> String {
    gs.players
        .get(&player_id)
        .map_or_else(|| format!("player #{player_id}"), |p| p.name.clone())
}
