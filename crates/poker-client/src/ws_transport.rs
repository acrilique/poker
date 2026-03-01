// Copyright (C) 2026 Lluc Simó Margalef <lluc.simo@protonmail.com>
// SPDX-License-Identifier: GPL-3.0-only
//
// This file is part of acrilique/poker.
//
// poker is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, version 3 of the License.
//
// poker is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with poker. If not, see <https://www.gnu.org/licenses/>.

//! WebSocket transport implementation for native targets.
//!
//! Uses `tokio-tungstenite` to provide a [`Transport`] over WebSocket
//! connections. This is used by both the desktop app and the TUI client.

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::transport::{Transport, TransportError, TransportReader, TransportWriter};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// WebSocket transport for native (non-WASM) targets.
pub struct WsTransport {
    stream: WsStream,
}

impl WsTransport {
    /// Connect to a WebSocket server at the given URL.
    ///
    /// Supports both `ws://` and `wss://` schemes.
    pub async fn connect(url: &str) -> Result<Self, TransportError> {
        let (stream, _response) = connect_async(url)
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(Self { stream })
    }
}

impl Transport for WsTransport {
    type Reader = WsReader;
    type Writer = WsWriter;

    fn split(self) -> (Self::Reader, Self::Writer) {
        let (sink, stream) = self.stream.split();
        (WsReader { stream }, WsWriter { sink })
    }
}

/// Read half of a WebSocket transport.
pub struct WsReader {
    stream: SplitStream<WsStream>,
}

impl TransportReader for WsReader {
    async fn recv(&mut self) -> Result<Option<String>, TransportError> {
        loop {
            match self.stream.next().await {
                Some(Ok(Message::Text(text))) => return Ok(Some(text.to_string())),
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                // Skip binary, ping, pong frames — continue to next message.
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(TransportError::Io(e.to_string())),
            }
        }
    }
}

/// Write half of a WebSocket transport.
pub struct WsWriter {
    sink: SplitSink<WsStream, Message>,
}

impl TransportWriter for WsWriter {
    async fn send(&mut self, text: &str) -> Result<(), TransportError> {
        self.sink
            .send(Message::text(text))
            .await
            .map_err(|e| TransportError::Io(e.to_string()))
    }
}
