# Deploy acrilique.com + poker on Fly.io (single binary)

## Goal

Serve my entire website (currently a Next.js static site on Netlify) **and** the poker WebSocket game server from a **single Rust binary** deployed to **Fly.io**. One process, one port, git-push deploy.

## Current state

### Website (`/home/llucsm/dev/js/acrilique.com`)
- **Next.js 15** + React 19 + TypeScript + Tailwind
- Purely static site (no SSR, no API routes beyond what Next generates)
- Currently deployed to Netlify via git push
- Pages: `/` (homepage), `/timestamps`, `/image-tracking`
- Uses themes (light/dark/sepia/sorbet/ocean)

### Poker app (`/home/llucsm/dev/rust/poker`)
- **poker-server** (`crates/poker-server`): Axum server with WebSocket (`/ws`) + REST (`/api/rooms`) + static file serving (SPA fallback)
- **poker-web** (`crates/poker-web`): Dioxus WASM frontend, builds to static files via `dx build --release --platform web --package poker-web`
- The poker-server already serves static files from a `STATIC_DIR` env var (default `./dist`) with SPA fallback to `index.html`
- The Dioxus app is already configured with `base_path = "poker"` in `crates/poker-web/Dioxus.toml`, so all asset paths start with `/poker/`
- The Dioxus app's connection screen auto-detects the WS URL from the page origin (e.g. `wss://acrilique.com/ws`)

## Plan

### 1. Export Next.js as static HTML
- Add `output: 'export'` to `next.config.ts` in the acrilique.com repo
- `npm run build` will produce a static `out/` directory
- Verify it works: the site uses no server-side features

### 2. Update poker-server to serve combined static output
- Modify `crates/poker-server/src/main.rs`:
  - Keep `/ws` (WebSocket) and `/api/rooms` (REST) routes as-is
  - Change the static file serving so it serves from `STATIC_DIR` (default `./dist`)
  - The `dist/` directory will contain:
    ```
    dist/
    ├── index.html          ← Next.js homepage
    ├── timestamps/         ← Next.js page
    ├── image-tracking/     ← Next.js page
    ├── _next/              ← Next.js assets
    ├── poker/              ← Dioxus poker SPA
    │   ├── index.html
    │   └── assets/
    │       ├── poker-web-*.js
    │       ├── poker-web_bg-*.wasm
    │       └── tailwind-*.css
    └── ...
    ```
  - The current SPA fallback (`not_found_service → index.html`) needs adjustment: it should only fallback to `/poker/index.html` for `/poker/*` routes, and to `/index.html` for everything else. OR simpler: serve both as nested services.
  - **Key routing logic:**
    - `GET /ws` → WebSocket upgrade
    - `GET /api/rooms` → JSON room list  
    - `GET /poker/*` → try `dist/poker/*` static files, fallback to `dist/poker/index.html`
    - `GET /*` → try `dist/*` static files, fallback to `dist/index.html` (for Next.js client-side routing if any, e.g. 404 page)

### 3. Create a multi-stage Dockerfile
```dockerfile
# Stage 1: Build Next.js static site
FROM node:20-alpine AS nextjs
WORKDIR /app
COPY acrilique.com/package*.json ./
RUN npm ci
COPY acrilique.com/ ./
RUN npm run build
# output: /app/out/

# Stage 2: Build Dioxus poker-web WASM
FROM rust:1-bookworm AS dioxus
RUN cargo install dioxus-cli
RUN rustup target add wasm32-unknown-unknown
WORKDIR /app
COPY poker/ ./
RUN dx build --release --platform web --package poker-web
# output: /app/target/dx/poker-web/release/web/public/

# Stage 3: Build poker-server binary
FROM rust:1-bookworm AS server
WORKDIR /app
COPY poker/ ./
RUN cargo build --release --bin poker-server
# output: /app/target/release/poker-server

# Stage 4: Final slim image
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app

# Combine static assets
COPY --from=nextjs /app/out/ ./dist/
COPY --from=dioxus /app/target/dx/poker-web/release/web/public/ ./dist/poker/

# Copy server binary
COPY --from=server /app/target/release/poker-server ./

ENV STATIC_DIR=/app/dist
ENV PORT=8080
EXPOSE 8080

CMD ["./poker-server"]
```

> Note: The Dockerfile assumes both repos are available in the build context. Adjust paths based on repo structure — could use a monorepo, git submodules, or a build script that copies files.

### 4. Create `fly.toml`
```toml
app = "acrilique"
primary_region = "mad"

[build]

[http_service]
  internal_port = 8080
  force_https = true
  auto_stop_machines = "stop"
  auto_start_machines = true
  min_machines_running = 0

[checks]
  [checks.health]
    type = "http"
    port = 8080
    path = "/"
    interval = "30s"
    timeout = "5s"
```

### 5. Update poker-server routing for the combined static site

The current `main.rs` has:
```rust
let serve_spa = ServeDir::new(&static_dir)
    .not_found_service(ServeFile::new(format!("{static_dir}/index.html")));
```

This needs to become something like:
```rust
// Poker SPA: /poker/* routes fallback to /poker/index.html
let poker_spa = ServeDir::new(format!("{static_dir}/poker"))
    .not_found_service(ServeFile::new(format!("{static_dir}/poker/index.html")));

// Main site: everything else falls back to /index.html
let main_site = ServeDir::new(&static_dir)
    .not_found_service(ServeFile::new(format!("{static_dir}/index.html")));

let app = Router::new()
    .route("/ws", get(ws_handler))
    .route("/api/rooms", get(rooms_handler))
    .layer(CorsLayer::permissive())
    .with_state(state)
    .nest_service("/poker", poker_spa)
    .fallback_service(main_site);
```

### 6. Add poker link to the Next.js homepage

In `acrilique.com/src/app/page.tsx`, in the Tools section, add:
```tsx
<div>
  <a href="/poker" className="text-foreground hover:underline">
    Poker ♠
  </a>
</div>
```
Use a plain `<a>` tag (not Next.js `<Link>`) since `/poker` is a separate SPA (full page navigation).

## Important details

- The poker-server Cargo.toml is at `crates/poker-server/Cargo.toml` and depends on `poker-core` with `features = ["native"]`
- Next.js config is at `acrilique.com/next.config.ts` — currently empty config, just add `output: 'export'`
- The WASM release build is ~1 MB (the debug build was 37 MB, don't use that)
- The `dx build --release` had a non-fatal `wasm-opt` SIGABRT (DWARF version issue) but still produced a working build
- The Dioxus connection screen has a "Server address" field defaulting to the page origin — when everything's on one domain, this just works
- `fly deploy` from a directory with `fly.toml` + `Dockerfile` deploys everything
- Fly.io free tier: 3 shared-cpu-1x VMs, 256 MB RAM each — more than enough for this
