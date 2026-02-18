//! TCP-based poker server.
//!
//! This module handles connection management, message routing, and broadcast
//! logic.  The actual game rules live in [`crate::game_logic`].

use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast};

use crate::game_logic::{GamePhase, GameState, PlayerStatus, card_to_info};
use crate::poker::{Hand, calculate_equity_multi};
use crate::protocol::{CardInfo, ClientMessage, PlayerAction, ServerMessage};

/// Errors that can occur during server operations.
#[derive(Debug, Error)]
pub enum ServerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Poker server that handles TCP connections.
pub struct PokerServer {
    address: String,
    state: Arc<Mutex<GameState>>,
    broadcast_tx: broadcast::Sender<String>,
}

impl PokerServer {
    pub fn new(address: &str) -> Self {
        let (broadcast_tx, _) = broadcast::channel(100);
        Self {
            address: address.to_string(),
            state: Arc::new(Mutex::new(GameState::new())),
            broadcast_tx,
        }
    }

    /// Start the server and listen for connections
    pub async fn run(&self) -> Result<(), ServerError> {
        let listener = TcpListener::bind(&self.address).await?;
        println!("Poker server listening on {}", self.address);

        loop {
            let (socket, addr) = listener.accept().await?;
            println!("New connection from {}", addr);

            let state = Arc::clone(&self.state);
            let broadcast_tx = self.broadcast_tx.clone();
            let broadcast_rx = self.broadcast_tx.subscribe();

            tokio::spawn(async move {
                if let Err(e) = handle_connection(socket, state, broadcast_tx, broadcast_rx).await {
                    eprintln!("Connection error from {}: {}", addr, e);
                }
            });
        }
    }
}

/// Handle a single client connection
async fn handle_connection(
    socket: TcpStream,
    state: Arc<Mutex<GameState>>,
    broadcast_tx: broadcast::Sender<String>,
    mut broadcast_rx: broadcast::Receiver<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (reader, writer) = socket.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    // Player ID will be assigned on join
    let mut player_id: Option<u32> = None;

    // Spawn task to forward broadcasts to this client
    let writer_clone = Arc::new(Mutex::new(writer));
    let writer_for_broadcast = Arc::clone(&writer_clone);

    let broadcast_handle = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            let mut w = writer_for_broadcast.lock().await;
            if w.write_all(msg.as_bytes()).await.is_err() {
                break;
            }
            if w.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    // Main message loop
    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;

        if bytes_read == 0 {
            // Connection closed
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Parse client message
        match serde_json::from_str::<ClientMessage>(trimmed) {
            Ok(msg) => {
                let response = process_message(msg, &state, &broadcast_tx, &mut player_id).await;
                let mut w = writer_clone.lock().await;
                send_message(&mut *w, &response).await?;
            }
            Err(e) => {
                let error_msg = ServerMessage::Error {
                    message: format!("Invalid message format: {}", e),
                };
                let mut w = writer_clone.lock().await;
                send_message(&mut *w, &error_msg).await?;
            }
        }
    }

    // Cleanup: remove player from game state
    if let Some(id) = player_id {
        let mut state = state.lock().await;
        state.remove_player(id);

        // Broadcast player left
        let msg = ServerMessage::PlayerLeft { player_id: id };
        let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
    }

    broadcast_handle.abort();
    Ok(())
}

/// Process a client message and return a response
async fn process_message(
    msg: ClientMessage,
    state: &Arc<Mutex<GameState>>,
    broadcast_tx: &broadcast::Sender<String>,
    player_id: &mut Option<u32>,
) -> ServerMessage {
    match msg {
        ClientMessage::Join { name } => {
            if player_id.is_some() {
                return ServerMessage::Error {
                    message: "Already joined".to_string(),
                };
            }

            let mut state = state.lock().await;
            if state.game_started {
                return ServerMessage::Error {
                    message: "Game already in progress".to_string(),
                };
            }

            let player = state.add_player(name.clone());
            *player_id = Some(player.id);

            // Broadcast new player to all
            let broadcast_msg = ServerMessage::PlayerJoined {
                player_id: player.id,
                name: player.name.clone(),
            };
            let _ = broadcast_tx.send(serde_json::to_string(&broadcast_msg).unwrap());

            ServerMessage::JoinedGame {
                player_id: player.id,
                chips: player.chips,
                player_count: state.player_count(),
            }
        }

        ClientMessage::GetPlayers => {
            let state = state.lock().await;
            let players: Vec<crate::protocol::PlayerInfo> = state
                .players
                .values()
                .map(|p| crate::protocol::PlayerInfo {
                    id: p.id,
                    name: p.name.clone(),
                    chips: p.chips,
                })
                .collect();
            ServerMessage::PlayerList { players }
        }

        ClientMessage::Chat { message } => {
            if let Some(id) = player_id {
                let broadcast_msg = ServerMessage::ChatMessage {
                    player_id: *id,
                    message,
                };
                let _ = broadcast_tx.send(serde_json::to_string(&broadcast_msg).unwrap());
                ServerMessage::Ok
            } else {
                ServerMessage::Error {
                    message: "Must join first".to_string(),
                }
            }
        }

        ClientMessage::StartGame => {
            let mut state = state.lock().await;
            if state.game_started {
                return ServerMessage::Error {
                    message: "Game already started".to_string(),
                };
            }
            if state.player_count() < 2 {
                return ServerMessage::Error {
                    message: "Need at least 2 players to start".to_string(),
                };
            }

            state.game_started = true;

            // Broadcast game start
            let broadcast_msg = ServerMessage::GameStarted;
            let _ = broadcast_tx.send(serde_json::to_string(&broadcast_msg).unwrap());

            // Start first hand
            let hand_messages = state.start_new_hand();
            for msg in hand_messages {
                let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
            }

            // Send hole cards to each player (private)
            // Note: In this simplified version, we broadcast - in production,
            // you'd send to specific players only
            for (&pid, player) in &state.players {
                if let Some((c1, c2)) = player.hole_cards {
                    let cards = [card_to_info(&c1), card_to_info(&c2)];
                    let hole_msg = ServerMessage::HoleCards { cards };
                    // For now we'll send all cards - client should filter
                    let _ = broadcast_tx.send(format!(
                        "PRIVATE:{}:{}",
                        pid,
                        serde_json::to_string(&hole_msg).unwrap()
                    ));
                }
            }

            // Notify current player it's their turn
            if let Some(current_id) = state.current_player_id() {
                let current_player = state.players.get(&current_id);
                let your_bet = current_player.map(|p| p.current_bet).unwrap_or(0);
                let valid_actions = state.valid_actions(current_id);

                let turn_msg = ServerMessage::YourTurn {
                    current_bet: state.current_bet,
                    your_bet,
                    pot: state.pot,
                    min_raise: state.min_raise,
                    valid_actions,
                };
                let _ = broadcast_tx.send(format!(
                    "PRIVATE:{}:{}",
                    current_id,
                    serde_json::to_string(&turn_msg).unwrap()
                ));
            }

            ServerMessage::Ok
        }

        ClientMessage::Fold => {
            process_betting_action(state, broadcast_tx, player_id, PlayerAction::Fold, 0).await
        }

        ClientMessage::Check => {
            process_betting_action(state, broadcast_tx, player_id, PlayerAction::Check, 0).await
        }

        ClientMessage::Call => {
            process_betting_action(state, broadcast_tx, player_id, PlayerAction::Call, 0).await
        }

        ClientMessage::Raise { amount } => {
            process_betting_action(state, broadcast_tx, player_id, PlayerAction::Raise, amount)
                .await
        }

        ClientMessage::AllIn => {
            process_betting_action(state, broadcast_tx, player_id, PlayerAction::AllIn, 0).await
        }

        ClientMessage::Ping => ServerMessage::Pong,

        // Room management is handled by the new Axum server (poker-server).
        // The legacy TCP server does not support rooms.
        ClientMessage::CreateRoom { .. }
        | ClientMessage::JoinRoom { .. }
        | ClientMessage::Rejoin { .. } => ServerMessage::Error {
            message: "Room management is not supported by the legacy TCP server".to_string(),
        },

        // Sit-out is not supported by the legacy TCP server.
        ClientMessage::SitOut | ClientMessage::SitIn => ServerMessage::Error {
            message: "Sit-out is not supported by the legacy TCP server".to_string(),
        },
    }
}

/// Process a betting action
async fn process_betting_action(
    state: &Arc<Mutex<GameState>>,
    broadcast_tx: &broadcast::Sender<String>,
    player_id: &Option<u32>,
    action: PlayerAction,
    amount: u32,
) -> ServerMessage {
    let pid = match player_id {
        Some(id) => *id,
        None => {
            return ServerMessage::Error {
                message: "Must join first".to_string(),
            };
        }
    };

    let mut state = state.lock().await;

    if !state.game_started {
        return ServerMessage::Error {
            message: "Game not started".to_string(),
        };
    }

    if state.current_player_id() != Some(pid) {
        return ServerMessage::Error {
            message: "Not your turn".to_string(),
        };
    }

    let valid_actions = state.valid_actions(pid);
    if !valid_actions.contains(&action) {
        return ServerMessage::Error {
            message: format!("Invalid action. Valid actions: {:?}", valid_actions),
        };
    }

    let player = match state.players.get(&pid) {
        Some(p) => p.clone(),
        None => {
            return ServerMessage::Error {
                message: "Player not found".to_string(),
            };
        }
    };

    let to_call = state.current_bet.saturating_sub(player.current_bet);
    let mut action_amount: Option<u32> = None;

    match action {
        PlayerAction::Fold => {
            if let Some(p) = state.players.get_mut(&pid) {
                p.status = PlayerStatus::Folded;
            }
        }
        PlayerAction::Check => {
            if to_call != 0 {
                return ServerMessage::Error {
                    message: "Cannot check, must call or raise".to_string(),
                };
            }
            // Big blind has exercised their option (checked without raising)
            if state.phase == GamePhase::PreFlop && state.big_blind_option {
                state.big_blind_option = false;
                // Clear last_raiser since BB posting blind isn't a raise
                // and BB chose to check (not raise), so betting round is complete
                state.last_raiser_index = None;
            }
        }
        PlayerAction::Call => {
            let call_amount = to_call.min(player.chips);
            {
                let p = state.players.get_mut(&pid).unwrap();
                p.chips -= call_amount;
                p.current_bet += call_amount;
                if p.chips == 0 {
                    p.status = PlayerStatus::AllIn;
                }
            }
            state.pot += call_amount;
            action_amount = Some(call_amount);
        }
        PlayerAction::Raise => {
            let raise_total = to_call + amount;
            if raise_total > player.chips {
                return ServerMessage::Error {
                    message: format!(
                        "Not enough chips. Have {}, need {}",
                        player.chips, raise_total
                    ),
                };
            }
            if amount < state.min_raise && raise_total < player.chips {
                return ServerMessage::Error {
                    message: format!("Minimum raise is {}", state.min_raise),
                };
            }

            let new_current_bet;
            {
                let p = state.players.get_mut(&pid).unwrap();
                p.chips -= raise_total;
                p.current_bet += raise_total;
                new_current_bet = p.current_bet;
                if p.chips == 0 {
                    p.status = PlayerStatus::AllIn;
                }
            }
            state.pot += raise_total;
            state.current_bet = new_current_bet;
            state.min_raise = state.big_blind;
            state.last_raiser_index = Some(state.current_player_index);
            state.big_blind_option = false; // Any raise clears the big blind option
            action_amount = Some(raise_total);
        }
        PlayerAction::AllIn => {
            let all_in_amount = player.chips;
            let new_current_bet;
            {
                let p = state.players.get_mut(&pid).unwrap();
                p.chips = 0;
                p.current_bet += all_in_amount;
                new_current_bet = p.current_bet;
                p.status = PlayerStatus::AllIn;
            }
            state.pot += all_in_amount;

            if new_current_bet > state.current_bet {
                state.current_bet = new_current_bet;
                state.last_raiser_index = Some(state.current_player_index);
            }
            action_amount = Some(all_in_amount);
        }
    }

    // Broadcast the action
    let action_msg = ServerMessage::PlayerActed {
        player_id: pid,
        action,
        amount: action_amount,
    };
    let _ = broadcast_tx.send(serde_json::to_string(&action_msg).unwrap());

    // Broadcast pot update
    let pot_msg = ServerMessage::PotUpdate { pot: state.pot };
    let _ = broadcast_tx.send(serde_json::to_string(&pot_msg).unwrap());

    // Mark that at least one player has acted this round
    state.has_acted_this_round = true;

    // Move to next player
    state.next_player();

    // Check if hand is over (only one player left)
    if state.active_player_count() == 1 {
        // Award pot to remaining player
        let messages = state.resolve_hand();
        for msg in messages {
            let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
        }

        // Start new hand if game isn't over
        if state.game_started && state.player_order.len() >= 2 {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            let hand_msgs = state.start_new_hand();
            for msg in hand_msgs {
                let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
            }
            notify_hole_cards(&state, broadcast_tx);
            notify_current_player(&state, broadcast_tx);
        }

        return ServerMessage::Ok;
    }

    // Check if betting round is complete
    if state.is_betting_complete() {
        if state.phase == GamePhase::River {
            // Showdown
            let messages = state.resolve_hand();
            for msg in messages {
                let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
            }

            // Start new hand if game isn't over
            if state.game_started && state.player_order.len() >= 2 {
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                let hand_msgs = state.start_new_hand();
                for msg in hand_msgs {
                    let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
                }
                notify_hole_cards(&state, broadcast_tx);
                notify_current_player(&state, broadcast_tx);
            }
        } else {
            // Advance to next phase
            let phase_messages = state.advance_phase();
            for msg in phase_messages {
                let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
            }

            // Check if only all-in players remain
            if state.actionable_players().is_empty() {
                // This is a flip! Show all hands and equity before running out the board
                broadcast_allin_showdown(&state, broadcast_tx);

                // Run out the board
                while state.phase != GamePhase::Showdown {
                    // Add a small delay between cards for dramatic effect
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    let phase_messages = state.advance_phase();
                    for msg in phase_messages {
                        let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
                    }
                }

                let messages = state.resolve_hand();
                for msg in messages {
                    let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
                }

                // Start new hand
                if state.game_started && state.player_order.len() >= 2 {
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    let hand_msgs = state.start_new_hand();
                    for msg in hand_msgs {
                        let _ = broadcast_tx.send(serde_json::to_string(&msg).unwrap());
                    }
                    notify_hole_cards(&state, broadcast_tx);
                    notify_current_player(&state, broadcast_tx);
                }
            } else {
                notify_current_player(&state, broadcast_tx);
            }
        }
    } else {
        // Notify next player
        notify_current_player(&state, broadcast_tx);
    }

    ServerMessage::Ok
}

fn notify_hole_cards(state: &GameState, broadcast_tx: &broadcast::Sender<String>) {
    for (&pid, player) in &state.players {
        if let Some((c1, c2)) = player.hole_cards {
            let cards = [card_to_info(&c1), card_to_info(&c2)];
            let hole_msg = ServerMessage::HoleCards { cards };
            let _ = broadcast_tx.send(format!(
                "PRIVATE:{}:{}",
                pid,
                serde_json::to_string(&hole_msg).unwrap()
            ));
        }
    }
}

/// Broadcast all-in showdown with hands and equity to all players
fn broadcast_allin_showdown(state: &GameState, broadcast_tx: &broadcast::Sender<String>) {
    // Collect all active/all-in players' hands
    let mut player_hands: Vec<(u32, [CardInfo; 2], Hand)> = Vec::new();

    for &id in &state.player_order {
        if let Some(player) = state.players.get(&id)
            && (player.status == PlayerStatus::Active || player.status == PlayerStatus::AllIn)
            && let Some((c1, c2)) = player.hole_cards
        {
            let cards = [card_to_info(&c1), card_to_info(&c2)];
            player_hands.push((id, cards, Hand(c1, c2)));
        }
    }

    if player_hands.len() < 2 {
        return; // Need at least 2 players for a showdown
    }

    // Build the current board state
    let board = state.build_board();

    // Calculate equity for each player
    let hands_for_calc: Vec<Hand> = player_hands
        .iter()
        .map(|(_, _, h)| Hand(h.0, h.1))
        .collect();
    let equities = calculate_equity_multi(&hands_for_calc, &board, 1000);

    // Build the showdown message
    let hands_with_equity: Vec<(u32, [CardInfo; 2], f64)> = player_hands
        .iter()
        .enumerate()
        .map(|(i, (id, cards, _))| (*id, *cards, equities.get(i).copied().unwrap_or(0.0)))
        .collect();

    let community_cards: Vec<CardInfo> = state.community_cards.iter().map(card_to_info).collect();

    let showdown_msg = ServerMessage::AllInShowdown {
        hands: hands_with_equity,
        community_cards,
    };

    let _ = broadcast_tx.send(serde_json::to_string(&showdown_msg).unwrap());
}

fn notify_current_player(state: &GameState, broadcast_tx: &broadcast::Sender<String>) {
    if let Some(current_id) = state.current_player_id() {
        let current_player = state.players.get(&current_id);
        let your_bet = current_player.map(|p| p.current_bet).unwrap_or(0);
        let valid_actions = state.valid_actions(current_id);

        let turn_msg = ServerMessage::YourTurn {
            current_bet: state.current_bet,
            your_bet,
            pot: state.pot,
            min_raise: state.min_raise,
            valid_actions,
        };
        let _ = broadcast_tx.send(format!(
            "PRIVATE:{}:{}",
            current_id,
            serde_json::to_string(&turn_msg).unwrap()
        ));
    }
}

/// Send a server message to a client
async fn send_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &ServerMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = serde_json::to_string(msg)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

/// Start the poker server
pub async fn start_server(address: &str) -> Result<(), ServerError> {
    let server = PokerServer::new(address);
    server.run().await
}
