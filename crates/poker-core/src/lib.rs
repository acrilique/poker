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

pub mod game_logic;
pub mod poker;

/// Test/bench/example fixtures (card, board, and game-state builders).
///
/// Gated behind the `test-util` feature (enabled by default) so benches —
/// which link the library without `cfg(test)` — and examples in other crates
/// can reuse the same helpers as the unit tests instead of re-implementing
/// them.
#[cfg(any(test, feature = "test-util"))]
pub mod test_util;
