// TEMPORARILY DISABLED for Windows debugging (syntax highlighting not
// showing up) - re-enable once resolved so release builds hide the console
// again: #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod hotkey;
mod session;
mod visor;

use app::XizorApp;

/// Shared entry point for both the `xizor` (tiny-skia) and `xizor-gpu`
/// (wgpu) binaries - each compiles in exactly one iced compositor backend
/// (see the `xizor` crate's `Cargo.toml`), so there's no runtime backend
/// choice to make here, unlike when both were compiled into one binary.
pub fn run() -> iced::Result {
    let config = xizor_config::load();

    // iced 0.14 moved the boot/init closure to `application`'s first
    // argument (previously supplied last, via `.run_with(...)`) and moved
    // the title out to its own `.title(...)` call. Unlike `run_with`, this
    // closure must be `Fn` (not just `FnOnce`) - `config` is cloned on each
    // call rather than moved out, even though boot only ever runs once in
    // practice.
    iced::application(move || XizorApp::new(config.clone()), XizorApp::update, XizorApp::view)
        .title("xizor")
        .window_size(iced::Size::new(900.0, 600.0))
        // No native titlebar/frame on any platform - winit's `decorations`
        // setting is what each OS's window server keys off of, so this one
        // call covers Windows/macOS/Linux alike. The app has no
        // replacement titlebar (drag region, custom min/max/close) yet, so
        // for now the window can only be moved/resized/closed via OS
        // window-manager shortcuts, not by dragging or clicking anything
        // xizor draws itself.
        .decorations(false)
        // The visor should render above whatever application currently has
        // focus while it's shown - it wouldn't be much of a drop-down
        // console otherwise.
        .level(iced::window::Level::AlwaysOnTop)
        .subscription(XizorApp::subscription)
        .theme(XizorApp::theme)
        // Don't let iced auto-exit on the window's close button - the app
        // needs a chance to run a last-ditch draft flush first (see
        // `Message::WindowCloseRequested` in `app.rs`), then closes the
        // window itself once that's done.
        .exit_on_close_request(false)
        .run()
}
