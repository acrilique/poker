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

//! Transport abstraction for network communication.
//!
//! Decouples the networking layer from any specific transport (TCP, WebSocket,
//! etc.). [`NetClient`](crate::net_client::NetClient) uses the [`Transport`]
//! trait to establish connections without caring about the underlying protocol.

use std::future::Future;

use thiserror::Error;

/// Errors that can occur during transport operations.
#[derive(Debug, Error)]
pub enum TransportError {
    /// The remote peer closed the connection.
    #[error("connection closed")]
    ConnectionClosed,

    /// An I/O or protocol-level error.
    #[error("{0}")]
    Io(String),
}

/// Read half of a transport connection.
///
/// Implementations receive text messages (typically JSON) from the remote peer.
pub trait TransportReader: Send + 'static {
    /// Receive the next text message.
    ///
    /// Returns `Ok(None)` when the connection is cleanly closed.
    fn recv(&mut self) -> impl Future<Output = Result<Option<String>, TransportError>> + Send;
}

/// Write half of a transport connection.
///
/// Implementations send text messages (typically JSON) to the remote peer.
pub trait TransportWriter: Send + 'static {
    /// Send a text message to the remote peer.
    fn send(&mut self, text: &str) -> impl Future<Output = Result<(), TransportError>> + Send;
}

/// A bidirectional transport that can be split into independent read and write
/// halves.
///
/// This allows the reader and writer to be moved into separate async tasks
/// for concurrent I/O.
pub trait Transport: Send + 'static {
    /// The read half produced by [`split`](Transport::split).
    type Reader: TransportReader;
    /// The write half produced by [`split`](Transport::split).
    type Writer: TransportWriter;

    /// Split the transport into independent read and write halves.
    fn split(self) -> (Self::Reader, Self::Writer);
}
