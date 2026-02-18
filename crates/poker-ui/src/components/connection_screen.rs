//! Connection screen — name, server address, room ID, create/join buttons.

use dioxus::prelude::*;
use poker_core::protocol::validate_room_id;

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
    let mut room_id = use_signal(|| String::new());
    let mut validation_error = use_signal(|| String::new());
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

        validation_error.set(String::new());
        coroutine.send(UiMessage::Connect {
            name: n,
            server_url: s,
            room_id: r,
            create,
        });
    };

    let err = error.read().clone();
    let val_err = validation_error.read().clone();

    rsx! {
        div { class: "flex items-center justify-center h-screen",
            div { class: "bg-gray-800 rounded-2xl shadow-2xl p-10 w-96 flex flex-col gap-6",
                h1 { class: "text-3xl font-bold text-center text-emerald-400", "♠ Poker ♥" }

                // Name input
                div { class: "flex flex-col gap-1",
                    label { class: "text-sm text-gray-400", "Player name" }
                    input {
                        class: "bg-gray-700 rounded-lg px-4 py-2 text-white outline-none focus:ring-2 focus:ring-emerald-500",
                        r#type: "text",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }

                // Server address input
                div { class: "flex flex-col gap-1",
                    label { class: "text-sm text-gray-400", "Server address" }
                    input {
                        class: "bg-gray-700 rounded-lg px-4 py-2 text-white outline-none focus:ring-2 focus:ring-emerald-500",
                        r#type: "text",
                        value: "{server_url}",
                        oninput: move |e| server_url.set(e.value()),
                    }
                }

                // Room ID input
                div { class: "flex flex-col gap-1",
                    label { class: "text-sm text-gray-400", "Room ID" }
                    input {
                        class: "bg-gray-700 rounded-lg px-4 py-2 text-white outline-none focus:ring-2 focus:ring-emerald-500",
                        r#type: "text",
                        placeholder: "e.g. myroom42",
                        value: "{room_id}",
                        oninput: move |e| room_id.set(e.value()),
                    }
                    p { class: "text-xs text-gray-500", "Alphanumeric, up to 19 characters" }
                }

                // Validation error
                if !val_err.is_empty() {
                    p { class: "text-red-400 text-sm text-center", "{val_err}" }
                }

                // Server error
                if !err.is_empty() {
                    p { class: "text-red-400 text-sm text-center", "{err}" }
                }

                // Buttons
                div { class: "flex gap-3",
                    button {
                        class: "flex-1 bg-emerald-600 hover:bg-emerald-500 text-white font-semibold rounded-lg py-2 transition",
                        onclick: move |_| on_submit(true),
                        "Create Room"
                    }
                    button {
                        class: "flex-1 bg-blue-600 hover:bg-blue-500 text-white font-semibold rounded-lg py-2 transition",
                        onclick: move |_| on_submit(false),
                        "Join Room"
                    }
                }
            }
        }
    }
}
