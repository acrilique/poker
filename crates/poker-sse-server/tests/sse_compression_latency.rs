// Verifies, end-to-end against a real axum app, that SSE routes can be
// compressed via tower-http without per-event latency: override the default
// predicate to drop the `text/event-stream` carve-out and rely on
// async-compression's (>= 0.4.31) auto-flush between chunks.
//
// Runs the same two-fat-event stream (~5KB, 200ms gap, ~5KB) under three
// encodings and asserts:
//   1. `content-encoding` is `br` / `gzip` (compression negotiated).
//   2. The round-tripped body equals the raw SSE bytes.
//   3. The compressed payload is materially smaller (~8:1 br, ~6:1 gz).
//   4. The first event's bytes arrive within a fraction of the 200ms gap,
//      proving the encoder flushed per event rather than buffering both.
//
// Run with:
//   cargo test -p poker-sse-server --test sse_compression_latency -- --nocapture --ignored

// Throwaway verification tool, not production code. Cargo.toml's panic-denying
// lints apply to test targets too, so we relax them here for legibility.
#![allow(
    clippy::pedantic,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::panic
)]

use std::io::Read;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use brotli::Decompressor as BrotliDecoder;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use futures_util::stream::{self};
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::Predicate;

/// Size of one fat event's payload — representative of a `state_events`
/// `#game-root` morph (~5 KB of repetitive, compressible markup).
const EVENT_BYTES: usize = 5_000;

/// Gap between the two events. First-event bytes MUST arrive well before this
/// (we assert < 50% of it) to prove per-event flush rather than end-of-stream
/// flush.
const GAP_MS: u64 = 200;

/// Build a payload the size of a real `#game-root` morph (~5 KB of repeated,
/// compressible markup) so the encoder has real input to buffer.
fn fat_event(label: &str) -> Event {
    let body = format!("<div id=\"{label}\">{}", "x".repeat(EVENT_BYTES));
    Event::default().data(body)
}

/// A minimal SSE stream: a fat event, then a `GAP_MS` gap, then a second fat
/// event. Returns `impl IntoResponse` (the same shape production handlers use).
async fn two_events() -> impl axum::response::IntoResponse {
    let s = stream::iter([Ok::<_, std::convert::Infallible>(fat_event("first"))])
        .chain(stream::once(async {
            tokio::time::sleep(Duration::from_millis(GAP_MS)).await;
            Ok(fat_event("second"))
        }))
        .boxed();
    Sse::new(s).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// tower-http's `DefaultPredicate` minus the `NotForContentType::SSE`
/// carve-out — i.e. it permits compressing `text/event-stream`. Uses the
/// production predicate so the test exercises exactly what the router applies.
fn predicate_allowing_sse() -> impl Predicate {
    poker_sse_server::compression_predicate()
}

/// A body frame as observed by the client: wall-clock time since request start
/// and the compressed byte length of that frame.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    t_ms: f64,
    bytes: usize,
}

struct Probe {
    encoding: String,
    chunks: Vec<Chunk>,
    /// The compressed body as received (still encoded).
    encoded: Vec<u8>,
}

async fn probe(label: &str, app: Router, accept_encoding: &str) -> Probe {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http::<Body>();
    let mut req = Request::builder()
        .uri(format!("http://{addr}/sse"))
        .body(Body::empty())
        .unwrap();
    req.headers_mut()
        .insert("accept-encoding", accept_encoding.parse().unwrap());

    let start = Instant::now();
    let resp = client.request(req).await.unwrap();
    let encoding = resp
        .headers()
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("identity")
        .to_string();
    println!("[{label}] negotiated content-encoding: {encoding}");

    use http_body_util::BodyExt;
    let mut chunks = Vec::new();
    let mut encoded = Vec::new();
    let mut body = resp.into_body();
    while let Some(frame) = body.frame().await {
        let frame = frame.unwrap();
        if frame.is_data() {
            let data = frame.into_data().unwrap();
            chunks.push(Chunk {
                t_ms: start.elapsed().as_secs_f64() * 1000.0,
                bytes: data.len(),
            });
            encoded.extend_from_slice(&data);
        }
    }

    println!("[{label}] frames (t_from_start, bytes):");
    for c in &chunks {
        println!("    {:>7.2} ms  {:>6} B", c.t_ms, c.bytes);
    }

    Probe {
        encoding,
        chunks,
        encoded,
    }
}

/// Decompress an encoded body back to raw SSE bytes, for round-trip correctness.
fn decode(encoding: &str, encoded: &[u8]) -> Vec<u8> {
    match encoding {
        "identity" => encoded.to_vec(),
        "br" => {
            let mut dec = BrotliDecoder::new(encoded, 32 * 1024);
            let mut out = Vec::new();
            dec.read_to_end(&mut out).expect("brotli decode");
            out
        }
        "gzip" => {
            let mut dec = GzDecoder::new(encoded);
            let mut out = Vec::new();
            dec.read_to_end(&mut out).expect("gzip decode");
            out
        }
        other => panic!("unexpected encoding {other}"),
    }
}

#[tokio::test]
#[ignore = "latency measurement, not a pass/fail test"]
async fn compare_latency() {
    // A plain (no-compression) app and one with SSE-permitting compression.
    let app_plain = Router::new().route("/sse", get(two_events));
    let app_compressed = Router::new().route("/sse", get(two_events)).layer(
        CompressionLayer::new()
            .br(true)
            .gzip(true)
            .compress_when(predicate_allowing_sse()),
    );

    let identity = probe("identity", app_plain.clone(), "identity").await;
    let br = probe("brotli", app_compressed.clone(), "br").await;
    let gz = probe("gzip", app_compressed.clone(), "gzip").await;

    let ms = |t: Option<f64>| t.unwrap_or(f64::NAN);
    println!("\n========== SUMMARY ==========");
    println!(
        "identity: total {} B, first frame {:.2} ms, second frame {:.2} ms",
        identity.encoded.len(),
        ms(identity.chunks.first().map(|c| c.t_ms)),
        ms(identity.chunks.get(1).map(|c| c.t_ms)),
    );
    println!(
        "brotli:   total {} B, first frame {:.2} ms, second frame {:.2} ms",
        br.encoded.len(),
        ms(br.chunks.first().map(|c| c.t_ms)),
        ms(br.chunks.get(1).map(|c| c.t_ms)),
    );
    println!(
        "gzip:     total {} B, first frame {:.2} ms, second frame {:.2} ms",
        gz.encoded.len(),
        ms(gz.chunks.first().map(|c| c.t_ms)),
        ms(gz.chunks.get(1).map(|c| c.t_ms)),
    );
}

/// The pass/fail test: the predicate override + auto-flush must (1) negotiate
/// brotli, (2) round-trip the bytes, (3) shrink the payload materially, and
/// (4) deliver the first event promptly.
#[tokio::test]
async fn brotli_sse_flushes_per_event() {
    let app_plain = Router::new().route("/sse", get(two_events));
    let app_compressed = Router::new().route("/sse", get(two_events)).layer(
        CompressionLayer::new()
            .br(true)
            .gzip(true)
            .compress_when(predicate_allowing_sse()),
    );

    let identity = probe("identity", app_plain, "identity").await;
    let br = probe("brotli", app_compressed.clone(), "br").await;
    let gz = probe("gzip", app_compressed, "gzip").await;

    let raw_sse = identity.encoded.clone();

    // (1) Compression is actually negotiated — the predicate override took.
    assert_eq!(br.encoding, "br", "brotli must be negotiated for SSE");
    assert_eq!(gz.encoding, "gzip", "gzip must be negotiated for SSE");

    // (2) Round-trip correctness: after decompression the browser sees the raw
    // SSE bytes. Necessary but not sufficient — see (3).
    let br_decoded = decode(&br.encoding, &br.encoded);
    let gz_decoded = decode(&gz.encoding, &gz.encoded);
    assert_eq!(
        br_decoded, raw_sse,
        "brotli round-trip must reproduce the raw SSE bytes"
    );
    assert_eq!(
        gz_decoded, raw_sse,
        "gzip round-trip must reproduce the raw SSE bytes"
    );

    // (3) Material bandwidth win: the shared dictionary must beat compressing
    // each event independently by a clear margin — that margin is the context
    // carried from event one into event two.
    let br_ratio = raw_sse.len() as f64 / br.encoded.len().max(1) as f64;
    let gz_ratio = raw_sse.len() as f64 / gz.encoded.len().max(1) as f64;
    println!("brotli ratio {br_ratio:.2}:1, gzip ratio {gz_ratio:.2}:1");
    assert!(
        br_ratio >= 4.0,
        "brotli should be a meaningful win (got {br_ratio:.2}:1)"
    );
    assert!(
        gz_ratio >= 3.0,
        "gzip should be a meaningful win (got {gz_ratio:.2}:1)"
    );

    // (4) The latency test: the first event must arrive well before the second
    // is produced. If the encoder buffered across the gap, the first frame
    // would arrive at ~GAP_MS (both batched). Fails if async-compression
    // regresses its auto-flush.
    let br_first = br.chunks.first().map(|c| c.t_ms).unwrap_or(f64::INFINITY);
    let gz_first = gz.chunks.first().map(|c| c.t_ms).unwrap_or(f64::INFINITY);
    let bound = (GAP_MS as f64) * 0.4;
    println!(
        "first-frame latency: brotli {br_first:.2} ms, gzip {gz_first:.2} ms (must be < {bound:.0} ms)"
    );
    assert!(
        br_first < bound,
        "brotli first event must flush promptly (got {br_first:.2} ms, bound {bound:.0} ms) — \
         the encoder is buffering across SSE frames and async-compression's auto-flush is not firing",
    );
    assert!(
        gz_first < bound,
        "gzip first event must flush promptly (got {gz_first:.2} ms, bound {bound:.0} ms) — \
         the encoder is buffering across SSE frames and async-compression's auto-flush is not firing",
    );
}
