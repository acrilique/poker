//! Platform-agnostic Dioxus application lifecycle logic.
//!
//! Contains the game loop, reconnection state machine, and session recovery
//! — parameterised over a [`SessionStore`](poker_client::session::SessionStore)
//! and an async sleep function so that platform crates (poker-web, a future
//! desktop crate, etc.) only need to provide thin adapters.
//!
//! Lower-level primitives (`try_rejoin`, `SessionStore`, reconnect constants)
//! live in [`poker_client::session`].

use dioxus::prelude::*;
use futures_util::StreamExt;
use poker_client::client_controller::{ClientController, PollResult};
use poker_client::game_state::{ClientGameState, LogCategory};
use poker_client::session::{self, MAX_RECONNECT_ATTEMPTS, RECONNECT_BASE_DELAY_MS, SessionStore};
use poker_core::protocol::ClientMessage;

use crate::{Screen, UiMessage};

// ---------------------------------------------------------------------------
// Game loop
// ---------------------------------------------------------------------------

/// Why the game loop ended.
pub enum GameLoopExit {
    /// Connection dropped (network error, server closed, etc.).
    Disconnected,
    /// User deliberately chose to exit the game.
    UserExit,
}

/// Run the main game loop, returning when the connection drops or the user
/// exits.
pub async fn game_loop(
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
                    PollResult::Disconnected => {
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

// ---------------------------------------------------------------------------
// Reconnection wrapper
// ---------------------------------------------------------------------------

/// Run the game loop with automatic reconnection on disconnect.
///
/// When the WebSocket drops, this function will attempt up to
/// [`MAX_RECONNECT_ATTEMPTS`] to rejoin using the saved session token,
/// with exponential back-off between attempts.
///
/// `sleep_ms` is an async function that sleeps for the given number of
/// milliseconds — callers provide a platform-appropriate implementation
/// (e.g. `gloo_timers` on web, `tokio::time::sleep` on native).
#[allow(clippy::too_many_arguments)]
pub async fn run_with_reconnect<F, Fut>(
    ctrl: &mut ClientController,
    rx: &mut UnboundedReceiver<UiMessage>,
    game_state: &mut Signal<ClientGameState>,
    screen: &mut Signal<Screen>,
    ws_url: &str,
    room_id: &str,
    name: &str,
    session: &dyn SessionStore,
    sleep_ms: F,
) where
    F: Fn(u64) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    loop {
        // Run the normal game loop until disconnect or user exit.
        match game_loop(ctrl, rx, game_state).await {
            GameLoopExit::UserExit => {
                // User deliberately exited — no reconnection.
                session.clear();
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
            sleep_ms(delay).await;

            ctrl.state.add_message(
                format!(
                    "Reconnection attempt {} of {MAX_RECONNECT_ATTEMPTS}…",
                    attempt + 1
                ),
                LogCategory::System,
            );
            game_state.set(ctrl.state.clone());

            if let Some(new_ctrl) = session::try_rejoin(ws_url, room_id, name, &session_token).await
            {
                *ctrl = new_ctrl;
                session.save(ws_url, room_id, name, &ctrl.state.session_token);
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

// ---------------------------------------------------------------------------
// Full coroutine lifecycle
// ---------------------------------------------------------------------------

/// Drive the entire application session lifecycle.
///
/// This is the async body that a Dioxus `use_coroutine` should run. It:
///
/// 1. Attempts to recover a previous session via `session.load()`.
/// 2. Enters a loop waiting for [`UiMessage::Connect`], connecting,
///    joining a room, and running the game loop with auto-reconnect.
///
/// Platform crates only need to provide a [`SessionStore`] and a sleep
/// function.
pub async fn run_app_session<F, Fut>(
    mut rx: UnboundedReceiver<UiMessage>,
    mut screen: Signal<Screen>,
    mut game_state: Signal<ClientGameState>,
    mut conn_error: Signal<String>,
    session: impl SessionStore,
    sleep_ms: F,
) where
    F: Fn(u64) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    // ── Check for a saved session from a previous page load ──────────
    if let Some((ws_url, room_id, name, session_token)) = session.load() {
        if let Some(mut ctrl) = session::try_rejoin(&ws_url, &room_id, &name, &session_token).await
        {
            // Update the session token (may have been refreshed).
            session.save(&ws_url, &room_id, &name, &ctrl.state.session_token);
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
                &session,
                &sleep_ms,
            )
            .await;
            session.clear();
            // Fall through to the loop below so the user can
            // create/join again without reloading.
        } else {
            session.clear();
        }
    }

    // ── Main coroutine loop: keeps running so we can handle
    //    successive Connect requests without a page reload. ────────────
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
                        session.save(&ws_url, &room_id, &name, &ctrl.state.session_token);
                        screen.set(Screen::Game);
                        break true;
                    }
                }
                PollResult::Disconnected => {
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
            &session,
            &sleep_ms,
        )
        .await;
        session.clear();
    }
}
