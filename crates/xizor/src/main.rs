// TEMPORARILY DISABLED for Windows debugging (syntax highlighting not
// showing up) - re-enable once resolved so release builds hide the console
// again: #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use app::XizorApp;

fn main() -> iced::Result {
    let config = xizor_config::load();

    // Must happen before the iced runtime starts: the compositor backend is
    // chosen the first time a window is created, reading `ICED_BACKEND`
    // (iced has no `Settings` field for this) - `main` is the last point
    // that's guaranteed to run before that.
    if let Some(backend) = config.renderer.iced_backend_env_value() {
        std::env::set_var("ICED_BACKEND", backend);
    }

    iced::application("xizor", XizorApp::update, XizorApp::view)
        .window_size(iced::Size::new(900.0, 600.0))
        .subscription(XizorApp::subscription)
        .theme(XizorApp::theme)
        .run_with(move || XizorApp::new(config))
}
