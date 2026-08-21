// TEMPORARILY DISABLED for Windows debugging (syntax highlighting not
// showing up) - re-enable once resolved so release builds hide the console
// again: #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod docwatch;
mod find;
mod hotkey;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
mod reload;
mod session;
mod visor;
mod window;
#[cfg(target_os = "windows")]
pub(crate) mod windows;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use app::JumpPadApp;

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Help,
    Version,
    /// Files to open at startup, in the order they were named.
    Open(Vec<PathBuf>),
}

/// Everything that isn't `--help`/`--version` is a path. Deliberately not a
/// flag parser: the two flags packagers expect, and nothing that would grow
/// into a CLI surface this editor doesn't want.
fn parse_args(args: impl Iterator<Item = OsString>) -> Invocation {
    let mut paths = Vec::new();
    for arg in args {
        match arg.to_str() {
            Some("--help" | "-h") => return Invocation::Help,
            Some("--version" | "-V") => return Invocation::Version,
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    Invocation::Open(paths)
}

/// The name this binary was invoked as - `run()` is shared by both binaries,
/// and the lib crate can't see `CARGO_BIN_NAME`.
fn program_name(argv0: Option<&OsString>) -> String {
    argv0
        .map(Path::new)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "jumppad".to_string())
}

/// Why `[alpha] background < 1.0` will be ignored on this build, or `None`
/// when the window can really be translucent. One known case, evidenced in
/// AGENTS.md: a hard platform limit, not an app bug, with the other binary
/// as the fix.
const OPAQUE_WINDOW_REASON: Option<&str> = if cfg!(all(
    target_os = "macos",
    feature = "tiny-skia"
)) {
    // softbuffer's CoreGraphics backend hardcodes `NoneSkipFirst`,
    // discarding the alpha channel tiny-skia painted.
    Some(
        "[alpha] background is ignored by this binary on macOS - the software renderer's presentation path drops the alpha channel. Run jumppad-gpu for a translucent window.",
    )
} else {
    None
};

/// Shared entry point for both the `jumppad` (tiny-skia) and `jumppad-gpu` (wgpu) binaries.
pub fn run() -> iced::Result {
    let mut argv = std::env::args_os();
    let program = program_name(argv.next().as_ref());
    let paths = match parse_args(argv) {
        Invocation::Help => {
            println!(
                "\
{program} - a lightweight plaintext editor

Usage: {program} [FILE]...

Opens each FILE in its own tab. A FILE that doesn't exist yet opens as an
empty tab saved to that path on the first save.

Options:
  -h, --help       Print this help
  -V, --version    Print the version"
            );
            return Ok(());
        }
        Invocation::Version => {
            println!("{program} {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Invocation::Open(paths) => paths,
    };

    let config = jumppad_config::load();
    // The same description a `config.toml` reload builds to decide whether
    // the window on screen still matches the file - see `window::replace`.
    let window = window::settings(&config);

    // Defaults to `info` level (still overridable via `RUST_LOG`) so
    // `iced_wgpu`'s own adapter/format/alpha-mode logging is visible.
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .try_init();

    // Neither backend can do transparency on every platform, and the failure
    // is silent - the window just comes up solid, which reads as a rendering
    // bug rather than a wrong-binary problem. Say so up front instead.
    if window.transparent
        && let Some(reason) = OPAQUE_WINDOW_REASON
    {
        eprintln!("jumppad: {reason}");
    }

    // `config` and `paths` are cloned per call since the boot closure must be
    // `Fn`, not just `FnOnce`.
    iced::application(
        move || JumpPadApp::new(config.clone(), paths.clone()),
        JumpPadApp::update,
        JumpPadApp::view,
    )
    .title("JumpPad")
    .window(window)
    // The icon glyphs the tab bar draws with. iced loads faces by bytes at
    // startup and then finds them by family, which is what `ICON_FONT` in
    // `app.rs` names.
    .font(jumppad_icons::FONT)
    // iced defaults this on, but its MSAA only ever applies to triangle
    // primitives - meshes, canvases, gradient quads - and this app draws
    // none. Quads and text are always `count: 1` regardless. So it buys
    // nothing visually and costs pipelines plus a 4x-sampled render target.
    .antialiasing(false)
    .subscription(JumpPadApp::subscription)
    .theme(JumpPadApp::theme)
    .style(JumpPadApp::style)
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Invocation {
        parse_args(args.iter().map(OsString::from))
    }

    fn paths(args: &[&str]) -> Vec<PathBuf> {
        match parse(args) {
            Invocation::Open(paths) => paths,
            other => panic!("expected paths, got {other:?}"),
        }
    }

    #[test]
    fn no_arguments_opens_nothing() {
        assert!(paths(&[]).is_empty());
    }

    #[test]
    fn bare_arguments_are_paths_in_order() {
        assert_eq!(
            paths(&["a.txt", "../b.md"]),
            vec![PathBuf::from("a.txt"), PathBuf::from("../b.md")]
        );
    }

    #[test]
    fn help_and_version_win_wherever_they_appear() {
        assert_eq!(parse(&["-h"]), Invocation::Help);
        assert_eq!(parse(&["--help"]), Invocation::Help);
        assert_eq!(parse(&["a.txt", "--help"]), Invocation::Help);
        assert_eq!(parse(&["-V"]), Invocation::Version);
        assert_eq!(parse(&["--version"]), Invocation::Version);
        assert_eq!(parse(&["a.txt", "--version"]), Invocation::Version);
    }

    #[test]
    fn an_unrecognized_flag_is_just_a_filename() {
        // No flag surface beyond the two above - `-x` becomes a path, and
        // fails later as a missing file rather than as a usage error.
        assert_eq!(paths(&["-x"]), vec![PathBuf::from("-x")]);
    }

    #[test]
    fn program_name_falls_back_when_argv0_is_missing_or_odd() {
        assert_eq!(
            program_name(Some(&OsString::from("/usr/bin/jumppad"))),
            "jumppad"
        );
        assert_eq!(
            program_name(Some(&OsString::from("jumppad-gpu"))),
            "jumppad-gpu"
        );
        assert_eq!(program_name(None), "jumppad");
    }
}
