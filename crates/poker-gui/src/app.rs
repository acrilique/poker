//! Root application component — state management, coroutine bridge, screen routing.

use dioxus::prelude::*;
use futures_util::StreamExt;
use poker_core::client_controller::{ClientController, PollResult};
use poker_core::game_state::ClientGameState;
use poker_core::protocol::ClientMessage;

use poker_ui::components::{action_bar, connection_screen, event_log, game_table, player_list};
use poker_ui::{Screen, StackDisplayMode, UiMessage};

// ---------------------------------------------------------------------------
// Root component
// ---------------------------------------------------------------------------

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

/// Root `<App>` component.
#[component]
pub fn App() -> Element {
    let screen = use_signal(|| Screen::Connection);
    let game_state = use_signal(|| ClientGameState::new(""));
    let conn_error = use_signal(String::new);

    // Shared display mode for stacks (blinds vs chips). Default: blinds.
    use_context_provider(|| Signal::new(StackDisplayMode::Blinds));

    // Spawn the networking coroutine. Components send UiMessage via the handle.
    let _coroutine = use_coroutine(move |mut rx: UnboundedReceiver<UiMessage>| {
        let mut screen = screen;
        let mut game_state = game_state;
        let mut conn_error = conn_error;

        async move {
            // Main coroutine loop: keeps running so we can handle
            // successive Connect requests without restarting the app.
            loop {
                screen.set(Screen::Connection);
                game_state.set(ClientGameState::new(""));

                // 1. Wait for a Connect message from the connection screen.
                let (name, server_url, room_id, create, blind_config) = loop {
                    if let Some(UiMessage::Connect {
                        name,
                        server_url,
                        room_id,
                        create,
                        blind_config,
                    }) = rx.next().await
                    {
                        break (name, server_url, room_id, create, blind_config);
                    }
                };

                // 2. Build WS URL and attempt connection.
                conn_error.set(String::new());
                let ws_url = if server_url.starts_with("ws://") || server_url.starts_with("wss://") {
                    format!("{server_url}/ws")
                } else {
                    format!("ws://{server_url}/ws")
                };
                let result = ClientController::connect_ws(&ws_url, &name).await;

                let mut ctrl = match result {
                    Ok(c) => c,
                    Err(e) => {
                        conn_error.set(format!("Connection failed: {e}"));
                        continue;
                    }
                };

                // 3. Send CreateRoom (if requested) then JoinRoom.
                if create {
                    ctrl.send(ClientMessage::CreateRoom {
                        room_id: room_id.clone(),
                        blind_config,
                    });
                }
                ctrl.send(ClientMessage::JoinRoom {
                    room_id: room_id.clone(),
                    name: name.clone(),
                });

                // 4. Wait for room confirmation before switching to game screen.
                let joined = loop {
                    match ctrl.recv().await {
                        PollResult::Updated(changed) => {
                            game_state.set(ctrl.state.clone());
                            if (changed.phase || changed.players) && ctrl.state.our_player_id != 0 {
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
                                Some(UiMessage::ExitGame) => {
                                    break;
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
        }
    });

    rsx! {
        document::Stylesheet { href: TAILWIND_CSS }
        div { class: "min-h-screen h-screen bg-gray-900 text-white font-sans",
            match &*screen.read() {
                Screen::Connection => rsx! {
                    connection_screen::ConnectionScreen { error: conn_error }
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
