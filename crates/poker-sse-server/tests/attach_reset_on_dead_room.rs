// Regression test for the "reload loops forever on a dead room" bug.
//
// When the persisted session points at a room that no longer exists (room torn
// down, or a stale token), GET /poker/events must both surface the toast AND
// blank the session signals so pokerHandleFetch clears localStorage on that
// same load. Without the signal patch, a reload reopens the stream against the
// same dead room and the "room doesn't exist" toast loops indefinitely.
//
// Drives the real axum router (same shape as tests/sse_compression_latency.rs)
// and asserts the SSE body carries an empty sessiontoken/roomid patch.

// Throwaway verification tool, not production code. Cargo.toml's pedantic
// lints apply to test targets too, so we relax them here for legibility.
#![allow(
    clippy::pedantic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::needless_pass_by_value,
    clippy::items_after_statements
)]

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use poker_sse_server::{ServerConfig, build_router};

/// Serialize the Datastar signals query exactly as the JS SDK does for a GET:
/// `?datastar=<url-encoded JSON>`. The `events` handler deserializes this into
/// `SessionSignals { roomid, sessiontoken }`.
fn datastar_query(room_id: &str, token: &str) -> String {
    let json = format!(r#"{{"roomid":{room_id:?},"sessiontoken":{token:?}}}"#);
    format!("?datastar={}", urlencoding(&json))
}

/// Minimal percent-encoding for the JSON query value (quotes, braces, etc.).
/// Avoids pulling in a `urlencoding` dev-dependency for a single test.
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '"' => "%22".to_string(),
            '{' => "%7B".to_string(),
            '}' => "%7D".to_string(),
            ':' => "%3A".to_string(),
            ',' => "%2C".to_string(),
            ' ' => "%20".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

/// GET /poker/events with the given signals and return the raw SSE body.
async fn events_body(addr: std::net::SocketAddr, room_id: &str, token: &str) -> String {
    let client = Client::builder(TokioExecutor::new()).build_http::<Body>();
    let req = Request::builder()
        .uri(format!(
            "http://{addr}/poker/events{}",
            datastar_query(room_id, token)
        ))
        .header("datastar-request", "true")
        .body(Body::empty())
        .unwrap();
    let resp = client.request(req).await.expect("GET /poker/events");
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("body collected")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf8 SSE body")
}

#[tokio::test]
async fn dead_room_blanks_session_signals() {
    // A router with one real room, plus a known-stale room id that doesn't
    // exist. We never touch the live room here — it only proves the test
    // harness isn't accidentally hitting an empty 404 path.
    let app = build_router(&ServerConfig::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let body = events_body(addr, "ghost-room", "stale-token").await;

    // (1) The error toast is surfaced ("Room 'ghost-room' not found").
    assert!(
        body.contains("Room 'ghost-room' not found"),
        "missing the room-not-found toast in:\n{body}"
    );

    // (2) The session signals are blanked so pokerHandleFetch clears
    // localStorage on this load. A PatchSignals event serializes its JSON
    // across one or more `data: signals <line>` datalines.
    assert!(
        body.contains(r#""sessiontoken":"""#) && body.contains(r#""roomid":"""#),
        "the dead-room attach must blank sessiontoken/roomid so a reload stops \
         reopening the stream; got:\n{body}"
    );
}
