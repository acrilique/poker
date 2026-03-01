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

use crate::game_logic::{GamePhase, PlayerStatus, TURN_TIMEOUT_SECS};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use poker_core::poker::{Hand, calculate_equity_multi};
use poker_core::protocol::{CardInfo, ClientMessage, PlayerAction, ServerMessage, card_to_info};
use tokio::sync::Mutex;

use crate::room::{PlayerRx, Room, RoomManager, broadcast, send_to_player};

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
                            send_one(
                                &ws_sink,
                                &ServerMessage::RoomError {
                                    message: e.to_string(),
                                },
                            )
                            .await;
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
                                let c = room
                                    .game_state
                                    .players
                                    .get(&pid)
                                    .map(|p| p.chips)
                                    .unwrap_or(0);
                                (
                                    c,
                                    room.game_state.host_id == pid,
                                    room.game_state.allow_late_entry,
                                    room.game_state.game_started,
                                )
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
                                let players: Vec<poker_core::protocol::PlayerInfo> = room
                                    .game_state
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

                                // GameStarted so the client knows the game is running.
                                send_one(&ws_sink, &ServerMessage::GameStarted).await;

                                // Current hand info.
                                if room.game_state.hand_number > 0 {
                                    let n = room.game_state.player_order.len();
                                    let (dealer_id, sb_id, bb_id) = if n >= 2 {
                                        let d = room.game_state.player_order
                                            [room.game_state.dealer_index % n];
                                        let sb = room.game_state.player_order
                                            [(room.game_state.dealer_index + 1) % n];
                                        let bb = room.game_state.player_order
                                            [(room.game_state.dealer_index + 2) % n];
                                        (d, sb, bb)
                                    } else {
                                        (0, 0, 0)
                                    };
                                    send_one(
                                        &ws_sink,
                                        &ServerMessage::NewHand {
                                            hand_number: room.game_state.hand_number,
                                            dealer_id,
                                            small_blind_id: sb_id,
                                            big_blind_id: bb_id,
                                            small_blind: room.game_state.small_blind,
                                            big_blind: room.game_state.big_blind,
                                        },
                                    )
                                    .await;
                                }

                                // Community cards.
                                if !room.game_state.community_cards.is_empty() {
                                    let stage = match room.game_state.phase {
                                        GamePhase::Flop => "flop",
                                        GamePhase::Turn => "turn",
                                        GamePhase::River => "river",
                                        _ => "flop",
                                    };
                                    let cards: Vec<poker_core::protocol::CardInfo> = room
                                        .game_state
                                        .community_cards
                                        .iter()
                                        .map(card_to_info)
                                        .collect();
                                    send_one(
                                        &ws_sink,
                                        &ServerMessage::CommunityCards {
                                            stage: stage.to_string(),
                                            cards,
                                        },
                                    )
                                    .await;
                                }

                                send_one(
                                    &ws_sink,
                                    &ServerMessage::PotUpdate {
                                        pot: room.game_state.pot,
                                    },
                                )
                                .await;

                                // Notify about sitting-out players.
                                for p in room.game_state.players.values() {
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
                            send_one(
                                &ws_sink,
                                &ServerMessage::RoomError {
                                    message: e.to_string(),
                                },
                            )
                            .await;
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
                                room.build_rejoin_snapshot(rid, pid, session_token)
                            };
                            send_one(&ws_sink, &snapshot).await;

                            room_id = Some(rid.clone());
                            player_id = Some(pid);
                            player_rx = Some(rx);
                            room_arc = Some(rarc);
                            break; // → enter the game loop
                        }
                        Err(e) => {
                            send_one(
                                &ws_sink,
                                &ServerMessage::RoomError {
                                    message: e.to_string(),
                                },
                            )
                            .await;
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
        // If the channel is dropped (e.g. due to being full), close the WebSocket
        // to ensure the read loop also terminates and the player is fully disconnected.
        let mut sink = write_sink.lock().await;
        let _ = sink.close().await;
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
            let mut room = room_arc.lock().await;
            send_to_player(
                &mut room.player_senders,
                player_id,
                &ServerMessage::Error {
                    message: "Already in a room".to_string(),
                },
            );
        }

        ClientMessage::Ping => {
            let mut room = room_arc.lock().await;
            send_to_player(&mut room.player_senders, player_id, &ServerMessage::Pong);
        }

        ClientMessage::GetPlayers => {
            let mut room = room_arc.lock().await;
            let players = room
                .game_state
                .players
                .values()
                .map(|p| poker_core::protocol::PlayerInfo {
                    id: p.id,
                    name: p.name.clone(),
                    chips: p.chips,
                })
                .collect();
            send_to_player(
                &mut room.player_senders,
                player_id,
                &ServerMessage::PlayerList { players },
            );
        }

        ClientMessage::Chat { message } => {
            const MAX_CHAT_LEN: usize = 256;
            let truncated: String = message.chars().take(MAX_CHAT_LEN).collect();
            // Escape HTML-sensitive characters to prevent XSS when rendered.
            let message = truncated
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&#x27;");
            let mut room = room_arc.lock().await;
            let chat = ServerMessage::ChatMessage { player_id, message };
            broadcast(&mut room.player_senders, &chat);
        }

        ClientMessage::StartGame => {
            let mut room = room_arc.lock().await;

            if room.game_state.game_started {
                send_to_player(
                    &mut room.player_senders,
                    player_id,
                    &ServerMessage::Error {
                        message: "Game already started".to_string(),
                    },
                );
                return;
            }
            if room.game_state.player_count() < 2 {
                send_to_player(
                    &mut room.player_senders,
                    player_id,
                    &ServerMessage::Error {
                        message: "Need at least 2 players to start".to_string(),
                    },
                );
                return;
            }

            room.game_state.game_started = true;

            // Freeze the starting chip amount and big blind for late entries.
            room.game_state.starting_big_blind = room.game_state.big_blind;
            room.game_state.starting_chips =
                room.game_state.starting_bbs * room.game_state.big_blind;

            // Initialise the blind increase timer if configured.
            if room.game_state.blind_config.is_enabled() {
                room.game_state.last_blind_increase = Some(std::time::Instant::now());
            }

            broadcast(&mut room.player_senders, &ServerMessage::GameStarted);

            // Start first hand.
            let hand_msgs = room.game_state.start_new_hand();
            for m in &hand_msgs {
                broadcast(&mut room.player_senders, m);
            }

            // Send hole cards privately to each player.
            send_hole_cards(&mut room);

            // Notify the current player it's their turn and start the timer.
            let sitting_out = notify_turn_and_start_timer(&mut room, room_arc);
            drop(room);
            if let Some((pid, act)) = sitting_out {
                process_action(pid, act, 0, room_arc).await;
            }
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
            let mut room = room_arc.lock().await;
            if room
                .game_state
                .players
                .get(&player_id)
                .map(|p| p.sitting_out)
                .unwrap_or(true)
            {
                return; // already sitting out or unknown player
            }
            room.game_state.set_sitting_out(player_id);
            broadcast(
                &mut room.player_senders,
                &ServerMessage::PlayerSatOut { player_id },
            );
        }

        ClientMessage::SitIn => {
            let mut room = room_arc.lock().await;
            if !room
                .game_state
                .players
                .get(&player_id)
                .map(|p| p.sitting_out)
                .unwrap_or(false)
            {
                return; // already sitting in or unknown player
            }
            room.game_state.set_sitting_in(player_id);
            broadcast(
                &mut room.player_senders,
                &ServerMessage::PlayerSatIn { player_id },
            );

            // If the game was paused waiting for players, check whether
            // we now have enough active players to start a new hand.
            // Toggle `waiting_for_players` to false *before* releasing
            // the lock so that a concurrent SitIn handler won't also
            // trigger a new hand.
            if room.game_state.waiting_for_players {
                let active_count = room
                    .game_state
                    .player_order
                    .iter()
                    .filter(|id| {
                        room.game_state
                            .players
                            .get(id)
                            .map(|p| !p.sitting_out && p.chips > 0)
                            .unwrap_or(false)
                    })
                    .count();
                if active_count >= 2 {
                    // Claim the transition: no other task will enter this
                    // branch until `waiting_for_players` is set back to true.
                    room.game_state.waiting_for_players = false;
                    drop(room);
                    let sitting_out = maybe_start_new_hand(room_arc).await;
                    if let Some((pid, act)) = sitting_out {
                        process_action(pid, act, 0, room_arc).await;
                    }
                }
            }
        }

        ClientMessage::ToggleLateEntry => {
            let mut room = room_arc.lock().await;
            if room.game_state.host_id != player_id {
                send_to_player(
                    &mut room.player_senders,
                    player_id,
                    &ServerMessage::Error {
                        message: "Only the host can toggle late entry".to_string(),
                    },
                );
                return;
            }
            room.game_state.allow_late_entry = !room.game_state.allow_late_entry;
            let allowed = room.game_state.allow_late_entry;
            broadcast(
                &mut room.player_senders,
                &ServerMessage::LateEntryChanged { allowed },
            );
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
    let mut room = room_arc.lock().await;

    // ── Pre-checks ───────────────────────────────────────────────────
    if !room.game_state.game_started {
        send_to_player(
            &mut room.player_senders,
            player_id,
            &ServerMessage::Error {
                message: "Game not started".to_string(),
            },
        );
        return;
    }

    if room.game_state.current_player_id() != Some(player_id) {
        send_to_player(
            &mut room.player_senders,
            player_id,
            &ServerMessage::Error {
                message: "Not your turn".to_string(),
            },
        );
        return;
    }

    let valid = room.game_state.valid_actions(player_id);
    if !valid.contains(&action) {
        send_to_player(
            &mut room.player_senders,
            player_id,
            &ServerMessage::Error {
                message: format!("Invalid action. Valid: {:?}", valid),
            },
        );
        return;
    }

    let player = match room.game_state.players.get(&player_id) {
        Some(p) => p.clone(),
        None => {
            send_to_player(
                &mut room.player_senders,
                player_id,
                &ServerMessage::Error {
                    message: "Player not found".to_string(),
                },
            );
            return;
        }
    };

    let to_call = room
        .game_state
        .current_bet
        .saturating_sub(player.current_bet);
    let mut action_amount: Option<u32> = None;

    // ── Apply the action ─────────────────────────────────────────────
    match action {
        PlayerAction::Fold => {
            if let Some(p) = room.game_state.players.get_mut(&player_id) {
                p.status = PlayerStatus::Folded;
            }
        }
        PlayerAction::Check => {
            if to_call != 0 {
                send_to_player(
                    &mut room.player_senders,
                    player_id,
                    &ServerMessage::Error {
                        message: "Cannot check, must call or raise".to_string(),
                    },
                );
                return;
            }
            if room.game_state.phase == GamePhase::PreFlop && room.game_state.big_blind_option {
                room.game_state.big_blind_option = false;
                room.game_state.last_raiser_index = None;
            }
        }
        PlayerAction::Call => {
            let call_amount = to_call.min(player.chips);
            {
                let p = room.game_state.players.get_mut(&player_id).unwrap();
                p.chips -= call_amount;
                p.current_bet += call_amount;
                if p.chips == 0 {
                    p.status = PlayerStatus::AllIn;
                }
            }
            room.game_state.pot += call_amount;
            *room
                .game_state
                .pot_contributions
                .entry(player_id)
                .or_insert(0) += call_amount;
            action_amount = Some(call_amount);
        }
        PlayerAction::Raise => {
            let raise_total = to_call.saturating_add(amount);
            if raise_total > player.chips {
                send_to_player(
                    &mut room.player_senders,
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
            let min_raise = room.game_state.min_raise;
            if amount < min_raise && raise_total < player.chips {
                send_to_player(
                    &mut room.player_senders,
                    player_id,
                    &ServerMessage::Error {
                        message: format!("Minimum raise is {}", min_raise),
                    },
                );
                return;
            }

            let new_bet;
            {
                let p = room.game_state.players.get_mut(&player_id).unwrap();
                p.chips -= raise_total;
                p.current_bet += raise_total;
                new_bet = p.current_bet;
                if p.chips == 0 {
                    p.status = PlayerStatus::AllIn;
                }
            }
            room.game_state.pot += raise_total;
            *room
                .game_state
                .pot_contributions
                .entry(player_id)
                .or_insert(0) += raise_total;
            room.game_state.current_bet = new_bet;
            room.game_state.min_raise = room.game_state.big_blind;
            room.game_state.last_raiser_index = Some(room.game_state.current_player_index);
            room.game_state.big_blind_option = false;
            action_amount = Some(raise_total);
        }
        PlayerAction::AllIn => {
            let all_in = player.chips;
            let new_bet;
            {
                let p = room.game_state.players.get_mut(&player_id).unwrap();
                p.chips = 0;
                p.current_bet += all_in;
                new_bet = p.current_bet;
                p.status = PlayerStatus::AllIn;
            }
            room.game_state.pot += all_in;
            *room
                .game_state
                .pot_contributions
                .entry(player_id)
                .or_insert(0) += all_in;
            if new_bet > room.game_state.current_bet {
                // Only reopen betting (set last_raiser_index) if the all-in
                // constitutes a full legal raise.  A "short all-in" (less than
                // the minimum raise above the current bet) does NOT give other
                // players a new opportunity to re-raise.
                let raise_increment = new_bet - room.game_state.current_bet;
                if raise_increment >= room.game_state.min_raise {
                    room.game_state.last_raiser_index = Some(room.game_state.current_player_index);
                }
                room.game_state.current_bet = new_bet;
            }
            action_amount = Some(all_in);
        }
    }

    // ── Broadcast the action + pot update ────────────────────────────
    broadcast(
        &mut room.player_senders,
        &ServerMessage::PlayerActed {
            player_id,
            action,
            amount: action_amount,
        },
    );
    let pot = room.game_state.pot;
    broadcast(&mut room.player_senders, &ServerMessage::PotUpdate { pot });

    room.game_state.has_acted_this_round = true;
    room.game_state.next_player();

    // ── Post-action: check hand / betting status ─────────────────────
    // Loop to process any sitting-out players synchronously rather than
    // spawning delayed tasks (which are susceptible to race conditions).
    loop {
        if room.game_state.active_player_count() == 1 {
            let msgs = room.game_state.resolve_hand();
            for m in &msgs {
                broadcast(&mut room.player_senders, m);
            }
            drop(room);
            if let Some((pid, act)) = maybe_start_new_hand(room_arc).await {
                room = room_arc.lock().await;
                apply_sitting_out_action(&mut room, pid, act);
                continue;
            }
            return;
        }

        if room.game_state.is_betting_complete() {
            if room.game_state.phase == GamePhase::River {
                let msgs = room.game_state.resolve_hand();
                for m in &msgs {
                    broadcast(&mut room.player_senders, m);
                }
                drop(room);
                if let Some((pid, act)) = maybe_start_new_hand(room_arc).await {
                    room = room_arc.lock().await;
                    apply_sitting_out_action(&mut room, pid, act);
                    continue;
                }
                return;
            } else {
                // Advance to next phase.
                let phase_msgs = room.game_state.advance_phase();
                for m in &phase_msgs {
                    broadcast(&mut room.player_senders, m);
                }

                // If only all-in players remain, run it out.
                if room.game_state.actionable_players().is_empty() {
                    // Release lock before the blocking equity
                    // calculation so we don't starve other tasks.
                    drop(room);

                    broadcast_allin_showdown(room_arc).await;
                    run_out_board(room_arc).await;
                    return;
                }

                if let Some((pid, act)) = notify_turn_and_start_timer(&mut room, room_arc) {
                    apply_sitting_out_action(&mut room, pid, act);
                    continue;
                }
                return;
            }
        }

        if let Some((pid, act)) = notify_turn_and_start_timer(&mut room, room_arc) {
            apply_sitting_out_action(&mut room, pid, act);
            continue;
        }
        return;
    }
}

/// If the game is still running with ≥ 2 active (not sitting-out) players,
/// start the next hand after a short delay. Otherwise pause and wait for
/// players to sit back in.
///
/// Returns `Some((player_id, action))` if the first player of the new hand
/// is sitting out, so the caller can process their auto-action synchronously.
async fn maybe_start_new_hand(room_arc: &Arc<Mutex<Room>>) -> Option<(u32, PlayerAction)> {
    // check conditions under a brief lock.
    let should_start = {
        let mut room = room_arc.lock().await;
        if !room.game_state.game_started {
            return None;
        }

        let active_count = room
            .game_state
            .player_order
            .iter()
            .filter(|id| {
                room.game_state
                    .players
                    .get(id)
                    .map(|p| !p.sitting_out && p.chips > 0)
                    .unwrap_or(false)
            })
            .count();

        if active_count >= 2 {
            room.game_state.waiting_for_players = false;
            true
        } else {
            room.game_state.waiting_for_players = true;
            broadcast(&mut room.player_senders, &ServerMessage::WaitingForPlayers);
            false
        }
    }; // lock released

    if !should_start {
        return None;
    }

    // delay *without* holding the room lock so that chat,
    // sit-out, ping and other messages can still be processed.
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    // Re-acquire the lock and re-check conditions. Players may have
    // disconnected, sat out, or been eliminated during the sleep.
    let mut room = room_arc.lock().await;

    if !room.game_state.game_started {
        return None;
    }

    let active_count = room
        .game_state
        .player_order
        .iter()
        .filter(|id| {
            room.game_state
                .players
                .get(id)
                .map(|p| !p.sitting_out && p.chips > 0)
                .unwrap_or(false)
        })
        .count();

    if active_count < 2 {
        room.game_state.waiting_for_players = true;
        broadcast(&mut room.player_senders, &ServerMessage::WaitingForPlayers);
        return None;
    }

    let hand_msgs = room.game_state.start_new_hand();
    for m in &hand_msgs {
        broadcast(&mut room.player_senders, m);
    }
    send_hole_cards(&mut room);
    notify_turn_and_start_timer(&mut room, room_arc)
}

/// Send each player their private hole cards.
fn send_hole_cards(room: &mut Room) {
    for (&pid, player) in &room.game_state.players {
        if let Some((c1, c2)) = player.hole_cards {
            let cards = [card_to_info(&c1), card_to_info(&c2)];
            send_to_player(
                &mut room.player_senders,
                pid,
                &ServerMessage::HoleCards { cards },
            );
        }
    }
}

/// Run out the remaining community cards when all players are all-in.
///
/// Locks are acquired and released each iteration so we can sleep between
/// cards without holding the room lock.
async fn run_out_board(room_arc: &Arc<Mutex<Room>>) {
    'run_out: loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

        let mut room = room_arc.lock().await;

        let phase_msgs = room.game_state.advance_phase();
        for m in &phase_msgs {
            broadcast(&mut room.player_senders, m);
        }

        if room.game_state.phase == GamePhase::Showdown {
            let msgs = room.game_state.resolve_hand();
            for m in &msgs {
                broadcast(&mut room.player_senders, m);
            }
            // Handle the new hand inline (with a loop for sitting-out
            // players) to avoid process_action <-> run_out_board recursion.
            drop(room);
            if let Some((mut pid, mut act)) = maybe_start_new_hand(room_arc).await {
                let mut room = room_arc.lock().await;
                loop {
                    apply_sitting_out_action(&mut room, pid, act);

                    if room.game_state.active_player_count() == 1 {
                        let hand_msgs = room.game_state.resolve_hand();
                        for m in &hand_msgs {
                            broadcast(&mut room.player_senders, m);
                        }
                        drop(room);
                        if let Some((np, na)) = maybe_start_new_hand(room_arc).await {
                            room = room_arc.lock().await;
                            pid = np;
                            act = na;
                            continue;
                        }
                        break;
                    }

                    if room.game_state.is_betting_complete() {
                        if room.game_state.phase == GamePhase::River {
                            let hand_msgs = room.game_state.resolve_hand();
                            for m in &hand_msgs {
                                broadcast(&mut room.player_senders, m);
                            }
                            drop(room);
                            if let Some((np, na)) = maybe_start_new_hand(room_arc).await {
                                room = room_arc.lock().await;
                                pid = np;
                                act = na;
                                continue;
                            }
                            break;
                        } else {
                            let phase_msgs = room.game_state.advance_phase();
                            for m in &phase_msgs {
                                broadcast(&mut room.player_senders, m);
                            }
                            if room.game_state.actionable_players().is_empty() {
                                drop(room);
                                broadcast_allin_showdown(room_arc).await;
                                // Re-enter the outer loop iteratively instead of
                                // recursing, which would grow the task stack.
                                continue 'run_out;
                            }
                            if let Some((np, na)) = notify_turn_and_start_timer(&mut room, room_arc)
                            {
                                pid = np;
                                act = na;
                                continue;
                            }
                            break;
                        }
                    }

                    if let Some((np, na)) = notify_turn_and_start_timer(&mut room, room_arc) {
                        pid = np;
                        act = na;
                        continue;
                    }
                    break;
                }
            }
            return;
        }
    }
}

/// Notify the player whose turn it is.
fn send_turn_notification(room: &mut Room) {
    if let Some(current_id) = room.game_state.current_player_id() {
        let your_bet = room
            .game_state
            .players
            .get(&current_id)
            .map(|p| p.current_bet)
            .unwrap_or(0);
        let valid_actions = room.game_state.valid_actions(current_id);

        let msg = ServerMessage::YourTurn {
            current_bet: room.game_state.current_bet,
            your_bet,
            pot: room.game_state.pot,
            min_raise: room.game_state.min_raise,
            valid_actions,
        };
        send_to_player(&mut room.player_senders, current_id, &msg);
    }
}

/// Apply a sitting-out player's auto-action (Check or Fold) inline.
///
/// This is used by the post-action loop to process consecutive sitting-out
/// players synchronously without dropping and re-acquiring locks.
fn apply_sitting_out_action(room: &mut Room, player_id: u32, action: PlayerAction) {
    match action {
        PlayerAction::Fold => {
            if let Some(p) = room.game_state.players.get_mut(&player_id) {
                p.status = PlayerStatus::Folded;
            }
        }
        PlayerAction::Check => {
            if room.game_state.phase == GamePhase::PreFlop && room.game_state.big_blind_option {
                room.game_state.big_blind_option = false;
                room.game_state.last_raiser_index = None;
            }
        }
        _ => {
            tracing::error!(?action, "Unexpected sitting-out auto-action");
            return;
        }
    }

    broadcast(
        &mut room.player_senders,
        &ServerMessage::PlayerActed {
            player_id,
            action,
            amount: None,
        },
    );
    let pot = room.game_state.pot;
    broadcast(&mut room.player_senders, &ServerMessage::PotUpdate { pot });

    room.game_state.has_acted_this_round = true;
    room.game_state.next_player();
}

/// Send the turn notification **and** start a 30-second turn timer.
///
/// Increments the room's turn counter so any previously-spawned timer
/// becomes a no-op, then spawns a new background task that will force a
/// check-or-fold when the timeout elapses.
///
/// If the current player is sitting out, their action is returned
/// synchronously as `Some((player_id, action))` so the caller can
/// process it immediately without spawning a delayed task.
fn notify_turn_and_start_timer(
    room: &mut Room,
    room_arc: &Arc<Mutex<Room>>,
) -> Option<(u32, PlayerAction)> {
    // Send the private YourTurn message to the current player.
    send_turn_notification(room);

    let current_id = room.game_state.current_player_id()?;

    // Increment the turn counter to invalidate any stale timer tasks.
    let turn = room.turn_counter.fetch_add(1, Ordering::SeqCst) + 1;

    if room.game_state.is_current_player_sitting_out() {
        // Sitting-out player: return the auto-action for the caller to
        // process synchronously, avoiding a spawned task race condition.
        let valid = room.game_state.valid_actions(current_id);
        let action = if valid.contains(&PlayerAction::Check) {
            PlayerAction::Check
        } else {
            PlayerAction::Fold
        };
        tracing::info!(
            player = current_id,
            ?action,
            "Sitting-out player, auto-acting"
        );
        return Some((current_id, action));
    }

    // Broadcast the timer start to all players so UIs can show a countdown.
    broadcast(
        &mut room.player_senders,
        &ServerMessage::TurnTimerStarted {
            player_id: current_id,
            timeout_secs: TURN_TIMEOUT_SECS,
        },
    );

    // Spawn a background task that will force an action after the timeout.
    let counter = Arc::clone(&room.turn_counter);
    let room_arc_clone = Arc::clone(room_arc);
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(TURN_TIMEOUT_SECS.into())).await;
        // Only act if the turn counter still matches (i.e. no one has acted
        // or started a new turn since we spawned).
        if counter.load(Ordering::SeqCst) == turn {
            force_timeout_action(room_arc_clone, turn, current_id).await;
        }
    });

    None
}

/// Force a check-or-fold for a player whose turn timer has expired.
///
/// If the forced action is a fold (i.e. the player could not simply check),
/// the player is also automatically sat out.
///
/// All validity checks, action determination, and sit-out logic are performed
/// under a single lock acquisition to prevent a real player action from
/// slipping in between (which would cause a spurious "Not your turn" error).
async fn force_timeout_action(room_arc: Arc<Mutex<Room>>, expected_turn: u64, player_id: u32) {
    // Perform everything under one lock to close the gap between
    // validity check and process_action.
    let action = {
        let mut room = room_arc.lock().await;

        if room.turn_counter.load(Ordering::SeqCst) != expected_turn {
            return;
        }
        if !room.game_state.game_started {
            return;
        }
        if room.game_state.current_player_id() != Some(player_id) {
            return;
        }

        // Determine the forced action (check if valid, otherwise fold).
        let valid = room.game_state.valid_actions(player_id);
        let act = if valid.contains(&PlayerAction::Check) {
            PlayerAction::Check
        } else {
            PlayerAction::Fold
        };

        // If forced to fold, automatically sit the player out.
        if act == PlayerAction::Fold
            && !room
                .game_state
                .players
                .get(&player_id)
                .map(|p| p.sitting_out)
                .unwrap_or(true)
        {
            room.game_state.set_sitting_out(player_id);
            broadcast(
                &mut room.player_senders,
                &ServerMessage::PlayerSatOut { player_id },
            );
            tracing::info!(player = player_id, "Auto sitting out after timeout fold");
        }

        act
    }; // lock released

    tracing::info!(
        player = player_id,
        ?action,
        "Turn timer expired, forcing action"
    );

    // Reuse the normal action processing pipeline.
    process_action(player_id, action, 0, &room_arc).await;
}

/// Broadcast an all-in showdown with equity percentages.
///
/// The equity Monte Carlo simulation is CPU-bound, so it is offloaded to
/// a blocking thread via [`tokio::task::spawn_blocking`] to avoid starving
/// the async runtime.
async fn broadcast_allin_showdown(room_arc: &Arc<Mutex<Room>>) {
    // --- 1. Extract data while holding the lock (cheap) ----------------
    let (player_hands, hands_for_calc, board, community_cards) = {
        let room = room_arc.lock().await;
        let mut player_hands: Vec<(u32, [CardInfo; 2], Hand)> = Vec::new();

        for &id in &room.game_state.player_order {
            if let Some(player) = room.game_state.players.get(&id)
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

        let board = room.game_state.build_board();
        let hands_for_calc: Vec<Hand> = player_hands
            .iter()
            .map(|(_, _, h)| Hand(h.0, h.1))
            .collect();
        let community_cards: Vec<CardInfo> = room
            .game_state
            .community_cards
            .iter()
            .map(card_to_info)
            .collect();

        (player_hands, hands_for_calc, board, community_cards)
    }; // lock released

    // --- 2. Run the CPU-heavy equity simulation off the async runtime --
    let equities =
        tokio::task::spawn_blocking(move || calculate_equity_multi(&hands_for_calc, &board, 1000))
            .await
            .expect("equity calculation task panicked");

    // --- 3. Re-acquire the lock and broadcast the result ---------------
    let hands_with_equity: Vec<(u32, [CardInfo; 2], f64)> = player_hands
        .iter()
        .enumerate()
        .map(|(i, (id, cards, _))| (*id, *cards, equities.get(i).copied().unwrap_or(0.0)))
        .collect();

    let mut room = room_arc.lock().await;
    broadcast(
        &mut room.player_senders,
        &ServerMessage::AllInShowdown {
            hands: hands_with_equity,
            community_cards,
        },
    );
}
