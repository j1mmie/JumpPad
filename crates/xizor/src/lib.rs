// TEMPORARILY DISABLED for Windows debugging (syntax highlighting not
// showing up) - re-enable once resolved so release builds hide the console
// again: #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use app::XizorApp;

/// Shared entry point for both the `xizor` (tiny-skia) and `xizor-gpu`
/// (wgpu) binaries - each compiles in exactly one iced compositor backend
/// (see the `xizor` crate's `Cargo.toml`), so there's no runtime backend
/// choice to make here, unlike when both were compiled into one binary.
pub fn run() -> iced::Result {
    let config = xizor_config::load();

    iced::application("xizor", XizorApp::update, XizorApp::view)
        .window_size(iced::Size::new(900.0, 600.0))
        .subscription(XizorApp::subscription)
        .theme(XizorApp::theme)
        .run_with(move || XizorApp::new(config))
}
