//! Root application component for the web frontend.
//!
//! Connects to the poker server via WebSocket, manages game state,
//! and routes between connection and game screens.
//!
//! Supports automatic session recovery: when a player disconnects (network
//! drop or page reload) the server holds their seat for several minutes.
//! The client persists the session token in `sessionStorage` and attempts
//! to rejoin transparently on reconnect.

use dioxus::prelude::*;
use futures_util::StreamExt;
use poker_core::client_controller::{ClientController, PollResult};
use poker_core::game_state::{ClientGameState, LogCategory};
use poker_core::protocol::ClientMessage;
use poker_ui::components::{action_bar, connection_screen, event_log, game_table, player_list};
use poker_ui::{Screen, StackDisplayMode, UiMessage};

// ---------------------------------------------------------------------------
// Root component
// ---------------------------------------------------------------------------

const TAILWIND_CSS: Asset = asset!(
    "/assets/tailwind.css",
    AssetOptions::css()
        .with_preload(true)
        .with_static_head(true)
);

/// Maximum number of automatic reconnection attempts before giving up.
const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// Base delay between reconnection attempts (doubles each attempt).
const RECONNECT_BASE_DELAY_MS: u64 = 1_000;

// ---------------------------------------------------------------------------
// Session persistence helpers (sessionStorage)
// ---------------------------------------------------------------------------

fn save_session(ws_url: &str, room_id: &str, name: &str, session_token: &str) {
    let window = web_sys::window().unwrap();
    if let Ok(Some(storage)) = window.session_storage() {
        let _ = storage.set_item("poker_ws_url", ws_url);
        let _ = storage.set_item("poker_room_id", room_id);
        let _ = storage.set_item("poker_name", name);
        let _ = storage.set_item("poker_session_token", session_token);
    }
}

fn load_session() -> Option<(String, String, String, String)> {
    let window = web_sys::window()?;
    let storage = window.session_storage().ok()??;
    let ws_url = storage.get_item("poker_ws_url").ok()??;
    let room_id = storage.get_item("poker_room_id").ok()??;
    let name = storage.get_item("poker_name").ok()??;
    let token = storage.get_item("poker_session_token").ok()??;
    if token.is_empty() {
        return None;
    }
    Some((ws_url, room_id, name, token))
}

fn clear_session() {
    let window = web_sys::window().unwrap();
    if let Ok(Some(storage)) = window.session_storage() {
        let _ = storage.remove_item("poker_ws_url");
        let _ = storage.remove_item("poker_room_id");
        let _ = storage.remove_item("poker_name");
        let _ = storage.remove_item("poker_session_token");
    }
}

/// Derive the WebSocket URL from the browser's current page origin.
///
/// `http://host:port` → `ws://host:port`, `https://…` → `wss://…`.
fn default_ws_origin() -> String {
    let window = web_sys::window().expect("no global `window`");
    let location = window.location();
    let protocol = location.protocol().unwrap_or_default();
    let host = location.host().unwrap_or_default();
    let ws_scheme = if protocol == "https:" { "wss" } else { "ws" };
    format!("{ws_scheme}://{host}")
}

// ---------------------------------------------------------------------------
// Reconnection helper
// ---------------------------------------------------------------------------

/// Attempt to rejoin using a saved session. Returns a connected controller
/// on success, or `None` if the session is invalid / expired.
async fn try_rejoin(
    ws_url: &str,
    room_id: &str,
    name: &str,
    session_token: &str,
) -> Option<ClientController> {
    let mut ctrl = ClientController::connect_ws(ws_url, name).await.ok()?;
    ctrl.send(ClientMessage::Rejoin {
        room_id: room_id.to_string(),
        session_token: session_token.to_string(),
    });

    // Wait for Rejoined or an error.
    loop {
        match ctrl.recv().await {
            PollResult::Updated(changed) => {
                if changed.players || changed.phase {
                    // Rejoined triggers players + phase change.
                    if ctrl.state.our_player_id != 0 && !ctrl.state.room_id.is_empty() {
                        return Some(ctrl);
                    }
                }
                // Check if the latest event is an error (session expired).
                if let Some(ev) = ctrl.state.events.back()
                    && matches!(ev, poker_core::game_state::GameEvent::ServerError { .. })
                {
                    return None;
                }
            }
            PollResult::Error | PollResult::Disconnected => return None,
            _ => {}
        }
    }
}

/// Why the game loop ended.
enum GameLoopExit {
    /// Connection dropped (network error, server closed, etc.).
    Disconnected,
    /// User deliberately chose to exit the game.
    UserExit,
}

/// Run the main game loop, returning when the connection drops or the user exits.
async fn game_loop(
    ctrl: &mut ClientController,
    rx: &mut UnboundedReceiver<UiMessage>,
    game_state: &mut Signal<ClientGameState>,
) -> GameLoopExit {
    loop {
        tokio::select! {
            poll = ctrl.recv() => {
                match poll {
                    PollResult::Updated(_changed) => {
                        game_state.set(ctrl.state.clone());
                    }
                    PollResult::Unknown => {}
                    PollResult::Error | PollResult::Disconnected => {
                        game_state.set(ctrl.state.clone());
                        return GameLoopExit::Disconnected;
                    }
                    PollResult::Empty => {}
                }
            }
            msg = rx.next() => {
                match msg {
                    Some(UiMessage::Action(client_msg)) => {
                        ctrl.send(client_msg);
                    }
                    Some(UiMessage::ExitGame) => {
                        return GameLoopExit::UserExit;
                    }
                    Some(UiMessage::Connect { .. }) => {
                        // Ignore duplicate connect requests.
                    }
                    None => return GameLoopExit::Disconnected,
                }
            }
        }
    }
}

/// Root `<App>` component.
#[component]
pub fn App() -> Element {
    let screen = use_signal(|| Screen::Connection);
    let game_state = use_signal(|| ClientGameState::new(""));
    let conn_error = use_signal(String::new);
    let ws_origin = use_signal(default_ws_origin);

    // Shared display mode for stacks (blinds vs chips). Default: blinds.
    use_context_provider(|| Signal::new(StackDisplayMode::Blinds));

    // Spawn the networking coroutine. Components send UiMessage via the handle.
    let _coroutine = use_coroutine(move |mut rx: UnboundedReceiver<UiMessage>| {
        let mut screen = screen;
        let mut game_state = game_state;
        let mut conn_error = conn_error;

        async move {
            // ── Check for a saved session from a previous page load ──────
            if let Some((ws_url, room_id, name, session_token)) = load_session() {
                if let Some(mut ctrl) = try_rejoin(&ws_url, &room_id, &name, &session_token).await {
                    // Update the session token (may have been refreshed).
                    save_session(&ws_url, &room_id, &name, &ctrl.state.session_token);
                    game_state.set(ctrl.state.clone());
                    screen.set(Screen::Game);

                    // Enter the game loop with reconnection support.
                    run_with_reconnect(
                        &mut ctrl,
                        &mut rx,
                        &mut game_state,
                        &mut screen,
                        &ws_url,
                        &room_id,
                        &name,
                    )
                    .await;
                    clear_session();
                    // Fall through to the loop below so the user can
                    // create/join again without reloading.
                } else {
                    clear_session();
                }
            }

            // ── Main coroutine loop: keeps running so we can handle
            //    successive Connect requests without a page reload. ────────
            loop {
                screen.set(Screen::Connection);
                game_state.set(ClientGameState::new(""));

                // Wait for a Connect message from the connection screen.
                let (name, server_url, room_id, create, blind_config, starting_bbs) = loop {
                    if let Some(UiMessage::Connect {
                        name,
                        server_url,
                        room_id,
                        create,
                        blind_config,
                        starting_bbs,
                    }) = rx.next().await
                    {
                        break (
                            name,
                            server_url,
                            room_id,
                            create,
                            blind_config,
                            starting_bbs,
                        );
                    }
                };

                // Build WS URL and attempt connection.
                conn_error.set(String::new());
                let ws_url = format!("{server_url}/ws");
                let result = ClientController::connect_ws(&ws_url, &name).await;

                let mut ctrl = match result {
                    Ok(c) => c,
                    Err(e) => {
                        conn_error.set(format!("Connection failed: {e}"));
                        continue;
                    }
                };

                // Send CreateRoom (if requested) then JoinRoom.
                if create {
                    ctrl.send(ClientMessage::CreateRoom {
                        room_id: room_id.clone(),
                        blind_config,
                        starting_bbs,
                    });
                }
                ctrl.send(ClientMessage::JoinRoom {
                    room_id: room_id.clone(),
                    name: name.clone(),
                });

                // Wait for room confirmation before switching to game screen.
                let joined = loop {
                    match ctrl.recv().await {
                        PollResult::Updated(changed) => {
                            game_state.set(ctrl.state.clone());
                            if (changed.phase || changed.players) && ctrl.state.our_player_id != 0 {
                                // Persist session for reconnection.
                                save_session(&ws_url, &room_id, &name, &ctrl.state.session_token);
                                screen.set(Screen::Game);
                                break true;
                            }
                        }
                        PollResult::Unknown => {}
                        PollResult::Error | PollResult::Disconnected => {
                            conn_error.set("Disconnected before joining room".to_string());
                            break false;
                        }
                        PollResult::Empty => {}
                    }
                };

                if !joined {
                    continue;
                }

                // Main game loop with reconnection support.
                run_with_reconnect(
                    &mut ctrl,
                    &mut rx,
                    &mut game_state,
                    &mut screen,
                    &ws_url,
                    &room_id,
                    &name,
                )
                .await;
                clear_session();
            }
        }
    });

    let origin = ws_origin.read().clone();

    rsx! {
        document::Stylesheet { href: TAILWIND_CSS }
        document::Link { rel: "manifest", href: "/poker/manifest.json" }
        document::Meta { name: "theme-color", content: "#1A130D" }
        document::Link { rel: "icon", href: "/poker/favicon.ico" }
        document::Script {
            r#"
            if ("serviceWorker" in navigator) {{
                navigator.serviceWorker.register("/poker/sw.js", {{ scope: "/poker/" }});
            }}
            "#
        }
        div { class: "min-h-screen bg-base text-foreground font-sans",
            match &*screen.read() {
                Screen::Connection => rsx! {
                    connection_screen::ConnectionScreen { error: conn_error, default_server: origin }
                },
                Screen::Game => rsx! {
                    div { class: "flex h-screen portrait-rotate",
                        // Left sidebar: player list
                        div { class: "w-64 bg-surface border-r border-muted/50 flex flex-col",
                            player_list::PlayerList { state: game_state }
                        }
                        // Main area
                        div { class: "flex-1 flex flex-col relative",
                            // Game table (top part, takes available space)
                            div { class: "flex-1 flex flex-col",
                                game_table::GameTable { state: game_state }
                            }
                            // Action bar
                            action_bar::ActionBar { state: game_state }
                            // Event log: always visible on large screens, hidden overlay on small
                            div { class: "hidden lg:block h-48 border-t border-muted/50",
                                event_log::EventLog { state: game_state }
                            }
                            // Mobile log overlay (managed by the GameLogOverlay component)
                            div { class: "lg:hidden",
                                GameLogOverlay { state: game_state }
                            }
                        }
                    }
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Mobile log overlay — toggle button + full overlay
// ---------------------------------------------------------------------------

/// A small component that shows a "Logs" toggle button at the bottom-right
/// of the game area. When tapped, an overlay with the event log covers the
/// board + action bar.
#[component]
fn GameLogOverlay(state: Signal<ClientGameState>) -> Element {
    let mut show_log = use_signal(|| false);
    let visible = *show_log.read();

    rsx! {
        if visible {
            // Overlay covering the main area
            div {
                class: "absolute inset-0 z-40 bg-base/95 overflow-y-auto",
                event_log::EventLog { state }
            }
        }
        // Toggle button always pinned to the bottom-right of the main area
        // Rendered after the overlay so it paints on top
        div { class: "absolute bottom-14 right-3 z-50",
            button {
                class: "px-3 py-1.5 bg-surface/80 hover:bg-muted rounded-lg text-xs font-semibold text-foreground/70 shadow-lg transition backdrop-blur-sm",
                onclick: move |_| show_log.set(!visible),
                if visible { "✕ Close" } else { "📋 Logs" }
            }
        }
    }
}

/// Run the game loop with automatic reconnection on disconnect.
///
/// When the WebSocket drops, this function will attempt up to
/// [`MAX_RECONNECT_ATTEMPTS`] to rejoin using the saved session token,
/// with exponential back-off between attempts.
async fn run_with_reconnect(
    ctrl: &mut ClientController,
    rx: &mut UnboundedReceiver<UiMessage>,
    game_state: &mut Signal<ClientGameState>,
    screen: &mut Signal<Screen>,
    ws_url: &str,
    room_id: &str,
    name: &str,
) {
    loop {
        // Run the normal game loop until disconnect or user exit.
        match game_loop(ctrl, rx, game_state).await {
            GameLoopExit::UserExit => {
                // User deliberately exited — no reconnection.
                clear_session();
                screen.set(Screen::Connection);
                *game_state.write() = ClientGameState::new(name);
                return;
            }
            GameLoopExit::Disconnected => {
                // Fall through to reconnection logic below.
            }
        }

        // Disconnected — try to rejoin.
        let session_token = ctrl.state.session_token.clone();
        if session_token.is_empty() {
            break; // No session token, can't reconnect.
        }

        ctrl.state.add_message(
            "Connection lost. Attempting to reconnect…".to_string(),
            LogCategory::System,
        );
        game_state.set(ctrl.state.clone());

        let mut reconnected = false;
        for attempt in 0..MAX_RECONNECT_ATTEMPTS {
            let delay = RECONNECT_BASE_DELAY_MS * 2u64.pow(attempt);
            gloo_timers::future::TimeoutFuture::new(delay as u32).await;

            ctrl.state.add_message(
                format!(
                    "Reconnection attempt {} of {MAX_RECONNECT_ATTEMPTS}…",
                    attempt + 1
                ),
                LogCategory::System,
            );
            game_state.set(ctrl.state.clone());

            if let Some(new_ctrl) = try_rejoin(ws_url, room_id, name, &session_token).await {
                *ctrl = new_ctrl;
                save_session(ws_url, room_id, name, &ctrl.state.session_token);
                game_state.set(ctrl.state.clone());
                reconnected = true;
                break;
            }
        }

        if !reconnected {
            ctrl.state.add_message(
                "Could not reconnect. Session may have expired.".to_string(),
                LogCategory::Error,
            );
            game_state.set(ctrl.state.clone());
            break;
        }
    }
}
