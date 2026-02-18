mod app;

fn main() {
    dioxus::LaunchBuilder::new()
        .with_cfg(
            dioxus::desktop::Config::new()
                .with_window(
                    dioxus::desktop::tao::window::WindowBuilder::new().with_title("pokerot"),
                )
                .with_menu(None),
        )
        .launch(app::App);
}
