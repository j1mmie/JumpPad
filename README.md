# xizor

A Rust-based plaintext editor with a focus on performance and
low system requirements.

## Goals

Xizor came about as a result of the
[enshittification](https://en.wikipedia.org/wiki/Enshittification) of many
commercial operating systems' built-in plaintext editors. Mainstays like
Notepad and Textedit began to suffer from feature creep, which resulted in:

- Unoptimized performance
- Bloat from features that nobody asked for, like
  - Markdown rendering
  - Cloud support (OpenDrive, iCloud)
- [Security vulnerabilities due to the above](https://www.cve.org/CVERecord?id=CVE-2026-20841)

Xizor opens instantly, uses very little memory, and only includes features
that are useful to text editing.

Xizor is an alternative to Notepad and TextEdit. But it is **not** a code
editor; it (purposely) lacks a LOT of the features of a code editor:
- no debugger
- no terminal
- no plugin ecosystem
- no project explorer

It's meant for quickly editing the occasional config file, or saving a note.
And it does that pretty well!

## Design principles

- **Minimal scope.** Scope should be thoughtful, so that memory, performance,
  and security are never at risk 
- **Low memory footprint.** The editor should not tax the system at all
- **Fast startup.** The editor should be quick to start, not accessing many
  files or parsing numerous configurations or loading unnecessary plugins.
- **Fast file loading.** File should load quickly and be ready for reading
  and writing immediately.
- **Minimize unnecessary work.** Features that don't benefit all use cases
  should be loaded asynchronously (or, lazily), so they don't occupy
  extra memory when not used, and they don't distract from the main goal of
  the project; file editing.

## How it stays lightweight

**Rendering.** Xizor uses `iced`, a tiny, high performance, low memory
UI framework. It comes in two flavors:

- Software renderer - `tiny-skia`, which has a low memory footprint, but
  renders on the CPU
- Hardware renderer - `wgpu`, which has a larger memory footprint, but
  offloads it's rendering to the GPU 

Software rendering is roughly a tenth of the memory cost. GPU-accelerated 
however may be preferred for depending on your use case.

**Syntax highlighting.** Powered by `tree-sitter`. Highlighting modules may be
added to the `syntaxes/` folder. These modules are small .wasm programs that 
provide grammar support for the highlighter engine. They run in their own VM,
so syntax highlighting is more secure than if it were run directly on the CPU.

Highlighting modules are loaded lazily if there's a tab open that uses that
module. They're unloaded if no such tabs are open. Highlighting is performed
asynchronously so that it does not block file loading or user input.

## Building from source

### Build syntax highlighter grammars:
```
./scripts/build-grammars.sh
```
This clones each grammar's upstream source and compiles it to WASM into
`syntaxes/` at the repo root, where the app looks for it. Requires `git`
and `npx` (from Node.js) - see the script for the exact sources.


### Build Xizor binaries:

Xizor ships as two separate binaries, one per renderer - `iced`'s `wgpu`
and `tiny-skia` backends can't coexist in one binary without carrying the
weight of both, so which one you get is a compile-time choice, not a
runtime one.

For everyday development, `cargo build` / `cargo run` alone build and run
just `xizor` (tiny-skia, the default).

To build both release binaries at once, for your host platform:
```
./scripts/build-release.sh       # Linux/macOS/WSL
./scripts/build-release.ps1      # Windows (produces xizor.exe, xizor-gpu.exe)
```
Or the equivalent commands by hand:
```
cargo build --release -p xizor --bin xizor
cargo build --release -p xizor --bin xizor-gpu --no-default-features --features wgpu
```

#### Cross-compiling Windows binaries from Linux/WSL

No need for a native Windows machine - with the `x86_64-pc-windows-gnu`
target and a mingw-w64 linker installed:
```
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64   # Debian/Ubuntu; package name varies by distro
```
build both Windows binaries with:
```
./scripts/build-release.sh x86_64-pc-windows-gnu
```
which produces `target/x86_64-pc-windows-gnu/release/xizor.exe` and
`target/x86_64-pc-windows-gnu/release/xizor-gpu.exe`. This is the same
target-triple argument `cargo build --target <triple>` takes, so the
by-hand commands above work too with `--target x86_64-pc-windows-gnu`
added to each.

## Status

Early prototype.
