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

//! Library facade so benches, examples, and tests can drive the render path.
//! `main.rs` stays a thin entry point and just `use`s this crate.

// The lib target exists only for benches/tests. Promoting the modules to a
// public API trips library-publishing lints that add no value for an internal
// server crate. Allowed here, at the lib root only — `main.rs` still enforces
// the full strict lint set from `Cargo.toml`.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::too_long_first_doc_paragraph
)]

pub mod handlers;
pub mod render;
pub mod room;

use std::sync::Arc;

use room::RoomManager;

/// Shared application state available to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub room_manager: Arc<RoomManager>,
}
