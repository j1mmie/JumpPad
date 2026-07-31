// TEMPORARILY DISABLED for Windows debugging (syntax highlighting not
// showing up) - re-enable once resolved so release builds hide the console
// again: #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod find;
mod hotkey;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
mod session;
mod visor;

use app::JumpPadApp;

/// Shared entry point for both the `jumppad` (tiny-skia) and `jumppad-gpu` (wgpu) binaries.
pub fn run() -> iced::Result {
    let config = jumppad_config::load();
    let visor_enabled = config.visor.enabled;
    // Visor mode wins: a drop-down visor is undecorated by definition.
    let decorations = config.window.decorations && !visor_enabled;
    // Skipped entirely (not just requested-then-ignored) at the default
    // alpha of `1.0`, since transparent windows use a costlier compositing path.
    let transparent = config.alpha.background < 1.0;

    // Defaults to `info` level (still overridable via `RUST_LOG`) so
    // `iced_wgpu`'s own adapter/format/alpha-mode logging is visible.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    // `config` is cloned per call since the boot closure must be `Fn`, not just `FnOnce`.
    iced::application(move || JumpPadApp::new(config.clone()), JumpPadApp::update, JumpPadApp::view)
        .title("JumpPad")
        .window_size(iced::Size::new(900.0, 600.0))
        .decorations(decorations)
        .transparent(transparent)
        // iced defaults this on, but its MSAA only ever applies to triangle
        // primitives - meshes, canvases, gradient quads - and this app draws
        // none. Quads and text are always `count: 1` regardless. So it buys
        // nothing visually and costs pipelines plus a 4x-sampled render target.
        .antialiasing(false)
        // The visor floats above whatever else has focus; an ordinary window doesn't.
        .level(if visor_enabled {
            iced::window::Level::AlwaysOnTop
        } else {
            iced::window::Level::Normal
        })
        .subscription(JumpPadApp::subscription)
        .theme(JumpPadApp::theme)
        .style(JumpPadApp::style)
        // Runs a last-ditch draft flush before actually closing the window.
        .exit_on_close_request(false)
        .run()
}
