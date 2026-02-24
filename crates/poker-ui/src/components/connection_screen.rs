//! Connection screen — name, server address, room ID, create/join buttons.

use dioxus::prelude::*;
use poker_core::protocol::{BlindConfig, validate_room_id};

use crate::UiMessage;

/// Maximum allowed length for a player name.
const MAX_NAME_LEN: usize = 16;

/// Which submit action is currently in-flight, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectingAction {
    Create,
    Join,
}

/// Props for the connection screen.
///
/// `default_server` pre-fills the server address field. For web builds this
/// is typically derived from the page origin; for desktop it defaults to
/// `localhost:8080`.
#[component]
pub fn ConnectionScreen(
    error: Signal<String>,
    #[props(default = "localhost:8080".to_string())] default_server: String,
) -> Element {
    let mut name = use_signal(String::new);
    let mut server_url = use_signal(|| default_server.clone());
    let mut room_id = use_signal(String::new);
    let mut validation_error = use_signal(String::new);
    let mut blind_interval_mins = use_signal(String::new);
    let mut blind_increase_pct = use_signal(String::new);
    let mut starting_bbs_input = use_signal(|| "100".to_string());
    let mut show_server = use_signal(|| false);
    let mut show_host_settings = use_signal(|| false);
    let mut connecting: Signal<Option<ConnectingAction>> = use_signal(|| None);
    let coroutine = use_coroutine_handle::<UiMessage>();

    // Reset connecting state when a server error arrives.
    use_effect(move || {
        if !error.read().is_empty() {
            connecting.set(None);
        }
    });

    let mut on_submit = move |create: bool| {
        if connecting.read().is_some() {
            return;
        }

        let n = name.read().trim().to_string();
        let s = server_url.read().trim().to_string();
        let r = room_id.read().clone();

        // Client-side validation
        if n.is_empty() {
            validation_error.set("Player name cannot be empty".to_string());
            return;
        }
        if n.len() > MAX_NAME_LEN {
            validation_error.set(format!(
                "Player name must be at most {MAX_NAME_LEN} characters"
            ));
            return;
        }
        if s.is_empty() {
            validation_error.set("Server address cannot be empty".to_string());
            return;
        }
        if let Err(e) = validate_room_id(&r) {
            validation_error.set(e);
            return;
        }

        // Parse blind increase settings (only relevant when creating).
        let blind_config = if create {
            let interval_raw = blind_interval_mins.read().trim().to_string();
            let increase_raw = blind_increase_pct.read().trim().to_string();

            let interval_secs = if interval_raw.is_empty() {
                0u64
            } else {
                match interval_raw.parse::<u64>() {
                    Ok(v) => v * 60,
                    Err(_) => {
                        validation_error.set("Blind interval must be a valid number".to_string());
                        return;
                    }
                }
            };

            let increase_percent = if increase_raw.is_empty() {
                0u32
            } else {
                match increase_raw.parse::<u32>() {
                    Ok(v) => v,
                    Err(_) => {
                        validation_error.set("Blind rise must be a valid number".to_string());
                        return;
                    }
                }
            };

            BlindConfig {
                interval_secs,
                increase_percent,
            }
        } else {
            BlindConfig::default()
        };

        let starting_bbs = {
            let raw = starting_bbs_input.read().trim().to_string();
            if raw.is_empty() {
                100u32
            } else {
                match raw.parse::<u32>() {
                    Ok(v) => v.max(1),
                    Err(_) => {
                        validation_error.set("Initial stack must be a valid number".to_string());
                        return;
                    }
                }
            }
        };

        validation_error.set(String::new());
        connecting.set(Some(if create {
            ConnectingAction::Create
        } else {
            ConnectingAction::Join
        }));
        coroutine.send(UiMessage::Connect {
            name: n,
            server_url: s,
            room_id: r,
            create,
            blind_config,
            starting_bbs,
        });
    };

    let err = error.read().clone();
    let val_err = validation_error.read().clone();
    let connecting_action = *connecting.read();
    let is_connecting = connecting_action.is_some();

    rsx! {
        div {
            class: "min-h-screen flex items-center justify-center p-4 bg-base",
            // Enter key → Create if host settings are open, otherwise Join
            onkeydown: move |e| {
                if e.key() == Key::Enter {
                    on_submit(*show_host_settings.read());
                }
            },
            div { class: "bg-surface w-full max-w-sm rounded-2xl shadow-2xl p-6 flex flex-col gap-4 sm:p-8 sm:gap-5 conn-card",
                div { class: "conn-header",
                    h1 { class: "text-3xl font-bold text-center text-accent", "Texas hold 'em" }
                    p { class: "text-xs text-foreground/50 text-right", "by acrilique" }
                }

                // Form fields — single column normally, two columns in landscape
                div { class: "flex flex-col gap-4 conn-fields",

                    // Name input
                    div { class: "flex flex-col gap-1",
                        label { class: "text-sm text-foreground/60", "Player name" }
                        input {
                            class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent",
                            r#type: "text",
                            maxlength: "{MAX_NAME_LEN}",
                            placeholder: "Enter your name",
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }

                    // Room ID input
                    div { class: "flex flex-col gap-1",
                        label { class: "text-sm text-foreground/60", "Room ID" }
                        input {
                            class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent",
                            r#type: "text",
                            placeholder: "room42",
                            value: "{room_id}",
                            oninput: move |e| room_id.set(e.value()),
                        }
                        p { class: "text-xs text-foreground/40", "Alphanumeric, up to 19 characters" }
                    }

                    // Server address (collapsed by default)
                    div { class: "flex flex-col gap-1",
                        button {
                            class: "text-sm text-foreground/60 flex items-center gap-1 hover:text-foreground/80 transition",
                            r#type: "button",
                            onclick: move |_| show_server.toggle(),
                            "Server address"
                            span {
                                class: if *show_server.read() {
                                    "text-xs transition-transform duration-150 rotate-180"
                                } else {
                                    "text-xs transition-transform duration-150"
                                },
                                "▾"
                            }
                        }
                        div {
                            class: if *show_server.read() {
                                "collapsible collapsible-open"
                            } else {
                                "collapsible"
                            },
                            div {
                                input {
                                    class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent w-full",
                                    r#type: "text",
                                    value: "{server_url}",
                                    oninput: move |e| server_url.set(e.value()),
                                }
                            }
                        }
                    }

                    // Host settings (collapsed by default)
                    div { class: "flex flex-col gap-2",
                        button {
                            class: "text-sm text-foreground/60 flex items-center gap-1 hover:text-foreground/80 transition",
                            r#type: "button",
                            onclick: move |_| show_host_settings.toggle(),
                            "Host settings"
                            span {
                                class: if *show_host_settings.read() {
                                    "text-xs transition-transform duration-150 rotate-180"
                                } else {
                                    "text-xs transition-transform duration-150"
                                },
                                "▾"
                            }
                        }
                        div {
                            class: if *show_host_settings.read() {
                                "collapsible collapsible-open"
                            } else {
                                "collapsible"
                            },
                            div { class: "flex flex-col gap-2 host-inputs",
                                // Starting big blinds
                                div { class: "flex-1 flex flex-col gap-1",
                                    label { class: "text-sm text-foreground/60", "Initial stack" }
                                    input {
                                        class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent w-full",
                                        r#type: "number",
                                        min: "1",
                                        value: "{starting_bbs_input}",
                                        oninput: move |e| starting_bbs_input.set(e.value()),
                                    }
                                    p { class: "text-xs text-foreground/40", "BBs per player" }
                                }

                                // Blind interval
                                div { class: "flex-1 flex flex-col gap-1",
                                    label { class: "text-sm text-foreground/60", "Blind interval" }
                                    input {
                                        class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent w-full",
                                        r#type: "number",
                                        min: "0",
                                        placeholder: "0",
                                        value: "{blind_interval_mins}",
                                        oninput: move |e| blind_interval_mins.set(e.value()),
                                    }
                                    p { class: "text-xs text-foreground/40", "Minutes" }
                                }

                                // Blind increase %
                                div { class: "flex-1 flex flex-col gap-1",
                                    label { class: "text-sm text-foreground/60", "Blind rise" }
                                    input {
                                        class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent w-full",
                                        r#type: "number",
                                        min: "0",
                                        placeholder: "0",
                                        value: "{blind_increase_pct}",
                                        oninput: move |e| blind_increase_pct.set(e.value()),
                                    }
                                    p { class: "text-xs text-foreground/40", "Percent" }
                                }
                            }
                        }
                    }
                }

                // Validation error
                if !val_err.is_empty() {
                    p { class: "text-primary text-sm text-center", "{val_err}" }
                }

                // Server error
                if !err.is_empty() {
                    p { class: "text-primary text-sm text-center", "{err}" }
                }

                // Buttons
                div { class: "flex gap-3",
                    button {
                        class: if is_connecting {
                            "flex-1 bg-primary/50 text-foreground/50 font-semibold rounded-lg py-2 cursor-not-allowed"
                        } else {
                            "flex-1 bg-primary hover:bg-primary-light text-foreground font-semibold rounded-lg py-2 transition"
                        },
                        disabled: is_connecting,
                        onclick: move |_| on_submit(true),
                        if connecting_action == Some(ConnectingAction::Create) { "Connecting…" } else { "Create Room" }
                    }
                    button {
                        class: if is_connecting {
                            "flex-1 bg-muted/50 text-foreground/50 font-semibold rounded-lg py-2 cursor-not-allowed"
                        } else {
                            "flex-1 bg-muted hover:bg-muted-light text-foreground font-semibold rounded-lg py-2 transition"
                        },
                        disabled: is_connecting,
                        onclick: move |_| on_submit(false),
                        if connecting_action == Some(ConnectingAction::Join) { "Connecting…" } else { "Join Room" }
                    }
                }
            }
        }
    }
}
