//! Client orchestrator — connects networking, game state, and the TUI frontend.
//!
//! This module owns the event loop and drives:
//! - [`poker_core::client_controller::ClientController`] — shared dispatch logic
//! - [`crate::tui::Tui`] — ratatui TUI frontend
//!
//! This module is specific to the TUI binary.

use crate::tui::{Tui, UserIntent};
use poker_core::client_controller::{ClientController, PollResult};
use poker_core::protocol::{BlindConfig, ClientMessage};

/// Start the poker client, connecting via WebSocket to the given server/room.
///
/// If `create` is true, sends `CreateRoom` before `JoinRoom`.
pub async fn start_client(
    server_url: &str,
    room_id: &str,
    name: &str,
    create: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Build the WS URL (append /ws if the user didn't already).
    let ws_url = if server_url.ends_with("/ws") {
        server_url.to_string()
    } else {
        format!("{}/ws", server_url.trim_end_matches('/'))
    };

    let mut ctrl = ClientController::connect_ws(&ws_url, name).await?;

    // Send CreateRoom (if requested) then JoinRoom.
    if create {
        ctrl.send(ClientMessage::CreateRoom {
            room_id: room_id.to_string(),
            blind_config: BlindConfig::default(),
            starting_bbs: 50,
        });
    }
    ctrl.send(ClientMessage::JoinRoom {
        room_id: room_id.to_string(),
        name: name.to_string(),
    });

    // Wait for room confirmation before entering the TUI.
    loop {
        match ctrl.recv().await {
            PollResult::Updated(changed) => {
                if (changed.phase || changed.players) && ctrl.state.our_player_id != 0 {
                    break; // Successfully joined.
                }
                // Check for room errors surfaced as events.
                if let Some(last) = ctrl.state.events.back()
                    && let poker_core::game_state::GameEvent::ServerError { message } = last
                {
                    return Err(message.clone().into());
                }
            }
            PollResult::Disconnected => {
                return Err("Disconnected before joining room".into());
            }
            PollResult::Empty => {}
        }
    }

    // Launch TUI and run the main event loop.
    let mut tui = Tui::setup()?;
    let result = run_event_loop(&mut tui, &mut ctrl).await;
    tui.teardown()?;
    result
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

async fn run_event_loop(
    tui: &mut Tui,
    ctrl: &mut ClientController,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        tui.render(&ctrl.state)?;

        let timeout = tokio::time::Duration::from_millis(50);

        tokio::select! {
            poll = ctrl.recv() => {
                match poll {
                    PollResult::Updated(changed) => {
                        if changed.actions {
                            tui.on_actions_changed(&ctrl.state);
                        }
                    }
                    PollResult::Disconnected => {
                        tui.render(&ctrl.state)?;
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        break;
                    }
                    PollResult::Empty => {}
                }
            }

            _ = tokio::time::sleep(timeout) => {
                match tui.poll_and_handle_input(&ctrl.state)? {
                    UserIntent::Quit => break,
                    UserIntent::Send(msg) => {
                        ctrl.send(msg);
                    }
                    UserIntent::Feedback(text, category) => {
                        ctrl.add_message(text, category);
                    }
                    UserIntent::None => {}
                }
            }
        }
    }

    Ok(())
}
