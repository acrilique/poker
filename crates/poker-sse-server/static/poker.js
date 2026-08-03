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

// Exit: clear the session and reload back to the connect screen.
function pokerExit() {
  pokerClearSession();
  location.reload();
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
