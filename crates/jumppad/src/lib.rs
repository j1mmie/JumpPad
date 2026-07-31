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

/// Why `[alpha] background < 1.0` will be ignored on this build, or `None`
/// when the window can really be translucent. See AGENTS.md for the evidence
/// behind each case - both are hard platform limits, not app bugs, and the
/// other binary is the fix in both directions.
const OPAQUE_WINDOW_REASON: Option<&str> = if cfg!(all(
    target_os = "macos",
    feature = "tiny-skia"
)) {
    // softbuffer's CoreGraphics backend hardcodes `NoneSkipFirst`, discarding
    // the alpha channel tiny-skia painted.
    Some("[alpha] background is ignored by this binary on macOS - the software renderer's presentation path drops the alpha channel. Run jumppad-gpu for a translucent window.")
} else if cfg!(all(target_os = "windows", feature = "wgpu")) {
    // wgpu's DX12 swapchain built from a raw HWND reports
    // `composite_alpha_modes: [Opaque]`, so alpha never reaches the DWM.
    Some("[alpha] background is ignored by this binary on Windows - wgpu's DX12 swapchain presents an opaque window. Run jumppad for a translucent window.")
} else {
    None
};

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

    // Neither backend can do transparency on every platform, and the failure
    // is silent - the window just comes up solid, which reads as a rendering
    // bug rather than a wrong-binary problem. Say so up front instead.
    if transparent {
        if let Some(reason) = OPAQUE_WINDOW_REASON {
            eprintln!("jumppad: {reason}");
        }
    }

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
