//! WebSocket handler for the Axum poker server.
//!
//! Each WebSocket connection follows this lifecycle:
//!
//! 1. Client sends `CreateRoom` or `JoinRoom`.
//! 2. On success the connection is bound to a room + player ID.
//! 3. Subsequent `ClientMessage`s are processed against that room's
//!    [`GameState`].
//! 4. On disconnect the player is removed and the room may be cleaned up.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::game_logic::{GamePhase, GameState, PlayerStatus, TURN_TIMEOUT_SECS};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use poker_core::poker::{Hand, calculate_equity_multi};
use poker_core::protocol::{CardInfo, ClientMessage, PlayerAction, ServerMessage, card_to_info};
use tokio::sync::Mutex;

use crate::room::{PlayerRx, Room, RoomManager};

/// Drive a single WebSocket connection.
///
/// Called after the Axum upgrade; `socket` is the full-duplex WebSocket.
pub async fn handle_socket(socket: WebSocket, room_manager: Arc<RoomManager>) {
    let (ws_sink, ws_stream) = socket.split();
    let ws_sink = Arc::new(Mutex::new(ws_sink));

    // Phase 1: wait for CreateRoom / JoinRoom before entering the game loop.
    let mut ws_stream = ws_stream;
    let room_id: Option<String>;
    let player_id: Option<u32>;
    let player_rx: Option<PlayerRx>;
    let room_arc: Option<Arc<Mutex<Room>>>;

    // ── Lobby: wait for room assignment ──────────────────────────────────
    loop {
        let frame = ws_stream.next().await;
        match frame {
            Some(Ok(Message::Text(text))) => {
                let msg: ClientMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        let err = ServerMessage::Error {
                            message: format!("Invalid message: {e}"),
                        };
                        send_one(&ws_sink, &err).await;
                        continue;
                    }
                };

                match msg {
                    ClientMessage::CreateRoom {
                        room_id: ref rid,
                        blind_config,
                        starting_bbs,
                        ..
                    } => match room_manager
                        .create_room(rid, blind_config, starting_bbs)
                        .await
                    {
                        Ok(()) => {
                            let ok = ServerMessage::RoomCreated {
                                room_id: rid.clone(),
                            };
                            send_one(&ws_sink, &ok).await;
                        }
                        Err(e) => {
                            send_one(&ws_sink, &ServerMessage::RoomError { message: e }).await;
                        }
                    },
                    ClientMessage::JoinRoom {
                        room_id: ref rid,
                        ref name,
                    } => match room_manager.join_room(rid, name).await {
                        Ok((pid, session_token, player_count, rx, rarc)) => {
                            // Send join confirmation to this player.
                            let (chips, is_host, allow_late_entry, game_started) = {
                                let room = rarc.lock().await;
                                let gs = room.game_state.lock().await;
                                let c = gs.players.get(&pid).map(|p| p.chips).unwrap_or(0);
                                (c, gs.host_id == pid, gs.allow_late_entry, gs.game_started)
                            };
                            let joined = ServerMessage::JoinedGame {
                                player_id: pid,
                                chips,
                                player_count,
                                session_token: session_token.clone(),
                                is_host,
                                allow_late_entry,
                            };
                            let blind_config = {
                                let room = rarc.lock().await;
                                room.blind_config
                            };
                            send_one(
                                &ws_sink,
                                &ServerMessage::RoomJoined {
                                    room_id: rid.clone(),
                                    blind_config,
                                },
                            )
                            .await;
                            send_one(&ws_sink, &joined).await;

                            // Send the full player list so the newcomer sees existing participants.
                            {
                                let room = rarc.lock().await;
                                let gs = room.game_state.lock().await;
                                let players: Vec<poker_core::protocol::PlayerInfo> = gs
                                    .players
                                    .values()
                                    .map(|p| poker_core::protocol::PlayerInfo {
                                        id: p.id,
                                        name: p.name.clone(),
                                        chips: p.chips,
                                    })
                                    .collect();
                                send_one(&ws_sink, &ServerMessage::PlayerList { players }).await;
                            }

                            // Late join: send full game state snapshot.
                            if game_started {
                                let room = rarc.lock().await;
                                let gs = room.game_state.lock().await;

                                // GameStarted so the client knows the game is running.
                                send_one(&ws_sink, &ServerMessage::GameStarted).await;

                                // Current hand info.
                                if gs.hand_number > 0 {
                                    let n = gs.player_order.len();
                                    let (dealer_id, sb_id, bb_id) = if n >= 2 {
                                        let d = gs.player_order[gs.dealer_index % n];
                                        let sb = gs.player_order[(gs.dealer_index + 1) % n];
                                        let bb = gs.player_order[(gs.dealer_index + 2) % n];
                                        (d, sb, bb)
                                    } else {
                                        (0, 0, 0)
                                    };
                                    send_one(
                                        &ws_sink,
                                        &ServerMessage::NewHand {
                                            hand_number: gs.hand_number,
                                            dealer_id,
                                            small_blind_id: sb_id,
                                            big_blind_id: bb_id,
                                            small_blind: gs.small_blind,
                                            big_blind: gs.big_blind,
                                        },
                                    )
                                    .await;
                                }

                                // Community cards.
                                if !gs.community_cards.is_empty() {
                                    let stage = match gs.phase {
                                        GamePhase::Flop => "flop",
                                        GamePhase::Turn => "turn",
                                        GamePhase::River => "river",
                                        _ => "flop",
                                    };
                                    let cards: Vec<poker_core::protocol::CardInfo> =
                                        gs.community_cards.iter().map(card_to_info).collect();
                                    send_one(
                                        &ws_sink,
                                        &ServerMessage::CommunityCards {
                                            stage: stage.to_string(),
                                            cards,
                                        },
                                    )
                                    .await;
                                }

                                send_one(&ws_sink, &ServerMessage::PotUpdate { pot: gs.pot }).await;

                                // Notify about sitting-out players.
                                for p in gs.players.values() {
                                    if p.sitting_out {
                                        send_one(
                                            &ws_sink,
                                            &ServerMessage::PlayerSatOut { player_id: p.id },
                                        )
                                        .await;
                                    }
                                }
                            }

                            room_id = Some(rid.clone());
                            player_id = Some(pid);
                            player_rx = Some(rx);
                            room_arc = Some(rarc);
                            break; // → enter the game loop
                        }
                        Err(e) => {
                            send_one(&ws_sink, &ServerMessage::RoomError { message: e }).await;
                        }
                    },
                    ClientMessage::Rejoin {
                        room_id: ref rid,
                        ref session_token,
                    } => match room_manager.rejoin_room(rid, session_token).await {
                        Ok((pid, rx, rarc)) => {
                            // Build and send a full state snapshot.
                            let snapshot = {
                                let room = rarc.lock().await;
                                let gs = room.game_state.lock().await;
                                room.build_rejoin_snapshot(&gs, rid, pid, session_token)
                            };
                            send_one(&ws_sink, &snapshot).await;

                            room_id = Some(rid.clone());
                            player_id = Some(pid);
                            player_rx = Some(rx);
                            room_arc = Some(rarc);
                            break; // → enter the game loop
                        }
                        Err(e) => {
                            send_one(&ws_sink, &ServerMessage::RoomError { message: e }).await;
                        }
                    },
                    ClientMessage::Ping => {
                        send_one(&ws_sink, &ServerMessage::Pong).await;
                    }
                    _ => {
                        send_one(
                            &ws_sink,
                            &ServerMessage::Error {
                                message: "Must create or join a room first".to_string(),
                            },
                        )
                        .await;
                    }
                }
            }
            Some(Ok(Message::Close(_))) | None => return,
            _ => continue,
        }
    }

    // ── Game loop ────────────────────────────────────────────────────────
    let rid = room_id.unwrap();
    let pid = player_id.unwrap();
    let mut rx = player_rx.unwrap();
    let rarc = room_arc.unwrap();

    // Spawn a write task that drains the player's mpsc receiver and forwards
    // messages as WebSocket text frames.
    let write_sink = Arc::clone(&ws_sink);
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(_) => continue,
            };
            let mut sink = write_sink.lock().await;
            if sink.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Read loop: deserialize ClientMessage, process, route responses.
    loop {
        match ws_stream.next().await {
            Some(Ok(Message::Text(text))) => {
                let msg: ClientMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        send_one(
                            &ws_sink,
                            &ServerMessage::Error {
                                message: format!("Invalid message: {e}"),
                            },
                        )
                        .await;
                        continue;
                    }
                };

                process_client_message(&msg, pid, &rarc).await;
            }
            Some(Ok(Message::Close(_))) | None => break,
            _ => continue,
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────────────
    write_handle.abort();
    room_manager.disconnect_player(&rid, pid).await;
    tracing::info!(room = %rid, player = pid, "Player disconnected");
}

// ─── Helpers ─────────────────────────────────────────────────────────────

/// Send a single `ServerMessage` directly on the raw WebSocket sink
/// (used during the lobby phase before the mpsc channel exists).
async fn send_one(
    sink: &Arc<Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
    msg: &ServerMessage,
) {
    if let Ok(json) = serde_json::to_string(msg) {
        let mut s = sink.lock().await;
        let _ = s.send(Message::Text(json.into())).await;
    }
}

// ─── Message processing ──────────────────────────────────────────────────

/// Process a single [`ClientMessage`] within an established room session.
async fn process_client_message(msg: &ClientMessage, player_id: u32, room_arc: &Arc<Mutex<Room>>) {
    match msg {
        // ── Join / room ops are no-ops once in a room ────────────────
        ClientMessage::Join { .. }
        | ClientMessage::CreateRoom { .. }
        | ClientMessage::JoinRoom { .. }
        | ClientMessage::Rejoin { .. } => {
            let room = room_arc.lock().await;
            room.send_to_player(
                player_id,
                &ServerMessage::Error {
                    message: "Already in a room".to_string(),
                },
            );
        }

        ClientMessage::Ping => {
            let room = room_arc.lock().await;
            room.send_to_player(player_id, &ServerMessage::Pong);
        }

        ClientMessage::GetPlayers => {
            let room = room_arc.lock().await;
            let gs = room.game_state.lock().await;
            let players = gs
                .players
                .values()
                .map(|p| poker_core::protocol::PlayerInfo {
                    id: p.id,
                    name: p.name.clone(),
                    chips: p.chips,
                })
                .collect();
            room.send_to_player(player_id, &ServerMessage::PlayerList { players });
        }

        ClientMessage::Chat { message } => {
            let room = room_arc.lock().await;
            let chat = ServerMessage::ChatMessage {
                player_id,
                message: message.clone(),
            };
            room.broadcast(&chat);
        }

        ClientMessage::StartGame => {
            let room = room_arc.lock().await;
            let mut gs = room.game_state.lock().await;

            if gs.game_started {
                room.send_to_player(
                    player_id,
                    &ServerMessage::Error {
                        message: "Game already started".to_string(),
                    },
                );
                return;
            }
            if gs.player_count() < 2 {
                room.send_to_player(
                    player_id,
                    &ServerMessage::Error {
                        message: "Need at least 2 players to start".to_string(),
                    },
                );
                return;
            }

            gs.game_started = true;

            // Freeze the starting chip amount for late entries.
            gs.starting_chips = gs.starting_bbs * gs.big_blind;

            // Initialise the blind increase timer if configured.
            if gs.blind_config.is_enabled() {
                gs.last_blind_increase = Some(std::time::Instant::now());
            }

            room.broadcast(&ServerMessage::GameStarted);

            // Start first hand.
            let hand_msgs = gs.start_new_hand();
            for m in &hand_msgs {
                room.broadcast(m);
            }

            // Send hole cards privately to each player.
            send_hole_cards(&gs, &room);

            // Notify the current player it's their turn and start the timer.
            notify_turn_and_start_timer(&gs, &room, room_arc);
        }

        // ── Betting actions ─────────────────────────────────────────
        ClientMessage::Fold => {
            process_action(player_id, PlayerAction::Fold, 0, room_arc).await;
        }
        ClientMessage::Check => {
            process_action(player_id, PlayerAction::Check, 0, room_arc).await;
        }
        ClientMessage::Call => {
            process_action(player_id, PlayerAction::Call, 0, room_arc).await;
        }
        ClientMessage::Raise { amount } => {
            process_action(player_id, PlayerAction::Raise, *amount, room_arc).await;
        }
        ClientMessage::AllIn => {
            process_action(player_id, PlayerAction::AllIn, 0, room_arc).await;
        }

        ClientMessage::SitOut => {
            let room = room_arc.lock().await;
            let mut gs = room.game_state.lock().await;
            if gs
                .players
                .get(&player_id)
                .map(|p| p.sitting_out)
                .unwrap_or(true)
            {
                return; // already sitting out or unknown player
            }
            gs.set_sitting_out(player_id);
            room.broadcast(&ServerMessage::PlayerSatOut { player_id });
        }

        ClientMessage::SitIn => {
            let room = room_arc.lock().await;
            let mut gs = room.game_state.lock().await;
            if !gs
                .players
                .get(&player_id)
                .map(|p| p.sitting_out)
                .unwrap_or(false)
            {
                return; // already sitting in or unknown player
            }
            gs.set_sitting_in(player_id);
            room.broadcast(&ServerMessage::PlayerSatIn { player_id });
        }

        ClientMessage::ToggleLateEntry => {
            let room = room_arc.lock().await;
            let mut gs = room.game_state.lock().await;
            if gs.host_id != player_id {
                room.send_to_player(
                    player_id,
                    &ServerMessage::Error {
                        message: "Only the host can toggle late entry".to_string(),
                    },
                );
                return;
            }
            gs.allow_late_entry = !gs.allow_late_entry;
            room.broadcast(&ServerMessage::LateEntryChanged {
                allowed: gs.allow_late_entry,
            });
        }
    }
}

/// Handle a betting action from a player.
///
/// This mirrors the logic in the legacy `server.rs` but routes messages
/// through per-player senders instead of a broadcast channel.
async fn process_action(
    player_id: u32,
    action: PlayerAction,
    amount: u32,
    room_arc: &Arc<Mutex<Room>>,
) {
    let room = room_arc.lock().await;
    let mut gs = room.game_state.lock().await;

    // ── Pre-checks ───────────────────────────────────────────────────
    if !gs.game_started {
        room.send_to_player(
            player_id,
            &ServerMessage::Error {
                message: "Game not started".to_string(),
            },
        );
        return;
    }

    if gs.current_player_id() != Some(player_id) {
        room.send_to_player(
            player_id,
            &ServerMessage::Error {
                message: "Not your turn".to_string(),
            },
        );
        return;
    }

    let valid = gs.valid_actions(player_id);
    if !valid.contains(&action) {
        room.send_to_player(
            player_id,
            &ServerMessage::Error {
                message: format!("Invalid action. Valid: {:?}", valid),
            },
        );
        return;
    }

    let player = match gs.players.get(&player_id) {
        Some(p) => p.clone(),
        None => {
            room.send_to_player(
                player_id,
                &ServerMessage::Error {
                    message: "Player not found".to_string(),
                },
            );
            return;
        }
    };

    let to_call = gs.current_bet.saturating_sub(player.current_bet);
    let mut action_amount: Option<u32> = None;

    // ── Apply the action ─────────────────────────────────────────────
    match action {
        PlayerAction::Fold => {
            if let Some(p) = gs.players.get_mut(&player_id) {
                p.status = PlayerStatus::Folded;
            }
        }
        PlayerAction::Check => {
            if to_call != 0 {
                room.send_to_player(
                    player_id,
                    &ServerMessage::Error {
                        message: "Cannot check, must call or raise".to_string(),
                    },
                );
                return;
            }
            if gs.phase == GamePhase::PreFlop && gs.big_blind_option {
                gs.big_blind_option = false;
                gs.last_raiser_index = None;
            }
        }
        PlayerAction::Call => {
            let call_amount = to_call.min(player.chips);
            {
                let p = gs.players.get_mut(&player_id).unwrap();
                p.chips -= call_amount;
                p.current_bet += call_amount;
                if p.chips == 0 {
                    p.status = PlayerStatus::AllIn;
                }
            }
            gs.pot += call_amount;
            action_amount = Some(call_amount);
        }
        PlayerAction::Raise => {
            let raise_total = to_call + amount;
            if raise_total > player.chips {
                room.send_to_player(
                    player_id,
                    &ServerMessage::Error {
                        message: format!(
                            "Not enough chips. Have {}, need {}",
                            player.chips, raise_total
                        ),
                    },
                );
                return;
            }
            if amount < gs.min_raise && raise_total < player.chips {
                room.send_to_player(
                    player_id,
                    &ServerMessage::Error {
                        message: format!("Minimum raise is {}", gs.min_raise),
                    },
                );
                return;
            }

            let new_bet;
            {
                let p = gs.players.get_mut(&player_id).unwrap();
                p.chips -= raise_total;
                p.current_bet += raise_total;
                new_bet = p.current_bet;
                if p.chips == 0 {
                    p.status = PlayerStatus::AllIn;
                }
            }
            gs.pot += raise_total;
            gs.current_bet = new_bet;
            gs.min_raise = gs.big_blind;
            gs.last_raiser_index = Some(gs.current_player_index);
            gs.big_blind_option = false;
            action_amount = Some(raise_total);
        }
        PlayerAction::AllIn => {
            let all_in = player.chips;
            let new_bet;
            {
                let p = gs.players.get_mut(&player_id).unwrap();
                p.chips = 0;
                p.current_bet += all_in;
                new_bet = p.current_bet;
                p.status = PlayerStatus::AllIn;
            }
            gs.pot += all_in;
            if new_bet > gs.current_bet {
                gs.current_bet = new_bet;
                gs.last_raiser_index = Some(gs.current_player_index);
            }
            action_amount = Some(all_in);
        }
    }

    // ── Broadcast the action + pot update ────────────────────────────
    room.broadcast(&ServerMessage::PlayerActed {
        player_id,
        action,
        amount: action_amount,
    });
    room.broadcast(&ServerMessage::PotUpdate { pot: gs.pot });

    gs.has_acted_this_round = true;
    gs.next_player();

    // ── Post-action: check hand / betting status ─────────────────────
    if gs.active_player_count() == 1 {
        let msgs = gs.resolve_hand();
        for m in &msgs {
            room.broadcast(m);
        }
        maybe_start_new_hand(&mut gs, &room, room_arc).await;
        return;
    }

    if gs.is_betting_complete() {
        if gs.phase == GamePhase::River {
            let msgs = gs.resolve_hand();
            for m in &msgs {
                room.broadcast(m);
            }
            maybe_start_new_hand(&mut gs, &room, room_arc).await;
        } else {
            // Advance to next phase.
            let phase_msgs = gs.advance_phase();
            for m in &phase_msgs {
                room.broadcast(m);
            }

            // If only all-in players remain, run it out.
            if gs.actionable_players().is_empty() {
                broadcast_allin_showdown(&gs, &room);

                // Release locks before the timed loop so we can
                // cleanly re-acquire them each iteration.
                drop(gs);
                drop(room);

                run_out_board(room_arc).await;
            } else {
                notify_turn_and_start_timer(&gs, &room, room_arc);
            }
        }
    } else {
        notify_turn_and_start_timer(&gs, &room, room_arc);
    }
}

/// If the game is still running with ≥ 2 players, start the next hand
/// after a short delay.
async fn maybe_start_new_hand(gs: &mut GameState, room: &Room, room_arc: &Arc<Mutex<Room>>) {
    if gs.game_started && gs.player_order.len() >= 2 {
        // Drop locks before sleeping would be ideal, but we hold mutable
        // borrows here. Since the delay is short (2 s) and actions are
        // serialised through the room lock anyway, this is acceptable.
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let hand_msgs = gs.start_new_hand();
        for m in &hand_msgs {
            room.broadcast(m);
        }
        send_hole_cards(gs, room);
        notify_turn_and_start_timer(gs, room, room_arc);
    }
}

/// Send each player their private hole cards.
fn send_hole_cards(gs: &GameState, room: &Room) {
    for (&pid, player) in &gs.players {
        if let Some((c1, c2)) = player.hole_cards {
            let cards = [card_to_info(&c1), card_to_info(&c2)];
            room.send_to_player(pid, &ServerMessage::HoleCards { cards });
        }
    }
}

/// Run out the remaining community cards when all players are all-in.
///
/// Locks are acquired and released each iteration so we can sleep between
/// cards without holding the room lock.
async fn run_out_board(room_arc: &Arc<Mutex<Room>>) {
    loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let room = room_arc.lock().await;
        let mut gs = room.game_state.lock().await;

        let phase_msgs = gs.advance_phase();
        for m in &phase_msgs {
            room.broadcast(m);
        }

        if gs.phase == GamePhase::Showdown {
            let msgs = gs.resolve_hand();
            for m in &msgs {
                room.broadcast(m);
            }
            maybe_start_new_hand(&mut gs, &room, room_arc).await;
            return;
        }
    }
}

/// Notify the player whose turn it is.
fn send_turn_notification(gs: &GameState, room: &Room) {
    if let Some(current_id) = gs.current_player_id() {
        let your_bet = gs
            .players
            .get(&current_id)
            .map(|p| p.current_bet)
            .unwrap_or(0);
        let valid_actions = gs.valid_actions(current_id);

        room.send_to_player(
            current_id,
            &ServerMessage::YourTurn {
                current_bet: gs.current_bet,
                your_bet,
                pot: gs.pot,
                min_raise: gs.min_raise,
                valid_actions,
            },
        );
    }
}

/// Send the turn notification **and** start a 30-second turn timer.
///
/// Increments the room's turn counter so any previously-spawned timer
/// becomes a no-op, then spawns a new background task that will force a
/// check-or-fold when the timeout elapses.
///
/// If the current player is sitting out, their action is resolved
/// immediately (auto-check or auto-fold) instead of waiting for input.
fn notify_turn_and_start_timer(gs: &GameState, room: &Room, room_arc: &Arc<Mutex<Room>>) {
    // Send the private YourTurn message to the current player.
    send_turn_notification(gs, room);

    let Some(current_id) = gs.current_player_id() else {
        return;
    };

    // Increment the turn counter to invalidate any stale timer tasks.
    let turn = room.turn_counter.fetch_add(1, Ordering::SeqCst) + 1;

    if gs.is_current_player_sitting_out() {
        // Sitting-out player: resolve immediately (no timer broadcast).
        let valid = gs.valid_actions(current_id);
        let action = if valid.contains(&PlayerAction::Check) {
            PlayerAction::Check
        } else {
            PlayerAction::Fold
        };
        let room_arc_clone = Arc::clone(room_arc);
        tokio::spawn(async move {
            // Small delay so the turn notification is delivered first.
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            tracing::info!(
                player = current_id,
                ?action,
                "Sitting-out player, auto-acting"
            );
            process_action(current_id, action, 0, &room_arc_clone).await;
        });
        return;
    }

    // Broadcast the timer start to all players so UIs can show a countdown.
    room.broadcast(&ServerMessage::TurnTimerStarted {
        player_id: current_id,
        timeout_secs: TURN_TIMEOUT_SECS,
    });

    // Spawn a background task that will force an action after the timeout.
    let counter = Arc::clone(&room.turn_counter);
    let room_arc_clone = Arc::clone(room_arc);
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(TURN_TIMEOUT_SECS as u64)).await;
        // Only act if the turn counter still matches (i.e. no one has acted
        // or started a new turn since we spawned).
        if counter.load(Ordering::SeqCst) == turn {
            force_timeout_action(room_arc_clone, turn, current_id).await;
        }
    });
}

/// Force a check-or-fold for a player whose turn timer has expired.
///
/// If the forced action is a fold (i.e. the player could not simply check),
/// the player is also automatically sat out.
async fn force_timeout_action(room_arc: Arc<Mutex<Room>>, expected_turn: u64, player_id: u32) {
    // Quick pre-check under the lock to confirm the turn is still valid.
    {
        let room = room_arc.lock().await;
        let gs = room.game_state.lock().await;

        if room.turn_counter.load(Ordering::SeqCst) != expected_turn {
            return;
        }
        if !gs.game_started {
            return;
        }
        if gs.current_player_id() != Some(player_id) {
            return;
        }
    }

    // Determine the forced action (check if valid, otherwise fold).
    let action = {
        let room = room_arc.lock().await;
        let gs = room.game_state.lock().await;
        let valid = gs.valid_actions(player_id);
        if valid.contains(&PlayerAction::Check) {
            PlayerAction::Check
        } else {
            PlayerAction::Fold
        }
    };

    // If forced to fold, automatically sit the player out.
    if action == PlayerAction::Fold {
        let room = room_arc.lock().await;
        let mut gs = room.game_state.lock().await;
        if !gs
            .players
            .get(&player_id)
            .map(|p| p.sitting_out)
            .unwrap_or(true)
        {
            gs.set_sitting_out(player_id);
            room.broadcast(&ServerMessage::PlayerSatOut { player_id });
            tracing::info!(player = player_id, "Auto sitting out after timeout fold");
        }
    }

    tracing::info!(
        player = player_id,
        ?action,
        "Turn timer expired, forcing action"
    );

    // Reuse the normal action processing pipeline.
    process_action(player_id, action, 0, &room_arc).await;
}

/// Broadcast an all-in showdown with equity percentages.
fn broadcast_allin_showdown(gs: &GameState, room: &Room) {
    let mut player_hands: Vec<(u32, [CardInfo; 2], Hand)> = Vec::new();

    for &id in &gs.player_order {
        if let Some(player) = gs.players.get(&id)
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

    let board = gs.build_board();
    let hands_for_calc: Vec<Hand> = player_hands
        .iter()
        .map(|(_, _, h)| Hand(h.0, h.1))
        .collect();
    let equities = calculate_equity_multi(&hands_for_calc, &board, 1000);

    let hands_with_equity: Vec<(u32, [CardInfo; 2], f64)> = player_hands
        .iter()
        .enumerate()
        .map(|(i, (id, cards, _))| (*id, *cards, equities.get(i).copied().unwrap_or(0.0)))
        .collect();

    let community_cards: Vec<CardInfo> = gs.community_cards.iter().map(card_to_info).collect();

    room.broadcast(&ServerMessage::AllInShowdown {
        hands: hands_with_equity,
        community_cards,
    });
}
