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

//! SSE read side: the `/poker/events` stream and its teardown plumbing.
//!
//! Each client's stream drains a per-player channel of rendered
//! [`DatastarEvent`]s. A [`DisconnectGuard`] wraps the stream so a client
//! disconnect (reload / tab close / network blip) spawns the seat's teardown
//! task deterministically — hyper drops the response future on disconnect, so
//! the `stream::once` finalizer below never runs; cleanup belongs in `Drop`.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::State;
use axum::response::{IntoResponse, Sse};
use datastar::axum::ReadSignals;
use futures_util::{Stream, StreamExt};

use crate::AppState;
use crate::handlers::SessionSignals;
use crate::render;
use crate::room::{RoomManager, resolve_caller};

/// Runs teardown on client disconnect. On reload/tab-close, hyper drops the
/// response future, so the `stream::once` finalizer below never runs. Cleanup
/// belongs in `Drop`, which spawns [`RoomManager::disconnect_player`]. That's
/// idempotent (generation guard), so this drop path and the server-side path
/// can both fire.
struct DisconnectGuard {
    inner: Option<Pin<Box<dyn Stream<Item = SseItem> + Send>>>,
    /// `Some` until `Drop` takes it to spawn the teardown task.
    teardown: Option<(
        Arc<RoomManager>,
        String, // room_id
        u32,    // player_id
        u64,    // connection generation
    )>,
}

/// Item type of the `/poker/events` SSE stream: an axum `Event`, or an
/// infallible error (the inner stream never errors).
pub(crate) type SseItem = Result<axum::response::sse::Event, std::convert::Infallible>;

impl Stream for DisconnectGuard {
    type Item = SseItem;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        this.inner
            .as_mut()
            .map_or_else(|| Poll::Ready(None), |s| s.as_mut().poll_next(cx))
    }
}

impl Drop for DisconnectGuard {
    fn drop(&mut self) {
        if let Some((manager, room_id, pid, generation)) = self.teardown.take() {
            tracing::info!(
                room = %room_id,
                player = pid,
                generation,
                "SSE stream dropped — spawning disconnect teardown"
            );
            tokio::spawn(async move {
                manager.disconnect_player(&room_id, pid, generation).await;
            });
        }
    }
}

pub async fn events(
    State(state): State<AppState>,
    ReadSignals(signals): ReadSignals<SessionSignals>,
) -> impl IntoResponse {
    let manager = state.room_manager.clone();
    let room_id = signals.room_id.clone();
    let token = signals.session_token.clone();

    // Resolve the session; on failure stream a single error event and end.
    // The error becomes a browser alert (targets <body>, so it works before
    // #game-root exists).
    let caller = match resolve_caller(&manager, &room_id, &token).await {
        Ok(c) => c,
        Err(e) => {
            let err = render::error_events_pub(&e);
            let stream = futures_util::stream::iter(
                err.into_iter()
                    .map(|ev| Ok::<_, std::convert::Infallible>(ev.write_as_axum_sse_event())),
            );
            return Sse::new(stream).into_response();
        }
    };

    // If the room already finished (GameOver broadcast), tear it down and
    // tell the client to reset.
    let teardown = {
        let room = caller.room_arc.lock().await;
        room.game_over
    };
    if teardown {
        manager.remove_room(&room_id).await;
        // Notify via alert, then blank the session signals; pokerHandleFetch
        // clears localStorage.
        let mut evs = render::notice_events("This game has ended. Thanks for playing!");
        evs.push(render::patch_signals(
            &serde_json::json!({ "sessiontoken": "", "roomid": "" }),
        ));
        let stream = futures_util::stream::iter(
            evs.into_iter()
                .map(|ev| Ok::<_, std::convert::Infallible>(ev.write_as_axum_sse_event())),
        );
        return Sse::new(stream).into_response();
    }

    let (rx, initial, generation) =
        RoomManager::attach_stream(&caller.room_arc, &room_id, caller.player_id, false).await;

    let pid = caller.player_id;
    let manager2 = manager.clone();
    let rid = room_id.clone();

    let stream = futures_util::stream::iter(
        initial
            .into_iter()
            .map(|ev| Ok::<_, std::convert::Infallible>(ev.write_as_axum_sse_event())),
    )
    .chain(
        tokio_stream::wrappers::ReceiverStream::new(rx).map(|ev| Ok(ev.write_as_axum_sse_event())),
    )
    .chain(futures_util::stream::once(async move {
        // Server-side close path: fires when the inner channel sender is
        // dropped (e.g. the seat is reclaimed elsewhere). Doesn't fire on
        // client disconnect — see [`DisconnectGuard`].
        manager2.detach_stream(&rid, pid, generation).await;
        tracing::info!(room = %rid, player = pid, generation, "SSE stream closed (server-side)");
        // Yield nothing further; this once() exists only for the side effect.
        Ok(datastar::prelude::PatchSignals::new("{}").write_as_axum_sse_event())
    }));

    let guarded = DisconnectGuard {
        inner: Some(Box::pin(stream)),
        teardown: Some((manager.clone(), room_id.clone(), pid, generation)),
    };

    Sse::new(guarded)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
}
