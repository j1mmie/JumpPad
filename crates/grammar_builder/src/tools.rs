//! The outside tools this build drives: `git` for the grammar sources, and
//! npm's `tree-sitter` for compiling them to wasm.

use std::error::Error;
use std::path::Path;
use std::process::{Command, Stdio};

/// npm ships its commands as `npm.cmd` and `npx.cmd` on Windows, and
/// `Command::new` there appends only `.exe` when it searches `PATH` - the
/// bare name resolves to nothing, so the build would fail before it started.
/// Going through `cmd /C` is what lets one call work on both platforms.
fn npm_tool(program: &str) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", program]);
        command
    } else {
        Command::new(program)
    }
}

/// Checks the outside tools are on `PATH` before any work starts.
///
/// Worth doing up front: otherwise the first failure is a bare non-zero exit
/// from whichever command ran first, and on Windows a missing Node arrives
/// as `cmd` reporting exit code 1, which names nothing at all.
pub fn check_available() -> Result<(), Box<dyn Error>> {
    let mut missing = Vec::new();
    if !runs(Command::new("git").arg("--version")) {
        missing.push("git");
    }
    if !runs(npm_tool("npm").arg("--version")) {
        missing.push("Node (which provides npm and npx)");
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!("not found on PATH: {}", missing.join(", ")).into())
}

fn runs(command: &mut Command) -> bool {
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Installs the pinned `tree-sitter-cli`. Run in `syntaxes/`, where the
/// `package.json` naming the version lives.
pub fn install_tree_sitter(syntaxes: &Path) -> Result<(), Box<dyn Error>> {
    let mut command = npm_tool("npm");
    command.arg("install").current_dir(syntaxes);
    run(command, "npm install")
}

pub fn clone(repository: &str, into: &Path) -> Result<(), Box<dyn Error>> {
    println!("cloning {repository}");
    let mut command = Command::new("git");
    command
        .args(["clone", "--quiet", "--depth", "1"])
        .arg(format!("https://github.com/{repository}.git"))
        .arg(into);
    run(command, &format!("cloning {repository}"))
}

/// Compiles one grammar to wasm.
///
/// Since 0.26.1 the CLI downloads its own wasi-sdk on first use, so this
/// needs neither Docker nor Emscripten on any platform - only the several
/// hundred megabytes it caches once.
pub fn build_wasm(
    syntaxes: &Path,
    source: &Path,
    destination: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut command = npm_tool("npx");
    command
        .args(["--yes", "tree-sitter-cli", "build", "--wasm", "-o"])
        .arg(destination)
        .arg(source)
        // From `syntaxes/`, so npx resolves the version `package.json` pins
        // rather than whatever a per-call lookup happens to find.
        .current_dir(syntaxes);
    run(command, &format!("building {}", destination.display()))
}

fn run(mut command: Command, what: &str) -> Result<(), Box<dyn Error>> {
    let status = command
        .status()
        .map_err(|error| format!("{what}: {error}"))?;
    if !status.success() {
        return Err(format!("{what}: {status}").into());
    }
    Ok(())
}
