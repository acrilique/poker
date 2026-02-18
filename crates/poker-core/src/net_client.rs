//! Framework-agnostic network client for the poker server.
//!
//! Spawns background reader/writer tasks and exposes channels so that the
//! frontend can send and receive messages without owning the TCP stream
//! directly.
//!
//! Use [`NetClient::from_transport`] to construct a client over any
//! [`Transport`](crate::transport::Transport) implementation, or the
//! convenience methods [`connect`](NetClient::connect) (TCP) and
//! [`connect_ws`](NetClient::connect_ws) (WebSocket).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(feature = "legacy-server")]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(feature = "legacy-server")]
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::protocol::{ClientMessage, ServerMessage};
#[cfg(feature = "native")]
use crate::transport::{Transport, TransportReader, TransportWriter};

// ---------------------------------------------------------------------------
// Wire-level parsing
// ---------------------------------------------------------------------------

/// Outcome of parsing one server line.
#[derive(Debug)]
pub enum ServerLine {
    /// A broadcast or targeted-to-us message, already deserialized.
    Message(ServerMessage),
    /// A PRIVATE message aimed at a different player — skip it.
    NotForUs,
    /// Empty / blank line — skip it.
    Empty,
    /// Couldn't parse the line (kept as raw text for logging).
    Unknown(String),
}

/// Parse a raw server line (handling the `PRIVATE:` wire prefix and JSON).
///
/// `our_player_id` is needed to filter `PRIVATE:player_id:json` messages.
/// Pass `0` before the server has confirmed our id (accepts all private messages).
pub fn parse_server_line(line: &str, our_player_id: u32) -> ServerLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ServerLine::Empty;
    }

    if trimmed.starts_with("PRIVATE:") {
        let parts: Vec<&str> = trimmed.splitn(3, ':').collect();
        if parts.len() == 3
            && let Ok(target_id) = parts[1].parse::<u32>()
        {
            if (our_player_id == 0 || target_id == our_player_id)
                && let Ok(msg) = serde_json::from_str::<ServerMessage>(parts[2])
            {
                return ServerLine::Message(msg);
            }
            return ServerLine::NotForUs;
        }
        return ServerLine::Unknown(trimmed.to_string());
    }

    match serde_json::from_str::<ServerMessage>(trimmed) {
        Ok(msg) => ServerLine::Message(msg),
        Err(_) => ServerLine::Unknown(trimmed.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Channel-based network events
// ---------------------------------------------------------------------------

/// High-level events produced by the background reader task.
#[derive(Debug)]
pub enum NetEvent {
    /// A successfully parsed and filtered [`ServerMessage`].
    Message(ServerMessage),
    /// An unrecognized line from the server (kept for logging).
    Unknown(String),
    /// The server closed the connection cleanly.
    Disconnected,
    /// An I/O error occurred on the connection.
    Error(String),
}

// ---------------------------------------------------------------------------
// NetClient
// ---------------------------------------------------------------------------

/// A channel-based network client for the poker server.
///
/// Construct with [`NetClient::from_transport`] (generic), or use the
/// convenience methods [`connect`](NetClient::connect) (TCP) and
/// [`connect_ws`](NetClient::connect_ws) (WebSocket).
///
/// The returned client exposes:
/// - [`incoming`](NetClient::incoming) — an [`mpsc::UnboundedReceiver<NetEvent>`]
///   for server events.
/// - [`send`](NetClient::send) — a non-async, non-blocking method to enqueue
///   a [`ClientMessage`] for transmission.
///
/// Background tasks handle the actual I/O, making this safe to use from
/// any async context.
pub struct NetClient {
    /// Receive parsed server events.
    pub incoming: mpsc::UnboundedReceiver<NetEvent>,
    /// Send-side of the writer channel (kept for [`Self::send`]).
    outgoing: mpsc::UnboundedSender<ClientMessage>,
    /// Shared player ID used by the reader task to filter `PRIVATE:` lines.
    player_id: Arc<AtomicU32>,
}

impl NetClient {
    // ------------------------------------------------------------------
    // Generic transport constructor (native only — uses tokio::spawn)
    // ------------------------------------------------------------------

    /// Create a `NetClient` over any [`Transport`] implementation.
    ///
    /// Splits the transport into read/write halves, spawns background tasks,
    /// and returns the ready-to-use client. No handshake messages are sent —
    /// the caller is responsible for sending `Join`/`JoinRoom` afterwards.
    #[cfg(feature = "native")]
    pub fn from_transport<T: Transport>(transport: T) -> Self {
        let (reader, writer) = transport.split();

        let player_id = Arc::new(AtomicU32::new(0));
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientMessage>();

        Self::spawn_reader_task(reader, Arc::clone(&player_id), event_tx);
        Self::spawn_writer_task(writer, cmd_rx);

        Self {
            incoming: event_rx,
            outgoing: cmd_tx,
            player_id,
        }
    }

    // ------------------------------------------------------------------
    // WebSocket convenience constructor
    // ------------------------------------------------------------------

    /// Connect to a WebSocket server and spawn background I/O tasks.
    ///
    /// No handshake messages are sent automatically — the caller should send
    /// `JoinRoom` (or `CreateRoom` + `JoinRoom`) after construction.
    #[cfg(feature = "native")]
    pub async fn connect_ws(url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let transport = crate::ws_transport::WsTransport::connect(url).await?;
        Ok(Self::from_transport(transport))
    }

    // ------------------------------------------------------------------
    // Legacy TCP convenience constructor
    // ------------------------------------------------------------------

    /// Connect to the server at `address` over TCP, perform the join
    /// handshake, and spawn background reader/writer tasks.
    ///
    /// Only available with the `legacy-server` feature (old TCP protocol).
    #[cfg(feature = "legacy-server")]
    pub async fn connect(address: &str, name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let stream = TcpStream::connect(address).await?;
        let (read_half, write_half) = stream.into_split();

        let player_id = Arc::new(AtomicU32::new(0));
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<ClientMessage>();

        // ── Reader task ──────────────────────────────────────────────────
        let pid = Arc::clone(&player_id);
        tokio::spawn(async move {
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        let _ = event_tx.send(NetEvent::Disconnected);
                        break;
                    }
                    Ok(_) => {
                        let our_id = pid.load(Ordering::Relaxed);
                        match parse_server_line(&line, our_id) {
                            ServerLine::Message(msg) => {
                                if event_tx.send(NetEvent::Message(msg)).is_err() {
                                    break;
                                }
                            }
                            ServerLine::Unknown(raw) => {
                                if event_tx.send(NetEvent::Unknown(raw)).is_err() {
                                    break;
                                }
                            }
                            ServerLine::NotForUs | ServerLine::Empty => {}
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(NetEvent::Error(e.to_string()));
                        break;
                    }
                }
            }
        });

        // ── Writer task ──────────────────────────────────────────────────
        tokio::spawn(async move {
            let mut writer = write_half;
            while let Some(msg) = cmd_rx.recv().await {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(_) => continue,
                };
                if writer.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(b"\n").await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
        });

        // ── Initial handshake ────────────────────────────────────────────
        cmd_tx.send(ClientMessage::Join {
            name: name.to_string(),
        })?;
        cmd_tx.send(ClientMessage::GetPlayers)?;

        Ok(Self {
            incoming: event_rx,
            outgoing: cmd_tx,
            player_id,
        })
    }

    // ------------------------------------------------------------------
    // Shared helpers
    // ------------------------------------------------------------------

    /// Update the player ID used by the reader task to filter `PRIVATE:` messages.
    ///
    /// Call this once you receive a [`ServerMessage::JoinedGame`] with your id.
    pub fn set_player_id(&self, id: u32) {
        self.player_id.store(id, Ordering::Relaxed);
    }

    /// Enqueue a [`ClientMessage`] for transmission to the server.
    ///
    /// This is non-blocking — the message is written to a channel and the
    /// background writer task handles the actual I/O.
    pub fn send(&self, msg: ClientMessage) -> Result<(), mpsc::error::SendError<ClientMessage>> {
        self.outgoing.send(msg)
    }

    // ------------------------------------------------------------------
    // WASM WebSocket constructor
    // ------------------------------------------------------------------

    /// Connect to a WebSocket server from a WASM environment.
    ///
    /// Uses `gloo-net` for the WebSocket and `wasm_bindgen_futures::spawn_local`
    /// for background reader/writer tasks (no `Send` requirement).
    #[cfg(all(feature = "web", not(feature = "native")))]
    pub async fn connect_ws(url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        use futures_util::{SinkExt, StreamExt};
        use gloo_net::websocket::{Message, futures::WebSocket};

        let ws = WebSocket::open(url).map_err(|e| format!("WebSocket connect failed: {e}"))?;
        let (mut sink, mut stream) = ws.split();

        let player_id = Arc::new(AtomicU32::new(0));
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<ClientMessage>();

        // Reader task (spawn_local — no Send required)
        let pid = Arc::clone(&player_id);
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        let our_id = pid.load(Ordering::Relaxed);
                        match parse_server_line(&text, our_id) {
                            ServerLine::Message(msg) => {
                                if event_tx.send(NetEvent::Message(msg)).is_err() {
                                    break;
                                }
                            }
                            ServerLine::Unknown(raw) => {
                                if event_tx.send(NetEvent::Unknown(raw)).is_err() {
                                    break;
                                }
                            }
                            ServerLine::NotForUs | ServerLine::Empty => {}
                        }
                    }
                    Ok(Message::Bytes(_)) => {} // skip binary frames
                    Err(e) => {
                        let _ = event_tx.send(NetEvent::Error(format!("{e}")));
                        break;
                    }
                }
            }
            // Stream ended — connection closed.
        });

        // Writer task (spawn_local — no Send required)
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(msg) = cmd_rx.recv().await {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(_) => continue,
                };
                if sink.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            incoming: event_rx,
            outgoing: cmd_tx,
            player_id,
        })
    }

    // ------------------------------------------------------------------
    // Private: background task spawners (native only)
    // ------------------------------------------------------------------

    /// Spawn the generic reader task that reads from any [`TransportReader`].
    #[cfg(feature = "native")]
    fn spawn_reader_task<R: TransportReader>(
        mut reader: R,
        player_id: Arc<AtomicU32>,
        event_tx: mpsc::UnboundedSender<NetEvent>,
    ) {
        tokio::spawn(async move {
            loop {
                match reader.recv().await {
                    Ok(Some(line)) => {
                        let our_id = player_id.load(Ordering::Relaxed);
                        match parse_server_line(&line, our_id) {
                            ServerLine::Message(msg) => {
                                if event_tx.send(NetEvent::Message(msg)).is_err() {
                                    break;
                                }
                            }
                            ServerLine::Unknown(raw) => {
                                if event_tx.send(NetEvent::Unknown(raw)).is_err() {
                                    break;
                                }
                            }
                            ServerLine::NotForUs | ServerLine::Empty => {}
                        }
                    }
                    Ok(None) => {
                        let _ = event_tx.send(NetEvent::Disconnected);
                        break;
                    }
                    Err(e) => {
                        let _ = event_tx.send(NetEvent::Error(e.to_string()));
                        break;
                    }
                }
            }
        });
    }

    /// Spawn the generic writer task that writes to any [`TransportWriter`].
    #[cfg(feature = "native")]
    fn spawn_writer_task<W: TransportWriter>(
        mut writer: W,
        mut cmd_rx: mpsc::UnboundedReceiver<ClientMessage>,
    ) {
        tokio::spawn(async move {
            while let Some(msg) = cmd_rx.recv().await {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(_) => continue,
                };
                if writer.send(&json).await.is_err() {
                    break;
                }
            }
        });
    }
}
