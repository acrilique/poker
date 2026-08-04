// poker.js — helpers invoked from Datastar expressions (data-signals /
// data-on).
//
// Datastar evaluates expressions in a scope where these top-level globals are
// visible. Same pattern as the main site's app.js.
//
// The long-lived `GET /poker/events` stream is opened declaratively, not from
// here: `<body>` carries a `data-init` that opens it once on page load when a
// persisted session exists (reload path), and the create/join SSE response
// appends a one-shot trigger div whose `data-init` opens it for a fresh join.
// This file keeps only the per-patch localStorage sync and the exit helper.

// ---------------------------------------------------------------------------
// Session / identity persistence (localStorage)
// ---------------------------------------------------------------------------

const POKER_KEYS = {
  room: "poker.roomId",
  token: "poker.sessionToken",
  name: "poker.name",
};

function pokerLoadRoom() {
  return localStorage.getItem(POKER_KEYS.room) || "";
}
function pokerLoadToken() {
  return localStorage.getItem(POKER_KEYS.token) || "";
}
function pokerLoadName() {
  return localStorage.getItem(POKER_KEYS.name) || "";
}

function pokerSaveSession(roomId, token) {
  localStorage.setItem(POKER_KEYS.room, roomId);
  localStorage.setItem(POKER_KEYS.token, token);
}

function pokerSaveName(name) {
  if (name) localStorage.setItem(POKER_KEYS.name, name);
}

function pokerClearSession() {
  localStorage.removeItem(POKER_KEYS.room);
  localStorage.removeItem(POKER_KEYS.token);
}

// Set by pokerExit() so pokerHandleFetch skips re-persisting the session on
// the final datastar-fetch `finished` event fired during teardown. Without it,
// exit would be a no-op: the in-memory signals still hold the token, so the
// reload below would resurrect the session.
let _exiting = false;

// Exit: clear the session and reload back to the connect screen.
function pokerExit() {
  _exiting = true;
  pokerClearSession();
  location.reload();
}

// ---------------------------------------------------------------------------
// Live blinds-timer countdown
// ---------------------------------------------------------------------------

// The server bakes an absolute epoch-ms deadline into #blinds-timer on every
// #table morph (data-blind-deadline). A single global interval ticks the text
// down using deadline - Date.now(), so the #table fat-morph — which fires on
// every action — can't restart the countdown: a re-baked identical deadline
// just keeps ticking. Reconnects resume in sync automatically because the
// deadline is absolute, not relative.
let _blindsTimerStarted = false;

function pokerBlindsTick() {
  const el = document.getElementById("blinds-timer");
  if (!el) return;
  const text = document.getElementById("blinds-timer-text");
  if (!text) return;
  const raw = el.getAttribute("data-blind-deadline") || "";
  if (!raw || el.getAttribute("data-blind-enabled") !== "true") {
    text.textContent = "";
    el.classList.remove("blinds-soon");
    return;
  }
  // parseInt is fine here: the server emits a plain u128 decimal string.
  const deadline = parseInt(raw, 10);
  if (!Number.isFinite(deadline)) {
    text.textContent = "";
    return;
  }
  const remainingMs = Math.max(0, deadline - Date.now());
  const totalSecs = Math.floor(remainingMs / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  text.textContent = m + ":" + (s < 10 ? "0" + s : s);
  // Warn state in the final 10 seconds.
  if (totalSecs <= 10) {
    el.classList.add("blinds-soon");
  } else {
    el.classList.remove("blinds-soon");
  }
}

// Start the interval once per tab. Guarded so repeated inits (e.g. a reconnect
// morph) can't stack intervals.
function pokerBlindsTickStart() {
  if (_blindsTimerStarted) return;
  _blindsTimerStarted = true;
  // 250ms is smooth for a seconds-precision display and cheap on idle.
  setInterval(pokerBlindsTick, 250);
  pokerBlindsTick();
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", pokerBlindsTickStart);
} else {
  pokerBlindsTickStart();
}

// ---------------------------------------------------------------------------
// Chips / big-blinds display formatting
// ---------------------------------------------------------------------------

// The server bakes raw chip counts into the morphed HTML and pushes the current
// big blind as a `$bb` signal on every state morph. A local `$_mode` signal
// ('bb' | 'chips') — underscore-prefixed so it never reaches the backend —
// drives all formatting client-side. Toggling it is therefore instant and
// involves no server round-trip, so a player switching their own view can never
// broadcast to others. This mirrors poker-ui's `StackDisplayMode`.

const POKER_MODE_KEY = "poker.displayMode";

function pokerLoadMode() {
  return localStorage.getItem(POKER_MODE_KEY) || "bb";
}

function pokerSaveMode(mode) {
  if (mode) localStorage.setItem(POKER_MODE_KEY, mode);
}

// Format a raw chip count for display: 'bb' mode shows big-blind multiples
// ("12.5 BB" / "12 BB"), 'chips' shows the raw number. Falls back to chips
// when the big blind isn't known yet (bb == 0, lobby / pre-start) to avoid
// div-by-zero. Ported from poker-ui's `format_stack`. `bb` is passed from the
// `$bb` signal the server patches on every morph.
function pokerFmt(chips, bb, mode) {
  if (chips == null || chips === "") return "";
  const n = Number(chips);
  if (!Number.isFinite(n)) return String(chips);
  const b = Number(bb) || 0;
  if (mode !== "bb" || !b) return String(n);
  const v = n / b;
  return Number.isInteger(v) ? v + " BB" : v.toFixed(1) + " BB";
}

// Convert a raise amount typed in display units into raw chips for the server.
// In 'bb' mode the player types big blinds (e.g. 2.5 → 2.5 BB worth of chips);
// in 'chips' mode the value passes through unchanged. Returns a string because
// the raise input is string-valued.
function pokerToChips(input, bb, mode) {
  if (input == null || input === "") return "";
  const n = Number(input);
  if (!Number.isFinite(n)) return "";
  if (mode === "bb" && bb) return String(Math.round(n * Number(bb)));
  return String(n);
}

// Inverse of pokerToChips: convert a raw chip amount (a preset button's value)
// into the display unit so the raise input shows the right number. In 'bb'
// mode a 250-chip preset at 50/BB shows "5".
function pokerToDisplay(chips, bb, mode) {
  if (chips == null || chips === "") return "";
  const n = Number(chips);
  if (!Number.isFinite(n)) return "";
  if (mode === "bb" && bb) {
    const v = n / Number(bb);
    return Number.isInteger(v) ? String(v) : v.toFixed(1);
  }
  return String(n);
}

// ---------------------------------------------------------------------------
// datastar-fetch lifecycle: persist session
// ---------------------------------------------------------------------------

// The live signal values are passed in from the `data-on:datastar-fetch`
// expression as arguments (`$sessiontoken`, `$roomid`) — they're the
// authoritative identity, patched by the create/join response. Mirror them
// into localStorage so a reload can re-open the stream, and clear it when the
// server blanks them (game-over teardown).
function pokerHandleFetch(token, room) {
  // See _exiting: skip the teardown-time `finished` event so we don't undo
  // pokerClearSession().
  if (_exiting) return;
  if (token && room) {
    pokerSaveSession(room, token);
  } else {
    pokerClearSession();
  }
}

// ---------------------------------------------------------------------------
// Expose to Datastar's expression scope.
// ---------------------------------------------------------------------------

window.pokerLoadRoom = pokerLoadRoom;
window.pokerLoadToken = pokerLoadToken;
window.pokerLoadName = pokerLoadName;
window.pokerSaveSession = pokerSaveSession;
window.pokerSaveName = pokerSaveName;
window.pokerClearSession = pokerClearSession;
window.pokerExit = pokerExit;
window.pokerHandleFetch = pokerHandleFetch;
window.pokerLoadMode = pokerLoadMode;
window.pokerSaveMode = pokerSaveMode;
window.pokerFmt = pokerFmt;
window.pokerToChips = pokerToChips;
window.pokerToDisplay = pokerToDisplay;
