//! Framework-agnostic network client for the poker server.
//!
//! Spawns background reader/writer tasks and exposes channels so that the
//! frontend can send and receive messages without owning the TCP stream
//! directly.
//!
//! Use [`NetClient::from_transport`] to construct a client over any
//! [`Transport`](crate::transport::Transport) implementation, or the
//! convenience method [`connect_ws`](NetClient::connect_ws) (WebSocket).

use tokio::sync::mpsc;

#[cfg(feature = "native")]
use crate::transport::{Transport, TransportReader, TransportWriter};
use poker_core::protocol::{ClientMessage, ServerMessage};

// ---------------------------------------------------------------------------
// Wire-level parsing
// ---------------------------------------------------------------------------

/// Try to deserialize a raw text frame as a [`ServerMessage`].
///
/// Returns `None` for empty/whitespace-only input or unrecognised JSON.
pub fn parse_server_line(line: &str) -> Option<ServerMessage> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str::<ServerMessage>(trimmed).ok()
}

// ---------------------------------------------------------------------------
// NetClient
// ---------------------------------------------------------------------------

/// A channel-based network client for the poker server.
///
/// Construct with [`NetClient::from_transport`] (generic), or use the
/// convenience method [`connect_ws`](NetClient::connect_ws) (WebSocket).
///
/// The returned client exposes:
/// - [`incoming`](NetClient::incoming) — an [`mpsc::UnboundedReceiver<ServerMessage>`]
///   for server messages. The channel closing signals disconnection.
/// - [`send`](NetClient::send) — a non-async, non-blocking method to enqueue
///   a [`ClientMessage`] for transmission.
///
/// Background tasks handle the actual I/O, making this safe to use from
/// any async context.
pub struct NetClient {
    /// Receive parsed server messages. Channel close = disconnected.
    pub incoming: mpsc::UnboundedReceiver<ServerMessage>,
    /// Send-side of the writer channel (kept for [`Self::send`]).
    outgoing: mpsc::UnboundedSender<ClientMessage>,
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

        let (msg_tx, msg_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientMessage>();

        Self::spawn_reader_task(reader, msg_tx);
        Self::spawn_writer_task(writer, cmd_rx);

        Self {
            incoming: msg_rx,
            outgoing: cmd_tx,
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
    // Shared helpers
    // ------------------------------------------------------------------

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

        let (msg_tx, msg_rx) = mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<ClientMessage>();

        // Reader task (spawn_local — no Send required)
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(frame) = stream.next().await {
                match frame {
                    Ok(Message::Text(text)) => {
                        if let Some(msg) = parse_server_line(&text)
                            && msg_tx.send(msg).is_err()
                        {
                            break;
                        }
                    }
                    Ok(Message::Bytes(_)) => {} // skip binary frames
                    Err(_) => break,
                }
            }
            // Stream ended or error — channel drops, signalling disconnect.
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
            incoming: msg_rx,
            outgoing: cmd_tx,
        })
    }

    // ------------------------------------------------------------------
    // Private: background task spawners (native only)
    // ------------------------------------------------------------------

    /// Spawn the generic reader task that reads from any [`TransportReader`].
    #[cfg(feature = "native")]
    fn spawn_reader_task<R: TransportReader>(
        mut reader: R,
        msg_tx: mpsc::UnboundedSender<ServerMessage>,
    ) {
        tokio::spawn(async move {
            while let Ok(Some(line)) = reader.recv().await {
                if let Some(msg) = parse_server_line(&line)
                    && msg_tx.send(msg).is_err()
                {
                    break;
                }
            }
            // Connection closed or error — channel drops, signalling disconnect.
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
