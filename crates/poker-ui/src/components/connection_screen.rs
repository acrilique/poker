//! Connection screen — name, server address, room ID, create/join buttons.

use dioxus::prelude::*;
use poker_core::protocol::{BlindConfig, validate_room_id};

use crate::UiMessage;

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
    let mut name = use_signal(|| "Player".to_string());
    let mut server_url = use_signal(move || default_server.clone());
    let mut room_id = use_signal(String::new);
    let mut validation_error = use_signal(String::new);
    let mut blind_interval_mins = use_signal(String::new);
    let mut blind_increase_pct = use_signal(String::new);
    let mut starting_bbs_input = use_signal(|| "50".to_string());
    let coroutine = use_coroutine_handle::<UiMessage>();

    let mut on_submit = move |create: bool| {
        let n = name.read().clone();
        let s = server_url.read().clone();
        let r = room_id.read().clone();

        // Client-side validation
        if n.trim().is_empty() {
            validation_error.set("Player name cannot be empty".to_string());
            return;
        }
        if s.trim().is_empty() {
            validation_error.set("Server address cannot be empty".to_string());
            return;
        }
        if let Err(e) = validate_room_id(&r) {
            validation_error.set(e);
            return;
        }

        // Parse blind increase settings (only relevant when creating).
        let blind_config = if create {
            let interval_secs = blind_interval_mins
                .read()
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
                * 60;
            let increase_percent = blind_increase_pct.read().trim().parse::<u32>().unwrap_or(0);
            BlindConfig {
                interval_secs,
                increase_percent,
            }
        } else {
            BlindConfig::default()
        };

        let starting_bbs = starting_bbs_input
            .read()
            .trim()
            .parse::<u32>()
            .unwrap_or(50)
            .max(1);

        validation_error.set(String::new());
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

    rsx! {
        div { class: "min-h-screen flex items-center justify-center p-4 bg-base",
            div { class: "bg-surface w-full max-w-sm rounded-2xl shadow-2xl p-6 flex flex-col gap-4 sm:p-8 sm:gap-5 conn-card",
                h1 { class: "text-3xl font-bold text-center text-accent", "Texas hold 'em" }
                p { class: "text-xs text-foreground/50 text-right -mt-2", "by acrilique" }

                // Form fields — single column normally, two columns in landscape
                div { class: "flex flex-col gap-4 conn-fields",

                    // Name input
                    div { class: "flex flex-col gap-1",
                        label { class: "text-sm text-foreground/60", "Player name" }
                        input {
                            class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent",
                            r#type: "text",
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                        }
                    }

                    // Server address input
                    div { class: "flex flex-col gap-1",
                        label { class: "text-sm text-foreground/60", "Server address" }
                        input {
                            class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent",
                            r#type: "text",
                            value: "{server_url}",
                            oninput: move |e| server_url.set(e.value()),
                        }
                    }

                    // Room ID input
                    div { class: "flex flex-col gap-1",
                        label { class: "text-sm text-foreground/60", "Room ID" }
                        input {
                            class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent",
                            r#type: "text",
                            placeholder: "e.g. myroom42",
                            value: "{room_id}",
                            oninput: move |e| room_id.set(e.value()),
                        }
                        p { class: "text-xs text-foreground/40", "Alphanumeric, up to 19 characters" }
                    }

                    // Starting big blinds
                    div { class: "flex flex-col gap-1",
                        label { class: "text-sm text-foreground/60", "Starting stack (host only)" }
                        div { class: "flex items-center gap-2",
                            input {
                                class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent w-24",
                                r#type: "number",
                                min: "1",
                                value: "{starting_bbs_input}",
                                oninput: move |e| starting_bbs_input.set(e.value()),
                            }
                            span { class: "text-sm text-foreground/60", "big blinds" }
                        }
                    }

                    // Blind increase settings (applies when creating a room)
                    div { class: "flex flex-col gap-2",
                        label { class: "text-sm text-foreground/60", "Blind increases (host only)" }
                        div { class: "flex gap-2",
                            div { class: "flex-1 flex flex-col gap-1",
                                input {
                                    class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent w-full",
                                    r#type: "number",
                                    min: "0",
                                    placeholder: "Minutes",
                                    value: "{blind_interval_mins}",
                                    oninput: move |e| blind_interval_mins.set(e.value()),
                                }
                                p { class: "text-xs text-foreground/40", "Interval (min)" }
                            }
                            div { class: "flex-1 flex flex-col gap-1",
                                input {
                                    class: "bg-muted rounded-lg px-4 py-2 text-foreground outline-none focus:ring-2 focus:ring-accent w-full",
                                    r#type: "number",
                                    min: "0",
                                    placeholder: "Percent",
                                    value: "{blind_increase_pct}",
                                    oninput: move |e| blind_increase_pct.set(e.value()),
                                }
                                p { class: "text-xs text-foreground/40", "Increase (%)" }
                            }
                        }
                        p { class: "text-xs text-foreground/40", "Leave empty or 0 to keep blinds fixed" }
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
                        class: "flex-1 bg-primary hover:bg-primary-light text-foreground font-semibold rounded-lg py-2 transition",
                        onclick: move |_| on_submit(true),
                        "Create Room"
                    }
                    button {
                        class: "flex-1 bg-muted hover:bg-muted-light text-foreground font-semibold rounded-lg py-2 transition",
                        onclick: move |_| on_submit(false),
                        "Join Room"
                    }
                }
            }
        }
    }
}
