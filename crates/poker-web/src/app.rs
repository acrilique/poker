//! Root application component for the web frontend.
//!
//! This is a thin web + PWA shell. All game lifecycle logic lives in
//! [`poker_ui::app_logic`]; this module only provides:
//!
//! - Session persistence via `sessionStorage`
//! - The browser-origin WebSocket URL helper
//! - PWA manifest / service-worker / theme-color tags
//! - The root `<App>` Dioxus component that wires everything together

use dioxus::prelude::*;
use poker_client::game_state::ClientGameState;
use poker_client::session::SessionStore;
use poker_ui::app_logic;
use poker_ui::components::{connection_screen, game_screen};
use poker_ui::{Screen, StackDisplayMode, UiMessage};

// ---------------------------------------------------------------------------
// Root component
// ---------------------------------------------------------------------------

const TAILWIND_CSS: Asset = asset!(
    "/assets/tailwind.css",
    AssetOptions::css()
        .with_preload(true)
        .with_static_head(true)
);

// ---------------------------------------------------------------------------
// Session persistence (sessionStorage)
// ---------------------------------------------------------------------------

struct WebSessionStore;

impl SessionStore for WebSessionStore {
    fn save(&self, ws_url: &str, room_id: &str, name: &str, session_token: &str) {
        let window = web_sys::window().unwrap();
        if let Ok(Some(storage)) = window.session_storage() {
            let _ = storage.set_item("poker_ws_url", ws_url);
            let _ = storage.set_item("poker_room_id", room_id);
            let _ = storage.set_item("poker_name", name);
            let _ = storage.set_item("poker_session_token", session_token);
        }
    }

    fn load(&self) -> Option<(String, String, String, String)> {
        let window = web_sys::window()?;
        let storage = window.session_storage().ok()??;
        let ws_url = storage.get_item("poker_ws_url").ok()??;
        let room_id = storage.get_item("poker_room_id").ok()??;
        let name = storage.get_item("poker_name").ok()??;
        let token = storage.get_item("poker_session_token").ok()??;
        if token.is_empty() {
            return None;
        }
        Some((ws_url, room_id, name, token))
    }

    fn clear(&self) {
        let window = web_sys::window().unwrap();
        if let Ok(Some(storage)) = window.session_storage() {
            let _ = storage.remove_item("poker_ws_url");
            let _ = storage.remove_item("poker_room_id");
            let _ = storage.remove_item("poker_name");
            let _ = storage.remove_item("poker_session_token");
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Derive the WebSocket URL from the browser's current page origin.
///
/// `http://host:port` → `ws://host:port`, `https://…` → `wss://…`.
fn default_ws_origin() -> String {
    let window = web_sys::window().expect("no global `window`");
    let location = window.location();
    let protocol = location.protocol().unwrap_or_default();
    let host = location.host().unwrap_or_default();
    let ws_scheme = if protocol == "https:" { "wss" } else { "ws" };
    format!("{ws_scheme}://{host}")
}

/// Platform sleep using gloo-timers (Web / wasm).
async fn sleep_ms(ms: u64) {
    gloo_timers::future::TimeoutFuture::new(ms as u32).await;
}

// ---------------------------------------------------------------------------
// App component
// ---------------------------------------------------------------------------

/// Root `<App>` component.
#[component]
pub fn App() -> Element {
    let screen = use_signal(|| Screen::Connection);
    let game_state = use_signal(|| ClientGameState::new(""));
    let conn_error = use_signal(String::new);
    let ws_origin = use_signal(default_ws_origin);

    // Shared display mode for stacks (blinds vs chips). Default: blinds.
    use_context_provider(|| Signal::new(StackDisplayMode::Blinds));

    // Spawn the networking coroutine — all logic lives in poker_ui::app_logic.
    let _coroutine = use_coroutine(move |rx: UnboundedReceiver<UiMessage>| {
        app_logic::run_app_session(
            rx,
            screen,
            game_state,
            conn_error,
            WebSessionStore,
            sleep_ms,
        )
    });

    let origin = ws_origin.read().clone();

    rsx! {
        document::Stylesheet { href: TAILWIND_CSS }
        document::Link { rel: "manifest", href: "/poker/manifest.json" }
        document::Meta { name: "theme-color", content: "#1A130D" }
        document::Link { rel: "icon", href: "/poker/favicon.ico" }
        document::Script {
            r#"
            if ("serviceWorker" in navigator) {{
                navigator.serviceWorker.register("/poker/sw.js", {{ scope: "/poker/" }});
            }}
            "#
        }
        div { class: "min-h-screen bg-base text-foreground font-sans",
            match &*screen.read() {
                Screen::Connection => rsx! {
                    connection_screen::ConnectionScreen { error: conn_error, default_server: origin }
                },
                Screen::Game => rsx! {
                    game_screen::GameScreen { state: game_state }
                },
            }
        }
    }
}
