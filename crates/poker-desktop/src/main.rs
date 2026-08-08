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

//! Desktop poker client.
//!
//! A native window ([`tao`]) hosting the OS webview ([`wry`]) pointed at the
//! [`poker_sse_server`] HTML UI. A native menu bar ([`muda`]) switches between:
//!
//! - **client mode** — webview → a remote server (the public default, or one
//!   entered via "Connect to server…"), so desktop users play in the same games
//!   as browser users; and
//! - **host mode** — an embedded [`poker_sse_server`] runs in-process, the
//!   webview points at `127.0.0.1`, and friends join over the LAN (or the
//!   internet with a forwarded router port).
//!
//! The webview is `!Send`, so it lives only on the tao event-loop thread.
//! Menu clicks arrive via [`muda`] as a [`MenuEvent`], which we forward to the
//! event loop through an [`EventLoopProxy`]; the loop then acts on the webview.

pub(crate) mod config;
pub(crate) mod server;

use std::cell::RefCell;
use std::rc::Rc;

use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

use crate::config::Cli;
use crate::server::{Handle, lan_invite_url};

/// Menu item identifiers. Compared against [`MenuEvent::id`] to route clicks.
mod menu_id {
    pub const CONNECT_DEFAULT: &str = "connect_default";
    pub const CONNECT_SERVER: &str = "connect_server";
    pub const HOST: &str = "host";
    pub const SHOW_ADDRESS: &str = "show_address";
    pub const QUIT: &str = "quit";
}

/// Internal commands the event loop processes, sourced from the menu.
enum Command {
    /// Point the webview at the public server, stopping the embedded server if
    /// it is running. The universal "go home" action.
    ConnectDefault,
    /// Open a `prompt()` dialog asking for a server address; the URL comes back
    /// via [`Command::ConnectServer`]. Dispatched directly from the menu-event
    /// handler, not routed through the event loop.
    ConnectServerPrompt(EventLoopProxy<Self>),
    /// Point the webview at the given server URL (entered via the connect
    /// dialog), stopping the embedded server if it is running.
    ConnectServer(String),
    /// Start the embedded server and point the webview at `127.0.0.1`.
    Host,
    /// Show the LAN invite URL (host mode only).
    ShowAddress,
    /// Fired by the page-load hook whenever a navigation finishes. If the
    /// host-entering paths marked `pending_invite`, this consumes the flag and
    /// injects the invite overlay now that the new DOM is ready.
    CheckPendingInvite,
    Quit,
}

// All panic sites below (`Runtime::new`, `WindowBuilder::build`, webview
// build, GTK vbox) are unrecoverable startup failures: no display, no webview
// backend, or broken GTK init. A desktop app that can't open a window has no
// meaningful recovery path.
#[allow(clippy::expect_used, clippy::panic)]
fn main() {
    // Initialise tracing (respects RUST_LOG env var).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // The embedded server runs on a tokio runtime on background threads. The
    // runtime guard must outlive the event loop, so it's leaked — the process
    // exits when the window closes, taking the runtime with it. We keep a
    // handle so `server::start` can spawn onto it from the (non-tokio) main
    // thread that runs the tao loop.
    let runtime = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
    let runtime_handle = runtime.handle().clone();
    std::mem::forget(runtime);

    // If launched with --host, start the server eagerly so it's listening
    // before the webview opens; carry the handle into the event-loop state.
    let host_port = cli.port();
    let (initial_host, initial_url) = if cli.host() {
        let handle = server::start(host_port, &runtime_handle);
        let url = format!("http://127.0.0.1:{host_port}/poker");
        (Some(handle), url)
    } else {
        (None, cli.server().to_string())
    };

    let event_loop = EventLoopBuilder::<Command>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // ---- Menu bar (muda) -------------------------------------------------
    // Forward muda's channel events into the tao event loop via the proxy so
    // they wake the loop and are handled on the webview's thread. `proxy` is
    // consumed by this handler; the event-loop run closure below doesn't need
    // it (it only touches `control_flow`).
    let connect_proxy = proxy.clone();
    let page_load_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let cmd = match event.id().as_ref() {
            menu_id::CONNECT_DEFAULT => Some(Command::ConnectDefault),
            // The prompt must run on the webview's thread, but the menu-event
            // handler already runs there — so instead of round-tripping through
            // the event loop, we hand it straight to `handle_command` as a
            // `ConnectServerPrompt` carrying the proxy the answer needs.
            menu_id::CONNECT_SERVER => Some(Command::ConnectServerPrompt(connect_proxy.clone())),
            menu_id::HOST => Some(Command::Host),
            menu_id::SHOW_ADDRESS => Some(Command::ShowAddress),
            menu_id::QUIT => Some(Command::Quit),
            _ => None,
        };
        if let Some(cmd) = cmd {
            // Sending can only fail if the loop has been destroyed; ignore.
            let _ = proxy.send_event(cmd);
        }
    }));

    let (app_menu, menu_controls) = build_menu(initial_host.is_some());

    let window = WindowBuilder::new()
        .with_title("Poker")
        .with_inner_size(LogicalSize::new(1280.0, 820.0))
        .build(&event_loop)
        .expect("failed to create window");

    // Attach the menu bar to the window (platform-specific) and keep it alive
    // for the window's lifetime — muda holds internal weak refs, so dropping
    // the root here would remove the menu.
    std::mem::forget(attach_menu(app_menu, &window));

    // ---- Webview (wry) ---------------------------------------------------
    // On Linux, tao's window is a GTK window whose raw handle isn't an Xlib
    // handle, so `build(&window)` fails with "window handle kind is not
    // supported". Attach the webview directly to the GTK container instead.
    // We use the same default vbox the menubar was packed into, so wry
    // pack_starts the webview below the menubar.
    //
    // The page-load hook lets us act *after* a navigation completes — needed
    // because `load_url` is async, so injecting an overlay (the invite address)
    // immediately after navigating would race the new page's DOM. On `Finished`
    // we just poke the event loop with `CheckPendingInvite`; the loop owns the
    // webview via `AppState` and does the actual injection. The hook can't
    // borrow `AppState` directly (the webview is built before the state that
    // owns it), so it carries only the proxy.
    let webview = WebViewBuilder::new()
        .with_url(&initial_url)
        .with_on_page_load_handler(move |event, _url| {
            if matches!(event, wry::PageLoadEvent::Finished) {
                let _ = page_load_proxy.send_event(Command::CheckPendingInvite);
            }
        });
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let container = window.default_vbox().expect("window has no default vbox");
        webview.build_gtk(container)
    };
    #[cfg(not(target_os = "linux"))]
    let webview = webview.build(&window);
    let webview = webview.unwrap_or_else(|e| panic!("failed to create webview: {e}"));

    // Shared mutable state held on the event-loop thread.
    let state = Rc::new(RefCell::new(AppState::new(
        webview,
        host_port,
        runtime_handle,
        initial_host,
        menu_controls,
    )));

    // If we booted straight into host mode, show the invite overlay on the
    // first page load (the eager 127.0.0.1 load) — the same UX as clicking
    // "Host a game" mid-session.
    if cli.host() {
        state.borrow_mut().request_invite();
    }

    // ---- Event loop ------------------------------------------------------
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(cmd) => handle_command(cmd, &state, control_flow),
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // Stop any embedded server before exiting.
                let mut s = state.borrow_mut();
                if let Some(handle) = s.host.take() {
                    handle.stop();
                }
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

/// Attach a muda [`Menu`] to a tao window. tao 0.36 dropped its built-in menu
/// API, so we go through muda's platform-specific initialisers. Returns the
/// menu so the caller can keep it alive (`std::mem::forget`) — muda holds only
/// weak refs internally.
#[allow(clippy::needless_pass_by_value, clippy::expect_used)] // consumes `menu` by design; attachment is unrecoverable startup setup
fn attach_menu(menu: Menu, window: &tao::window::Window) -> Menu {
    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::WindowExtUnix;
        // tao's GtkApplicationWindow is a GtkBin that already holds a GtkBox
        // (the webview's container). Passing that box as the container makes
        // muda pack_start the menubar at index 0 instead of `window.add()`,
        // which would fail (GtkBin can only hold one child).
        let vbox = window
            .default_vbox()
            .map(gtk::prelude::Cast::upcast_ref::<gtk::Container>);
        menu.init_for_gtk_window(window.gtk_window(), vbox)
            .expect("failed to attach menu to GTK window");
    }
    #[cfg(target_os = "windows")]
    unsafe {
        use tao::platform::windows::WindowExtWindows;
        menu.init_for_hwnd(window.hwnd())
            .expect("failed to attach menu to HWND");
    }
    #[cfg(target_os = "macos")]
    {
        menu.init_for_nsapp();
    }
    menu
}

/// Per-process state living on the event-loop thread.
struct AppState {
    webview: wry::WebView,
    /// The port the embedded server (when active) listens on.
    host_port: u16,
    /// Handle to the shared tokio runtime, used to spawn the embedded server.
    runtime_handle: tokio::runtime::Handle,
    /// The embedded server handle, present only while hosting.
    host: Option<Handle>,
    /// Live handles to the mode-dependent menu items, used to toggle their
    /// enabled state when hosting starts/stops.
    controls: MenuControls,
    /// One-shot flag set by the host-entering paths (`--host` startup,
    /// `Command::Host`). The page-load hook's `CheckPendingInvite` consumes it
    /// to inject the invite overlay once the new page's DOM is ready. Cleared
    /// by `Command::ConnectDefault` / `ConnectServer` so a friend entering an
    /// address (or returning to default) doesn't trigger it.
    pending_invite: bool,
}

/// Retained [`MenuItem`] handles so we can call `set_enabled` at runtime. muda
/// items are `Rc`-backed, so a clone shares the same underlying widget —
/// mutating it here flips the real on-screen item.
struct MenuControls {
    /// "Host a game" — disabled while hosting.
    host: MenuItem,
    /// "Connect to server…" — disabled while hosting (you're already pinned to
    /// `127.0.0.1`).
    connect_server: MenuItem,
    /// "Show my address…" — disabled unless hosting.
    show_address: MenuItem,
}

impl AppState {
    // Not actually `const`-constructible (`Handle` owns a JoinHandle); the
    // nursery lint is a false positive here.
    #[allow(clippy::missing_const_for_fn)]
    fn new(
        webview: wry::WebView,
        host_port: u16,
        runtime_handle: tokio::runtime::Handle,
        host: Option<Handle>,
        controls: MenuControls,
    ) -> Self {
        Self {
            webview,
            host_port,
            runtime_handle,
            host,
            controls,
            pending_invite: false,
        }
    }

    /// Mark that the invite overlay should be shown when the in-flight page
    /// load finishes. The page-load hook consumes this via
    /// [`Command::CheckPendingInvite`].
    const fn request_invite(&mut self) {
        self.pending_invite = true;
    }

    /// Flip the mode-dependent menu items to match `hosting`. Called on every
    /// hosting transition so the on-screen enabled state stays in sync with
    /// reality (the alternative — a one-time initial flag — drifts the moment
    /// the user starts or stops hosting).
    fn set_hosting_menu(&self, hosting: bool) {
        self.controls.host.set_enabled(!hosting);
        self.controls.connect_server.set_enabled(!hosting);
        self.controls.show_address.set_enabled(hosting);
    }
}

/// Build the native menu bar. `hosting` sets the initial enabled state of the
/// mode-dependent items. Returns the [`Menu`] plus retained [`MenuItem`]
/// handles the caller needs to toggle those items at runtime.
///
/// `expect` is permitted here: menu construction happens once at startup, and a
/// failure (muda misuse) has no recovery path before the window appears.
#[allow(clippy::expect_used)]
fn build_menu(hosting: bool) -> (Menu, MenuControls) {
    let poker_menu = Submenu::new("Poker", true);
    poker_menu
        .append_items(&[&MenuItem::with_id(
            MenuId::new(menu_id::QUIT),
            "Quit",
            true,
            None,
        )])
        .expect("failed to build Poker menu");

    let host_item = MenuItem::with_id(
        MenuId::new(menu_id::HOST),
        "Host a game",
        !hosting, // disabled if already hosting
        None,
    );
    let connect_server_item = MenuItem::with_id(
        MenuId::new(menu_id::CONNECT_SERVER),
        "Connect to server…",
        !hosting, // disabled while hosting (pinned to 127.0.0.1)
        None,
    );
    let show_address_item = MenuItem::with_id(
        MenuId::new(menu_id::SHOW_ADDRESS),
        "Show my address…",
        hosting, // disabled until we're hosting
        None,
    );

    let server_menu = Submenu::new("Server", true);
    server_menu
        .append_items(&[
            &MenuItem::with_id(
                MenuId::new(menu_id::CONNECT_DEFAULT),
                "Connect to default server",
                true,
                None,
            ),
            &connect_server_item,
            &PredefinedMenuItem::separator(),
            &host_item,
            &PredefinedMenuItem::separator(),
            &show_address_item,
        ])
        .expect("failed to build Server menu");

    let menu = Menu::new();
    menu.append_items(&[&poker_menu, &server_menu])
        .expect("failed to assemble menu bar");

    let controls = MenuControls {
        host: host_item,
        connect_server: connect_server_item,
        show_address: show_address_item,
    };

    (menu, controls)
}

/// Dispatch a menu command. Runs on the event-loop thread, so it may touch
/// the webview freely.
fn handle_command(cmd: Command, state: &Rc<RefCell<AppState>>, control_flow: &mut ControlFlow) {
    match cmd {
        Command::ConnectDefault => {
            let mut s = state.borrow_mut();
            // Connect is the universal "go home": stop the embedded server if
            // it's running, then point at the default public server. No invite
            // overlay — this isn't entering host mode.
            if let Some(handle) = s.host.take() {
                handle.stop();
            }
            s.pending_invite = false;
            let url = config::default_server_url();
            let _ = s.webview.load_url(&url);
            s.set_hosting_menu(false);
            tracing::info!("connecting to {url}");
        }
        Command::ConnectServerPrompt(proxy) => {
            // Run a JS prompt to collect the address. `evaluate_script_with_callback`
            // hands the serialized return back on the webview's own (main) thread.
            // The callback must be `Send`, so it can't capture the `Rc` state — it
            // forwards the answer as a `ConnectServer(url)` event through the proxy.
            let s = state.borrow();
            let js = "prompt('Enter the server address to connect to:', 'http://')";
            let result = s.webview.evaluate_script_with_callback(js, move |raw| {
                if let Some(url) = decode_prompt_result(&raw) {
                    tracing::info!("connect-to-server dialog returned {url}");
                    let _ = proxy.send_event(Command::ConnectServer(url));
                } else {
                    tracing::info!("connect-to-server dialog cancelled");
                }
            });
            if let Err(e) = result {
                tracing::error!("failed to run connect-to-server prompt: {e}");
            }
        }
        Command::ConnectServer(url) => {
            let mut s = state.borrow_mut();
            // A friend pasting an address has no local server; stop ours if up.
            // No invite overlay — they already know the address.
            if let Some(handle) = s.host.take() {
                handle.stop();
            }
            s.pending_invite = false;
            let _ = s.webview.load_url(&url);
            s.set_hosting_menu(false);
            tracing::info!("connecting to {url}");
        }
        Command::Host => {
            let mut s = state.borrow_mut();
            if s.host.is_none() {
                let port = s.host_port;
                s.host = Some(server::start(port, &s.runtime_handle));
                let url = format!("http://127.0.0.1:{port}/poker");
                let _ = s.webview.load_url(&url);
                s.set_hosting_menu(true);
                // Auto-open the invite overlay once the new page finishes
                // loading.
                s.request_invite();
                tracing::info!("hosting on port {port}");
            }
        }
        Command::ShowAddress => {
            let s = state.borrow();
            // The page is already loaded, so inject the overlay right away.
            let invite = lan_invite_url(s.host_port);
            let js = inject_invite_overlay(&invite);
            let _ = s.webview.evaluate_script(&js);
            tracing::info!("invite URL: {invite}");
        }
        Command::CheckPendingInvite => {
            // Fired by the page-load hook after each navigation finishes. Only
            // the host-entering paths set the flag, so a friend connecting to
            // our server (or returning to default) won't trigger an overlay.
            let mut s = state.borrow_mut();
            if s.pending_invite {
                s.pending_invite = false;
                let invite = lan_invite_url(s.host_port);
                let js = inject_invite_overlay(&invite);
                let _ = s.webview.evaluate_script(&js);
                tracing::info!("invite URL: {invite}");
            }
        }
        Command::Quit => {
            let mut s = state.borrow_mut();
            if let Some(handle) = s.host.take() {
                handle.stop();
            }
            *control_flow = ControlFlow::Exit;
        }
    }
}

/// Decode the JSON-serialized result of a JS `prompt()`.
///
/// wry's `evaluate_script_with_callback` serializes the return value via
/// `JSValue::to_json`: a string comes back as `"\"http://...\""` (with outer
/// quotes and any inner quotes escaped), and a cancelled prompt (which returns
/// `null`) comes back as `"null"`. We avoid pulling in `serde_json` for a single
/// value by handling these two shapes directly with `strip_*` (no slicing).
///
/// Returns `Some(url)` for a non-empty string, `None` for `null`/empty/cancel.
fn decode_prompt_result(raw: &str) -> Option<String> {
    // Trim the surrounding whitespace the serializer may emit.
    let trimmed = raw.trim();
    if trimmed == "null" || trimmed.is_empty() {
        return None;
    }
    // Unwrap the outer JSON string quotes, if present.
    let inner = trimmed
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(trimmed);
    if inner.is_empty() {
        return None;
    }
    // The serializer escapes embedded quotes/backslashes (`"` → `\"`, `\` →
    // `\\`). Reversing those without unescaping the whole string is wrong, but
    // for URLs neither character appears in practice. We do a minimal, correct
    // unescape pass so a user who pastes either (or a URL containing `\"`)
    // isn't surprised.
    let mut url = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // Consume the escaped char verbatim; if the input ends with a lone
            // backslash, emit nothing (the prompt can't produce that shape, so
            // this is purely defensive).
            if let Some(escaped) = chars.next() {
                url.push(escaped);
            }
        } else {
            url.push(c);
        }
    }
    if url.is_empty() { None } else { Some(url) }
}

/// Build the JavaScript that shows a self-contained, selectable invite overlay.
///
/// This replaces the previous `alert()`-based approach: native webview alerts
/// have a fixed (and odd, e.g. "JavaScript - http://…") title and don't let the
/// user select or copy the address. The injected overlay is plain DOM, so the
/// address lives in a read-only `<input>` the user can select and copy, and it
/// dismisses itself on Escape / backdrop click / Close button — no Rust
/// round-trip.
///
/// Styling leans on the poker page's CSS variables (with hard-coded fallbacks
/// for safety) so the card matches the app rather than looking foreign. The
/// overlay is idempotent: a re-injection removes any existing instance first.
fn inject_invite_overlay(invite_url: &str) -> String {
    // Embed the URL as a JS string literal. Invite URLs are plain
    // `http://host:port/poker`, so the only characters that could need
    // escaping are backslash and double-quote — neither appears in practice,
    // but we escape defensively rather than pulling in a JSON dep for one
    // value. The result is wrapped in double quotes.
    let escaped: String = invite_url
        .chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            '"' => "\\\"".to_string(),
            _ => c.to_string(),
        })
        .collect();
    let url_literal = format!("\"{escaped}\"");
    format!(
        r"(function () {{
  var old = document.getElementById('pokerdesktop-invite');
  if (old) {{ old.remove(); }}
  var backdrop = document.createElement('div');
  backdrop.id = 'pokerdesktop-invite';
  backdrop.style.cssText =
    'position:fixed;inset:0;background:rgba(0,0,0,0.55);display:flex;' +
    'align-items:center;justify-content:center;z-index:999999;' +
    'font-family:system-ui,-apple-system,sans-serif;';
  var card = document.createElement('div');
  card.style.cssText =
    'background:var(--color-elevated,#2D2018);' +
    'color:var(--color-foreground,#F9F7F5);border-radius:12px;padding:24px;' +
    'max-width:460px;width:90%;box-shadow:0 8px 32px rgba(0,0,0,0.45);' +
    'box-sizing:border-box;';
  var heading = document.createElement('h3');
  heading.textContent = 'Share your address';
  heading.style.cssText =
    'margin:0 0 8px;color:var(--color-accent,#FBDB93);font-size:18px;';
  var blurb = document.createElement('p');
  blurb.textContent =
    'Friends in the LAN open this in their browser, or via “Connect to server…”:';
  blurb.style.cssText = 'margin:0 0 14px;opacity:0.85;font-size:14px;';
  var field = document.createElement('input');
  field.type = 'text';
  field.readOnly = true;
  field.value = {url_literal};
  field.style.cssText =
    'width:100%;box-sizing:border-box;padding:9px 11px;border-radius:8px;' +
    'border:1px solid var(--color-surface,#641B2E);' +
    'background:var(--color-base,#1A130D);' +
    'color:var(--color-foreground,#F9F7F5);font-family:monospace;' +
    'font-size:14px;outline:none;';
  // Select-on-focus so Ctrl+C copies the address immediately.
  field.addEventListener('focus', function () {{ field.select(); }});
  var close = document.createElement('button');
  close.type = 'button';
  close.textContent = 'Close';
  close.style.cssText =
    'margin-top:18px;padding:8px 18px;border:none;border-radius:8px;cursor:pointer;' +
    'background:var(--color-primary,#BE5B50);color:var(--color-foreground,#F9F7F5);' +
    'font-size:14px;font-weight:600;';
  card.appendChild(heading);
  card.appendChild(blurb);
  card.appendChild(field);
  card.appendChild(close);
  backdrop.appendChild(card);
  document.body.appendChild(backdrop);
  // Focus (→ select) after the browser lays the field out.
  setTimeout(function () {{ field.focus(); }}, 0);
  function dismiss() {{
    backdrop.remove();
    document.removeEventListener('keydown', onKey);
  }}
  close.addEventListener('click', dismiss);
  backdrop.addEventListener('click', function (e) {{
    if (e.target === backdrop) {{ dismiss(); }}
  }});
  function onKey(e) {{ if (e.key === 'Escape') {{ dismiss(); }} }}
  document.addEventListener('keydown', onKey);
}})();"
    )
}
