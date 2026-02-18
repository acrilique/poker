# Poker — Multiplayer Texas Hold'em

A multiplayer Texas Hold'em poker app with three frontends (web, desktop, TUI) connecting to a shared WebSocket server.

## Architecture

| Crate | Description |
|-------|-------------|
| `poker-core` | Core game logic, protocol, transport abstraction, game state |
| `poker-server` | Multi-room Axum server with WebSocket support |
| `poker-web` | Dioxus web frontend (WASM) |
| `poker-gui` | Dioxus desktop frontend (native window) |
| `poker-tui` | Ratatui terminal frontend |
| `poker-ui` | Shared Dioxus UI components (used by web + desktop) |

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
cargo build -p poker-server --release

# Serve the web frontend from the dist/ directory:
STATIC_DIR=crates/poker-web/dist ./target/release/poker-server
```

The server listens on `0.0.0.0:8080` by default. Configure with:

- `PORT` — listen port (default: `8080`)
- `STATIC_DIR` — path to the Dioxus web build output (default: `./dist`)

Open `http://localhost:8080` in a browser to play.

### 3. Desktop app (optional)

```bash
cargo build -p poker-gui --release
./target/release/poker-gui
```

Connects to the same server as the web app — enter the server address, room ID, and your name.

### 4. TUI client (optional)

```bash
cargo build -p poker-tui --release

# Create a room and join:
./target/release/poker --server ws://127.0.0.1:8080 --room myroom --name Alice --create

# Join an existing room:
./target/release/poker --server ws://127.0.0.1:8080 --room myroom --name Bob
```

## Development

```bash
# Check everything compiles:
cargo check

# Run tests:
cargo test

# Run the server in dev mode:
cargo run -p poker-server

# Run the desktop app in dev mode:
cd crates/poker-gui && dx serve

# Build the web frontend in dev mode:
cd crates/poker-web && dx serve
```

## Gameplay

1. One player creates a room (picks a room ID)
2. Other players join using the same room ID
3. Any player can start the game once 2+ players have joined
4. Standard Texas Hold'em rules with blinds, betting rounds, and showdown
