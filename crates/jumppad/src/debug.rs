//! The one switch that decides whether this build talks to a terminal.
//!
//! Both binaries link as Windows GUI programs (`windows_subsystem = "windows"`
//! in `src/bin/*.rs`), so a double-clicked JumpPad gets no console window at
//! all: `GetStdHandle` hands std a null handle and every `println!` in the
//! process goes nowhere. That is the wanted default - a text editor should
//! not drag a black box along behind it.
//!
//! `JUMPPAD_DEBUG=1` turns that around. [`start`] opens a console - reusing
//! the shell's, when it was launched from one - and installs the `log`
//! backend. Without it no logger is installed at all, and that absence is
//! what silences the `log::*` calls scattered through this crate and its
//! siblings: the `log` facade drops every record until something calls
//! `set_logger`, so a diagnostic costs one relaxed atomic load and never
//! formats its arguments.
//!
//! Which is why diagnostics go through `log::*` and never through
//! `println!`/`eprintln!` - a bare print would escape the switch, and on a
//! console-less Windows run it would write into the void anyway. The only
//! writes to stdout that outlive this switch are `--help` and `--version`,
//! which answer a question the user typed rather than reporting on the app;
//! `run` gives those the launching shell's console explicitly.

use std::sync::OnceLock;

/// The environment variable that opens the terminal and starts the logging.
pub const ENV_VAR: &str = "JUMPPAD_DEBUG";

/// Whether `JUMPPAD_DEBUG` is set to something that means yes.
///
/// Read once and cached. The answer decides whether a logger exists at all,
/// and a logger can only be installed once per process, so re-reading the
/// environment later could only produce an answer the process can't act on.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var(ENV_VAR).is_ok_and(|value| is_true(&value))
    })
}

/// Spelling-tolerant on purpose: this gets typed at a prompt by hand far more
/// often than it gets set by a script. Anything outside the list reads as
/// off - including the empty string that a bare `set JUMPPAD_DEBUG=` on
/// Windows leaves behind, and the `0`/`false` someone writes meaning to
/// switch it back off.
fn is_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Opens the terminal and starts logging, if `JUMPPAD_DEBUG` asked for it.
///
/// Call once, first thing, before anything with something to report runs.
/// A second call is harmless - the console is already attached by then and
/// `try_init` declines to replace a logger - but nothing logged before the
/// first call is recoverable.
pub fn start() {
    if !enabled() {
        return;
    }
    console::attach();

    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(DEFAULT_FILTER),
    )
    .try_init();
}

/// What `JUMPPAD_DEBUG` turns on when `RUST_LOG` doesn't say otherwise.
///
/// `debug` globally rather than the `info` this used to default to: the whole
/// point of the switch is the chatter, and the grammar-loading and
/// window-backend lines worth having when chasing a rendering bug - iced and
/// wgpu's adapter, format and alpha-mode reporting among them - sit below
/// `info`.
///
/// Cranelift is the exception, and it has to be one. wasmtime compiles every
/// `.wasm` grammar through it at startup, and at `debug` that is thousands
/// of lines of per-pass timing and per-function statistics describing work
/// that is going fine - enough to bury anything else the startup had to say.
/// `warn` keeps a real Cranelift failure visible and drops the commentary.
///
/// `RUST_LOG` still wins wherever it is set, so
/// `RUST_LOG=cranelift_codegen=debug` brings it all back for anyone
/// debugging the grammar pipeline itself.
const DEFAULT_FILTER: &str = "debug,cranelift_codegen=warn,wasmtime_internal_cranelift=warn,\
     regalloc2=warn";

/// Getting a readable stdout in front of a GUI-subsystem process.
#[cfg(target_os = "windows")]
pub(crate) mod console {
    use std::fs::OpenOptions;
    use std::os::windows::io::IntoRawHandle;

    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, GetConsoleWindow,
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        SetStdHandle,
    };

    /// Gives this process a console, preferring the one it was launched
    /// from. Called when `JUMPPAD_DEBUG` is on.
    ///
    /// Reusing the launching shell's console rather than always allocating
    /// means `set JUMPPAD_DEBUG=1 && jumppad.exe` puts its output in the
    /// window you typed that into, where it can be scrolled back and
    /// selected. A launch with no console behind it - Explorer, a shortcut,
    /// the Start menu - gets a window of its own instead, which is the only
    /// way that run has to show anything.
    pub fn attach() {
        if attach_to_parent() {
            return;
        }
        // SAFETY: no arguments and no out-params. Failure leaves the process
        // exactly as it was, console-less, and the records simply go nowhere
        // - no worse off than not having asked.
        if unsafe { AllocConsole() } != 0 {
            bind_std_handles();
        }
    }

    /// Attaches to the launching process's console, reporting whether stdout
    /// now lands somewhere a person can read.
    ///
    /// Never allocates: this is also the `--help`/`--version` path, where a
    /// console window of our own would paint the text and then vanish with
    /// the process a few milliseconds later.
    pub fn attach_to_parent() -> bool {
        // Already having one is the `AttachConsole` failure case that isn't
        // a failure - it returns `ERROR_ACCESS_DENIED` for a process that is
        // attached to a console already, which happens whenever this is
        // reached twice or the binary was linked as a console program.
        if has_console() {
            return true;
        }
        // SAFETY: takes a process id by value and returns a BOOL. Fails
        // harmlessly when whatever launched us had no console to share.
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
            return false;
        }
        bind_std_handles();
        true
    }

    /// SAFETY: no arguments. A null window handle - which `windows-sys` 0.52
    /// still spells as a bare `isize` rather than a pointer - means no
    /// console is attached.
    fn has_console() -> bool {
        let window = unsafe { GetConsoleWindow() };
        window != 0
    }

    /// Points std's stdin/stdout/stderr at the console device.
    ///
    /// `AllocConsole` sets the three standard handles up itself, but
    /// `AttachConsole` deliberately doesn't touch a process that started
    /// life without them: they stay null and `println!` keeps writing into
    /// the void. Opening the console's own `CONOUT$`/`CONIN$` pseudo-files
    /// and installing those covers both cases, and in the `AllocConsole`
    /// case costs nothing beyond a second handle onto a device already open.
    ///
    /// This reaches `println!` calls anywhere in the process, including the
    /// ones already compiled into the iced and wgpu stacks: std looks
    /// `GetStdHandle` up on every write rather than caching it at first use.
    fn bind_std_handles() {
        // Both output slots share one handle to the same screen buffer -
        // that is what interleaves `log`'s stderr records with anything
        // written to stdout in the order they actually happened.
        let out = unset(STD_OUTPUT_HANDLE);
        let err = unset(STD_ERROR_HANDLE);
        if (out || err)
            && let Some(handle) = open_device("CONOUT$", true)
        {
            if out {
                install(STD_OUTPUT_HANDLE, handle);
            }
            if err {
                install(STD_ERROR_HANDLE, handle);
            }
        }
        // Nothing here reads stdin. It's bound anyway so the console is a
        // whole one: a null `STD_INPUT_HANDLE` is what makes `GetConsoleMode`
        // fail for any dependency that asks whether it's talking to a
        // terminal, and answering "no" on a real console invites a library
        // to strip the colour out of the output we opened this window for.
        if unset(STD_INPUT_HANDLE)
            && let Some(handle) = open_device("CONIN$", false)
        {
            install(STD_INPUT_HANDLE, handle);
        }
    }

    /// Whether Windows has nothing in that standard slot.
    ///
    /// The check that keeps this from trampling a redirect. A process
    /// launched as `jumppad-gpu > log.txt`, or into a pipe, inherits those
    /// handles whatever its subsystem - redirection is set up by the parent
    /// before the child starts, and has nothing to do with owning a console.
    /// Binding the console over the top of them sent the output to a window
    /// while the file the user was watching stayed empty.
    ///
    /// So only an empty slot gets filled. `GetStdHandle` answers null for a
    /// GUI process launched without one and `INVALID_HANDLE_VALUE` on error;
    /// anything else is a handle somebody meant this process to write to.
    /// `AllocConsole` sets all three itself, so its path lands here and
    /// correctly does nothing.
    fn unset(slot: u32) -> bool {
        // SAFETY: takes a slot id by value and returns a handle or a
        // sentinel; it borrows nothing and cannot fail in a way that matters.
        let handle = unsafe { GetStdHandle(slot) };
        handle == 0 || handle == INVALID_HANDLE_VALUE
    }

    /// Opens one of the console's pseudo-files and leaks the handle.
    ///
    /// `std::fs` rather than a raw `CreateFileW` because it already spells
    /// out the share mode and `OPEN_EXISTING` disposition these devices need.
    /// `into_raw_handle` on purpose: the handle has to outlive every `File`
    /// we could hold it in - it is the process's stdout for the rest of the
    /// run - and letting that `File` drop would close the console out from
    /// under the next `println!`.
    fn open_device(name: &str, write: bool) -> Option<HANDLE> {
        OpenOptions::new()
            .read(true)
            .write(write)
            .open(name)
            .ok()
            // std hands back a `RawHandle` pointer; `HANDLE` is the same
            // value typed the way this version of `windows-sys` spells it.
            .map(|file| file.into_raw_handle() as HANDLE)
    }

    fn install(slot: u32, handle: HANDLE) {
        // SAFETY: `handle` came from a successful open of a console device
        // and is never closed, so the slot stays valid for the rest of the
        // process's life.
        unsafe { SetStdHandle(slot, handle) };
    }
}

/// The same two entry points, with nothing to do.
///
/// `windows_subsystem` is a Windows-only linker setting, so there is no
/// console being withheld here to hand back: a process launched from a shell
/// inherited its stdio already, and one launched from Finder or a `.desktop`
/// entry has nothing to attach *to*. `JUMPPAD_DEBUG` on macOS and Linux is
/// therefore purely about whether a logger gets installed.
#[cfg(not(target_os = "windows"))]
pub(crate) mod console {
    pub fn attach() {}

    pub fn attach_to_parent() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::is_true;

    #[test]
    fn the_usual_spellings_of_yes_all_count() {
        for value in ["1", "true", "TRUE", "True", "yes", "on", " true "] {
            assert!(is_true(value), "{value:?} should read as on");
        }
    }

    #[test]
    fn everything_else_reads_as_off() {
        // `""` is what a bare `set JUMPPAD_DEBUG=` leaves behind, and `0`
        // and `false` are what someone writes meaning to turn it back off -
        // a "set at all means on" rule would strand both of them.
        for value in ["", " ", "0", "false", "no", "off", "2", "please"] {
            assert!(!is_true(value), "{value:?} should read as off");
        }
    }
}
