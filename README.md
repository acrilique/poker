# Poker — Multiplayer Texas Hold'em

A multiplayer Texas Hold'em poker app with several frontends (web, desktop, TUI) connecting to shared servers (WebSocket and SSE).

## Architecture

| Crate | Description |
|-------|-------------|
| `poker-core` | Core game logic, protocol types, and transport abstraction |
| `poker-sse-server` | Multi-room Axum server using Datastar (SSE + CQRS); serves its own HTML UI |
| `poker-desktop` | Desktop client: OS webview (Wry/Tao) over the `poker-sse-server` UI, with host-a-game mode |
| `poker-ws-server` | Multi-room Axum server with WebSocket support |
| `poker-client` | Client-side networking, game state, session persistence, and reconnection logic |
| `poker-ui` | Platform-agnostic Dioxus UI components and application lifecycle |
| `poker-web` | Thin Dioxus web + PWA shell (WASM) |
| `poker-tui` | Ratatui terminal frontend |

## Quick Start

### 1. Build the web frontend

```bash
cd crates/poker-web
npm install
npx tailwindcss -i assets/input.css -o assets/tailwind.css
dx build --release
```

This produces a `dist/` directory with the static web assets.

### 2. Run the server

```bash
# From the workspace root:
cargo build -p poker-ws-server --release

# Serve the web frontend from the dist/ directory:
STATIC_DIR=crates/poker-web/dist ./target/release/poker-ws-server
```

The server listens on `0.0.0.0:8080` by default. Configure with:

- `PORT` — listen port (default: `8080`)
- `STATIC_DIR` — path to the Dioxus web build output (default: `./dist`)

Open `http://localhost:8080` in a browser to play.

### 3. TUI client

```bash
cargo build -p poker-tui --release

# Create a room and join:
./target/release/poker --server ws://127.0.0.1:8080 --room myroom --name Alice --create

# Join an existing room:
./target/release/poker --server ws://127.0.0.1:8080 --room myroom --name Bob
```

### 4. Desktop client

The desktop client wraps the `poker-sse-server` HTML UI in a native window using the OS webview (WebKitGTK/WebView2/WKWebView via Wry). It has two modes:

- **Client mode** (default): connects to the public server, so desktop users play in the same games as browser users.
- **Host mode**: runs `poker-sse-server` in-process and points the window at `127.0.0.1`. Friends join you over the LAN (or the internet, with a forwarded router port) by opening the address the app shows them.

```bash
cargo build -p poker-desktop --release

# Client mode (connect to the default public server):
./target/release/poker-desktop

# Override the server:
./target/release/poker-desktop --server https://my.host/poker

# Host a game yourself (friends use your LAN/internet address):
./target/release/poker-desktop --host
```

In host mode, use the **Server → Show my address…** menu item to reveal the address to share with friends, e.g. `http://192.168.1.42:3001/poker`. For internet play, forward that port (default `3001`, override with `--port`) on your router.

> **Packaging note:** when hosting, the server serves its `static/` assets. Under `cargo run` this resolves to the sibling `crates/poker-sse-server/static` directory; for a distributable binary, ship that directory alongside and point `STATIC_DIR` at it.

## Development

```bash
# Check everything compiles:
cargo check

# Run tests:
cargo test

# Run the server in dev mode:
cargo run -p poker-ws-server

# Build the web frontend in dev mode:
cd crates/poker-web && dx serve
```

## Gameplay

1. One player creates a room (picks a room ID)
2. Other players join using the same room ID
3. Any player can start the game once 2+ players have joined
4. Standard Texas Hold'em rules with blinds, betting rounds, and showdown
