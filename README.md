# Poker — Multiplayer Texas Hold'em

A multiplayer Texas Hold'em poker app. A browser or the desktop client talks to
an SSE server (Datastar) that also serves its own HTML UI.

## Architecture

| Crate | Description |
|-------|-------------|
| `poker-core` | Core game logic, card evaluation, and wire protocol types |
| `poker-sse-server` | Multi-room Axum server using Datastar (SSE + CQRS); serves its own HTML UI |
| `poker-desktop` | Desktop client: OS webview (Wry/Tao) over the `poker-sse-server` UI, with host-a-game mode |

## Quick Start

### Run the server

```bash
# From the workspace root:
cargo build -p poker-sse-server --release
./target/release/poker-sse-server
```

The server listens on `0.0.0.0:3001` by default and serves its HTML UI from the
`static/` directory. Configure with environment variables:

- `PORT` — listen port (default: `3001`)
- `STATIC_DIR` — path to the static assets (default: `static`)
- `CORS_ORIGIN` — comma-separated allowed origins, or unset for permissive CORS

Open `http://localhost:3001` in a browser to play.

### Desktop client

The desktop client wraps the `poker-sse-server` HTML UI in a native window using
the OS webview (WebKitGTK/WebView2/WKWebView via Wry). It has two modes:

- **Client mode** (default): connects to a public server, so desktop users play in
  the same games as browser users.
- **Host mode**: runs `poker-sse-server` in-process and points the window at
  `127.0.0.1`. Friends join you over the LAN (or the internet, with a forwarded
  router port) by opening the address the app shows them.

```bash
cargo build -p poker-desktop --release

# Client mode (connect to a public server):
./target/release/poker-desktop

# Override the server:
./target/release/poker-desktop --server https://my.host/poker

# Host a game yourself (friends use your LAN/internet address):
./target/release/poker-desktop --host
```

In host mode, use the **Server → Show my address…** menu item to reveal the
address to share with friends, e.g. `http://192.168.1.42:3001/poker`. For
internet play, forward that port (default `3001`, override with `--port`) on
your router.

> **Packaging note:** when hosting, the server serves its `static/` assets. Under
> `cargo run` this resolves to the sibling `crates/poker-sse-server/static`
> directory; for a distributable binary, ship that directory alongside and point
> `STATIC_DIR` at it.

## Development

```bash
# Check everything compiles:
cargo check

# Run clippy (strict pedantic + nursery + no-panic lints, enforced workspace-wide):
cargo clippy --workspace -- -D warnings

# Run tests:
cargo test

# Run the server in dev mode:
cargo run -p poker-sse-server
```

## Gameplay

1. One player creates a room (picks a room ID)
2. Other players join using the same room ID
3. Any player can start the game once 2+ players have joined
4. Standard Texas Hold'em rules with blinds, betting rounds, and showdown
