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

#[cfg(any(feature = "native", all(feature = "web", target_arch = "wasm32")))]
use crate::transport::TransportError;
#[cfg(feature = "native")]
use crate::transport::{Transport, TransportReader, TransportWriter};
use poker_core::protocol::{ClientMessage, ServerMessage};

// ---------------------------------------------------------------------------
// Channel capacity constants
// ---------------------------------------------------------------------------

/// Capacity for the incoming (server → client) message channel.
///
/// Bounded to prevent unbounded memory growth if the UI thread falls behind
/// (e.g. frozen UI, malicious server).  When full the reader task blocks,
/// applying TCP-level back-pressure to the server.
const INCOMING_CHANNEL_CAPACITY: usize = 128;

/// Capacity for the outgoing (client → server) command channel.
///
/// Human-speed input means this rarely fills up; if it does, `send()` returns
/// an error rather than blocking the UI thread.
const OUTGOING_CHANNEL_CAPACITY: usize = 32;

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
/// - [`incoming`](NetClient::incoming) — an [`mpsc::Receiver<ServerMessage>`]
///   for server messages. The channel closing signals disconnection.
/// - [`send`](NetClient::send) — a non-async, non-blocking method to enqueue
///   a [`ClientMessage`] for transmission.
///
/// Both internal channels are **bounded** so that a slow UI or a malicious
/// server cannot cause unbounded memory growth.
///
/// Background tasks handle the actual I/O, making this safe to use from
/// any async context.
pub struct NetClient {
    /// Receive parsed server messages. Channel close = disconnected.
    pub incoming: mpsc::Receiver<ServerMessage>,
    /// Send-side of the writer channel (kept for [`Self::send`]).
    outgoing: mpsc::Sender<ClientMessage>,
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

        let (msg_tx, msg_rx) = mpsc::channel(INCOMING_CHANNEL_CAPACITY);
        let (cmd_tx, cmd_rx) = mpsc::channel::<ClientMessage>(OUTGOING_CHANNEL_CAPACITY);

        Self::spawn_io_tasks(reader, writer, msg_tx, cmd_rx);

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
    pub async fn connect_ws(url: &str) -> Result<Self, TransportError> {
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
    pub fn send(&self, msg: ClientMessage) -> Result<(), mpsc::error::TrySendError<ClientMessage>> {
        self.outgoing.try_send(msg)
    }

    // ------------------------------------------------------------------
    // WASM WebSocket constructor
    // ------------------------------------------------------------------

    /// Connect to a WebSocket server from a WASM environment.
    ///
    /// Uses `gloo-net` for the WebSocket and `wasm_bindgen_futures::spawn_local`
    /// for background reader/writer tasks (no `Send` requirement).
    #[cfg(all(feature = "web", not(feature = "native"), target_arch = "wasm32"))]
    pub async fn connect_ws(url: &str) -> Result<Self, TransportError> {
        use futures_util::{SinkExt, StreamExt};
        use gloo_net::websocket::{Message, futures::WebSocket};

        let ws = WebSocket::open(url)
            .map_err(|e| TransportError::Io(format!("WebSocket connect failed: {e}")))?;
        let (mut sink, mut stream) = ws.split();

        let (msg_tx, msg_rx) = mpsc::channel(INCOMING_CHANNEL_CAPACITY);
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<ClientMessage>(OUTGOING_CHANNEL_CAPACITY);

        // Single combined I/O task: `select` ensures that when either the
        // reader or writer loop exits, the other is dropped immediately.
        // This prevents a half-dead connection where the UI never sees a
        // disconnect because one side still holds `msg_tx` alive.
        wasm_bindgen_futures::spawn_local(async move {
            use std::pin::pin;

            let reader_fut = async {
                while let Some(frame) = stream.next().await {
                    match frame {
                        Ok(Message::Text(text)) => {
                            if let Some(msg) = parse_server_line(&text)
                                && msg_tx.send(msg).await.is_err()
                            {
                                break;
                            }
                        }
                        Ok(Message::Bytes(_)) => {} // skip binary frames
                        Err(_) => break,
                    }
                }
            };

            let writer_fut = async {
                while let Some(msg) = cmd_rx.recv().await {
                    let json = match serde_json::to_string(&msg) {
                        Ok(j) => j,
                        Err(_) => continue,
                    };
                    if sink.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            };

            futures_util::future::select(pin!(reader_fut), pin!(writer_fut)).await;
            // Whichever finished first, both halves are now dropped —
            // channels close, cleanly signalling disconnect to the UI.
        });

        Ok(Self {
            incoming: msg_rx,
            outgoing: cmd_tx,
        })
    }

    // ------------------------------------------------------------------
    // Private: background task spawners (native only)
    // ------------------------------------------------------------------

    /// Spawn combined reader + writer I/O tasks.
    ///
    /// Both loops run inside **one** `tokio::select!` so that when either
    /// the reader or writer exits (error, EOF, channel close) the other is
    /// cancelled immediately.  This ensures `msg_tx` and `cmd_rx` are both
    /// dropped, so the UI sees a clean disconnect without delay.
    #[cfg(feature = "native")]
    fn spawn_io_tasks<R: TransportReader, W: TransportWriter>(
        mut reader: R,
        mut writer: W,
        msg_tx: mpsc::Sender<ServerMessage>,
        mut cmd_rx: mpsc::Receiver<ClientMessage>,
    ) {
        tokio::spawn(async move {
            tokio::select! {
                () = async {
                    while let Ok(Some(line)) = reader.recv().await {
                        if let Some(msg) = parse_server_line(&line)
                            && msg_tx.send(msg).await.is_err()
                        {
                            break;
                        }
                    }
                } => {}
                () = async {
                    while let Some(msg) = cmd_rx.recv().await {
                        let json = match serde_json::to_string(&msg) {
                            Ok(j) => j,
                            Err(_) => continue,
                        };
                        if writer.send(&json).await.is_err() {
                            break;
                        }
                    }
                } => {}
            }
            // Whichever branch finished first, both `msg_tx` and `cmd_rx`
            // are dropped here, cleanly signalling disconnect to the UI.
        });
    }
}
