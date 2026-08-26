// The console-hiding `windows_subsystem` attribute lives on the two binary
// crate roots in `src/bin/`, not here - it is ignored on a library. What
// hands a terminal back on demand is `JUMPPAD_DEBUG`; see `debug.rs`.

mod app;
mod debug;
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
use std::io::Write;
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
    // First, before anything with something to report gets to run. Opens a
    // console and installs the logger when `JUMPPAD_DEBUG` is set, and does
    // nothing whatsoever when it isn't - see `debug.rs`.
    debug::start();

    let mut argv = std::env::args_os();
    let program = program_name(argv.next().as_ref());
    let paths = match parse_args(argv) {
        Invocation::Help => {
            answer_the_shell(&format!(
                "\
{program} - a lightweight plaintext editor

Usage: {program} [FILE]...

Opens each FILE in its own tab. A FILE that doesn't exist yet opens as an
empty tab saved to that path on the first save.

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Environment:
  {debug_var}=1  Open a terminal and log to it",
                debug_var = debug::ENV_VAR,
            ));
            return Ok(());
        }
        Invocation::Version => {
            answer_the_shell(&format!(
                "{program} {}",
                env!("CARGO_PKG_VERSION")
            ));
            return Ok(());
        }
        Invocation::Open(paths) => paths,
    };

    let config = jumppad_config::load();
    prefer_gpu(config.gpu.power);
    // Read out before `config` is moved into the boot closure below.
    let vsync = config.gpu.vsync;
    // The same description a `config.toml` reload builds to decide whether
    // the window on screen still matches the file - see `window::replace`.
    let window = window::settings(&config);

    // Neither backend can do transparency on every platform, and the failure
    // is silent - the window just comes up solid, which reads as a rendering
    // bug rather than a wrong-binary problem. Say so up front instead.
    if window.transparent
        && let Some(reason) = OPAQUE_WINDOW_REASON
    {
        log::warn!("{reason}");
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
    // One struct rather than the `.font()`/`.antialiasing()` builders that
    // used to stand here: `.settings` replaces the whole `Settings`, so
    // either of those called after it would have been silently thrown away.
    // Naming every field we care about in one place is what keeps that from
    // being an ordering rule nobody knows about.
    .settings(iced::Settings {
        // The icon glyphs the tab bar draws with. iced loads faces by bytes
        // at startup and then finds them by family, which is what
        // `ICON_FONT` in `app.rs` names.
        fonts: vec![app::ICON_FONT_BYTES.into()],
        // iced defaults this on, but its MSAA only ever applies to triangle
        // primitives - meshes, canvases, gradient quads - and this app draws
        // none. Quads and text are always `count: 1` regardless. So it buys
        // nothing visually and costs pipelines plus a 4x-sampled render
        // target.
        antialiasing: false,
        // `[gpu] vsync`, and off by default - see `GpuConfig::vsync` for why
        // waiting on the display is what made `jumppad-gpu` feel slower than
        // the software binary rather than faster. Inert in `jumppad`:
        // `iced_tiny_skia` never reads this field.
        vsync,
        ..iced::Settings::default()
    })
    .subscription(JumpPadApp::subscription)
    .theme(JumpPadApp::theme)
    .style(JumpPadApp::style)
    .run()
}

/// Points wgpu at the adapter `[gpu] power` asked for.
///
/// Through the environment because that is the only door iced leaves open:
/// it builds its wgpu compositor deep inside `run()` and takes no adapter
/// preference from the caller, but it does consult wgpu's own
/// `WGPU_POWER_PREF` on the way (`iced_wgpu`'s `window::compositor`). So the
/// config setting becomes that variable.
///
/// A `WGPU_POWER_PREF` already in the environment is left alone. It is the
/// same variable meaning the same thing, and someone who exported it by hand
/// is answering the question more locally than a config file can.
///
/// Inert in the `jumppad` binary, which has no wgpu to read it.
fn prefer_gpu(power: jumppad_config::GpuPower) {
    const VAR: &str = "WGPU_POWER_PREF";
    if let Some(existing) = std::env::var_os(VAR) {
        log::debug!("{VAR}={existing:?} in the environment, leaving it");
        return;
    }
    // SAFETY: `set_var` is unsound with another thread reading the
    // environment concurrently. Nothing in this process has spawned a thread
    // yet - `debug::start` installs a logger and `load` reads files, both on
    // this thread - and iced has not been handed control. That ordering is
    // why the call sits up in `run` rather than next to the code that cares.
    unsafe { std::env::set_var(VAR, power.as_wgpu_power_pref()) };
    // Said out loud because the answer is assembled from three places - a
    // config file, this default, and an environment that silently outranks
    // both - and iced reports only which adapter it ended up with. Without
    // this line, working out why the adapter was not the expected one means
    // checking all three by hand.
    log::debug!("asking wgpu for {power:?} power ({VAR} was unset)");
}

/// Answers `--help`/`--version` on the terminal that asked.
///
/// These two are the standing exception to the `JUMPPAD_DEBUG` rule in
/// `debug.rs`: they are the reply to something typed at a prompt rather than
/// a report on how the app is doing, and a packager checking `--version`
/// shouldn't have to know a debug switch exists. Because both binaries link
/// as GUI programs, though, that reply has nowhere to land until we ask for
/// the launching shell's console - hence the attach, which is a no-op
/// everywhere but Windows.
///
/// `writeln!` rather than `println!`, and the result dropped: a launch with
/// no console behind it at all (a shortcut, the Start menu, stdout closed)
/// should end the process quietly, not panic on the way out.
fn answer_the_shell(text: &str) {
    debug::console::attach_to_parent();
    let _ = writeln!(std::io::stdout(), "{text}");
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
