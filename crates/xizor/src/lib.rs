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
    let visor_enabled = config.visor.enabled;
    // Only actually asks the OS for a transparent (alpha-composited) window
    // when the background is configured to be anything less than fully
    // solid - a transparent window engages a different, costlier
    // compositing path than an ordinary opaque one regardless of what gets
    // painted into it, so this is skipped entirely (not just harmlessly
    // requested) for the common case of leaving background alpha at its
    // default `1.0`.
    //
    // The default `tiny-skia` build can't do this on macOS at all - see the
    // `tiny-skia`/`wgpu` feature comment in this crate's `Cargo.toml`. The
    // `wgpu` binary (`xizor-gpu`) was assumed to be the fix, but that's
    // unconfirmed on macOS - it stayed opaque there too in testing, which
    // could mean `iced_wgpu` isn't landing on the composite alpha mode this
    // comment expects, or something upstream of that. `env_logger::init()`
    // below turns on `iced_wgpu`'s own `log::info!`s (which were previously
    // silent - nothing in this crate ever installed a logger) so a run with
    // `RUST_LOG=info` shows exactly which adapter/format/alpha mode got
    // selected, to actually pin this down instead of guessing further from
    // source alone.
    let transparent = config.alpha.background < 1.0;

    // See the long comment on `transparent` above - temporary, for
    // diagnosing the macOS `xizor-gpu` transparency issue. Forces a
    // default of `info` rather than relying on `RUST_LOG` alone (still
    // overridable by it) - the previous plain `try_init()` produced no
    // output at all in testing, which is itself suspicious (`iced_wgpu`
    // unconditionally logs its `Settings` as the very first thing
    // `Compositor::request` does, so *something* should have shown up if
    // `RUST_LOG=info` had actually reached the process). `try_init` (not
    // `init`) so this can't panic if something else already installed a
    // logger.
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .try_init();
    eprintln!("xizor: logger installed, max_level={:?}", log::max_level());

    // iced 0.14 moved the boot/init closure to `application`'s first
    // argument (previously supplied last, via `.run_with(...)`) and moved
    // the title out to its own `.title(...)` call. Unlike `run_with`, this
    // closure must be `Fn` (not just `FnOnce`) - `config` is cloned on each
    // call rather than moved out, even though boot only ever runs once in
    // practice.
    iced::application(move || XizorApp::new(config.clone()), XizorApp::update, XizorApp::view)
        .title("xizor")
        .window_size(iced::Size::new(900.0, 600.0))
        // No native titlebar/frame while the visor is enabled - winit's
        // `decorations` setting is what each OS's window server keys off
        // of, so this one call covers Windows/macOS/Linux alike. The app
        // has no replacement titlebar (drag region, custom min/max/close)
        // yet, so in that mode the window can only be moved/resized/closed
        // via OS window-manager shortcuts, not by dragging or clicking
        // anything xizor draws itself. Visor mode is still a bit buggy on
        // some systems (see `xizor_config::VisorConfig`), so with it off
        // this instead behaves like an ordinary window, default OS
        // titlebar/frame included.
        .decorations(!visor_enabled)
        .transparent(transparent)
        // The visor should render above whatever application currently has
        // focus while it's shown - it wouldn't be much of a drop-down
        // console otherwise. An ordinary (non-visor) window has no reason
        // to float above everything else.
        .level(if visor_enabled {
            iced::window::Level::AlwaysOnTop
        } else {
            iced::window::Level::Normal
        })
        .subscription(XizorApp::subscription)
        .theme(XizorApp::theme)
        // Scales the window's base background_color (what the renderer
        // clears the whole surface to before drawing anything) by
        // `config.alpha.background` - see `XizorApp::style`. Without this,
        // `.transparent(transparent)` above requests OS-level window
        // transparency but the window still gets cleared fully opaque, so
        // nothing actually shows through.
        .style(XizorApp::style)
        // Don't let iced auto-exit on the window's close button - the app
        // needs a chance to run a last-ditch draft flush first (see
        // `Message::WindowCloseRequested` in `app.rs`), then closes the
        // window itself once that's done.
        .exit_on_close_request(false)
        .run()
}
