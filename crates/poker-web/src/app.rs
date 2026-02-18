//! Root application component for the web frontend.
//!
//! Connects to the poker server via WebSocket, manages game state,
//! and routes between connection and game screens.

use dioxus::prelude::*;
use futures_util::StreamExt;
use poker_core::client_controller::{ClientController, PollResult};
use poker_core::game_state::ClientGameState;
use poker_core::protocol::ClientMessage;
use poker_ui::components::{action_bar, connection_screen, event_log, game_table, player_list};
use poker_ui::{Screen, UiMessage};

// ---------------------------------------------------------------------------
// Root component
// ---------------------------------------------------------------------------

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

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

/// Root `<App>` component.
#[component]
pub fn App() -> Element {
    let screen = use_signal(|| Screen::Connection);
    let game_state = use_signal(|| ClientGameState::new(""));
    let conn_error = use_signal(|| String::new());
    let ws_origin = use_signal(|| default_ws_origin());

    // Spawn the networking coroutine. Components send UiMessage via the handle.
    let _coroutine = use_coroutine(move |mut rx: UnboundedReceiver<UiMessage>| {
        let mut screen = screen;
        let mut game_state = game_state;
        let mut conn_error = conn_error;

        async move {
            // 1. Wait for a Connect message from the connection screen.
            let (name, server_url, room_id, create) = loop {
                if let Some(UiMessage::Connect {
                    name,
                    server_url,
                    room_id,
                    create,
                }) = rx.next().await
                {
                    break (name, server_url, room_id, create);
                }
            };

            // 2. Build WS URL and attempt connection.
            conn_error.set(String::new());
            let ws_url = format!("{server_url}/ws");
            let result = ClientController::connect_ws(&ws_url, &name).await;

            let mut ctrl = match result {
                Ok(c) => c,
                Err(e) => {
                    conn_error.set(format!("Connection failed: {e}"));
                    return;
                }
            };

            // 3. Send CreateRoom (if requested) then JoinRoom.
            if create {
                ctrl.send(ClientMessage::CreateRoom {
                    room_id: room_id.clone(),
                });
            }
            ctrl.send(ClientMessage::JoinRoom {
                room_id: room_id.clone(),
                name: name.clone(),
            });

            // 4. Wait for room confirmation before switching to game screen.
            //    Process events until we get RoomJoined, RoomError, or disconnect.
            loop {
                match ctrl.recv().await {
                    PollResult::Updated(changed) => {
                        game_state.set(ctrl.state.clone());
                        // Check for room-related responses in the latest events.
                        if changed.phase || changed.players {
                            // JoinedGame triggers players change — we're in.
                            if ctrl.state.our_player_id != 0 {
                                screen.set(Screen::Game);
                                break;
                            }
                        }
                    }
                    PollResult::Unknown => {}
                    PollResult::Error | PollResult::Disconnected => {
                        conn_error.set("Disconnected before joining room".to_string());
                        return;
                    }
                    PollResult::Empty => {}
                }
            }

            // 5. Main event loop: network events + UI actions.
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
                                break;
                            }
                            PollResult::Empty => {}
                        }
                    }
                    msg = rx.next() => {
                        match msg {
                            Some(UiMessage::Action(client_msg)) => {
                                ctrl.send(client_msg);
                            }
                            Some(UiMessage::Connect { .. }) => {
                                // Ignore duplicate connect requests.
                            }
                            None => break,
                        }
                    }
                }
            }
        }
    });

    let origin = ws_origin.read().clone();

    rsx! {
        document::Stylesheet { href: TAILWIND_CSS }
        div { class: "min-h-screen h-screen bg-gray-900 text-white font-sans",
            match &*screen.read() {
                Screen::Connection => rsx! {
                    connection_screen::ConnectionScreen { error: conn_error, default_server: origin }
                },
                Screen::Game => rsx! {
                    div { class: "flex h-full",
                        // Left sidebar: player list
                        div { class: "w-64 bg-gray-800 border-r border-gray-700 flex flex-col",
                            player_list::PlayerList { state: game_state }
                        }
                        // Main area
                        div { class: "flex-1 flex flex-col",
                            // Game table (top part, takes available space)
                            div { class: "flex-1 flex flex-col",
                                game_table::GameTable { state: game_state }
                            }
                            // Action bar
                            action_bar::ActionBar { state: game_state }
                            // Event log (fixed height at bottom)
                            div { class: "h-48 border-t border-gray-700",
                                event_log::EventLog { state: game_state }
                            }
                        }
                    }
                },
            }
        }
    }
}
