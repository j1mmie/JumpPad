
<div align="center">
  
<img src="assets/JumpPad.svg" width="128px" />

# JumpPad

A lightweight, cross-platform, Rust-based plaintext editor with modern features and minimal bloat

</div>

## Motivation

JumpPad is a learning project in the Rust ecosystem. It can be seen as a
replacement for Notepad or Textedit - a handy, plaintext editor with a
minimal featureset and low system requirements. At the same time, it tries
to improve upon shortcomings of those projects, such as:

- Poor performance
- Overconsumption of memory
- Bloat from features that nobody asked for, like
  - Markdown rendering
  - Cloud support (OpenDrive, iCloud)
- [Security vulnerabilities due to the above](https://www.cve.org/CVERecord?id=CVE-2026-20841)

JumpPad starts instantly, uses very little memory, and only includes features
that are useful to text editing.

JumpPad is **not** a code editor; it (purposely) lacks many features
that a code editor has:
- no debugger
- no terminal
- no plugin ecosystem
- no project explorer

It's meant to be small and handy, for everyday tasks like editing a config
file, saving a note, or drafting a message.

## Features
 - Speed
 - Portability
 - Tabs
 - Syntax highlighting
 - Transparency
 - Automatic draft saving
 - Configurable via TOML
 - More to come...

## Design principles

- **Minimal scope.** Scope should only increase thoughtfully, so that the
  subsequent principles can be achieved.
- **Low memory footprint.** The editor should consume no more memory than
  it needs at any given time.
- **Fast startup.** The editor should start instantly.
- **Fast file loading.** Files should load instantly.
- **Low CPU usage.** At idle, the editor should consume no CPU. While
  editing, the editor should consume as little CPU as possible.

## Architecture

Language: [rust](https://github.com/rust-lang/rust)
GUI Library: [iced](https://github.com/iced-rs/iced)
Renderer:
  - Software - [tiny-skia](https://github.com/linebender/tiny-skia): low memory, uses CPU
  - Hardware - [wgpu](https://github.com/gfx-rs/wgpu): more memory, offloads some work to GPU 
Syntax Highlighting:
  - [tree-sitter](https://github.com/tree-sitter/tree-sitter)
  - Grammars are WAM
  - performed asynchronously

## Building from source

### Build syntax highlighter grammars:
```
./syntaxes/build-grammars.sh
```
This clones each grammar's upstream source and compiles it to WASM into
`syntaxes/output`. The contents of this folder should be placed in a `syntaxes`
folder, sibling to a JumpPad binary, so that it can detect the grammars.
Requires `git` and `npm`.


### Build JumpPad:

JumpPad has two binary targets, one for software rendering and one for
GPU-powered rendering. Note: the GPU rendering binary occupies much more memory

One caveat on transparency (`[alpha] background` below `1.0`): on macOS the
software binary cannot produce a translucent window, because its
presentation path drops the alpha channel. Use `jumppad-gpu` there. It
prints a warning at startup if you've configured transparency it can't
deliver. Windows and Linux support transparency on both binaries.

To build both release binaries at once, for your host platform:
```
./scripts/build-release.sh       # Linux/macOS/WSL
./scripts/build-release.ps1      # Windows (produces jumppad.exe, jumppad-gpu.exe)
```

Or the equivalent commands by hand:
```
cargo build --release -p jumppad --bin jumppad
cargo build --release -p jumppad --bin jumppad-gpu --no-default-features --features wgpu
```

#### Cross-compiling Windows binaries from Linux/WSL

With a mingw-w64 installed, you can build to Windows from Linux or WSL.
```
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64   # Debian/Ubuntu; package name varies by distro
```

Then build both Windows binaries with:
```
./scripts/build-release.sh x86_64-pc-windows-gnu
```

Produces:
- `target/x86_64-pc-windows-gnu/release/jumppad.exe`
- `target/x86_64-pc-windows-gnu/release/jumppad-gpu.exe`

## Status

Early prototype.
