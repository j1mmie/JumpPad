# xizor

A Rust-based plaintext editor with a focus on performance and
low system requirements.

## Why this exists

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

Xizor is an alternative to Notepad and TextEdit. It is **not** a code editor;
it does not do anything better than Vim, VSCode, Emacs, or whatever your 
favorite text editor is. There's no debugger, no terminal, no plugin ecosystem,
no project explorer. It's meant for quickly editing the occasional
config file, or saving a note. And it does that extremely well!

## Design principles

- **Low memory footprint by default.** This is the thing xizor optimizes
  for above all else. Every other feature is designed to cost nothing
  until you actually use it.
- **Fast to open, fast to edit.** Loading a file should never feel like
  waiting on anything else the app is doing in the background.
- **Highlighting is a bonus, not a requirement.** Syntax highlighting adds
  color and structure when it's useful, but it never gets in the way of
  the two points above, and you can remove it entirely if you don't want
  it.

## How it stays lightweight

**Rendering.** By default, xizor draws its own UI in software (via
`tiny-skia`) instead of going through the GPU. For a text editor, the GPU
is mostly unnecessary weight - software rendering is roughly a tenth of
the memory cost. GPU-accelerated rendering is still available as an
opt-in for anyone who wants it, just not the default.

**Syntax highlighting.** Highlighting happens asynchronously, off the
critical path of opening a file - so a file is never slower to load
because of it. The highlighter for a given file type is only loaded the
first time you actually open a file of that type, not at startup, which
keeps idle memory use down. Each highlighter runs inside a small WASM
sandbox, which keeps the safety and stability of untrusted parsing code
without the cost (or risk) of running it natively. Highlighting itself is
powered by tree-sitter.

**Optional by design.** Highlighters are separate, removable pieces.
Don't need syntax highlighting for a particular format, or want the
smallest possible footprint? Remove the highlighter and xizor doesn't
know the difference - startup time and idle memory are unaffected either
way, since nothing is loaded until it's needed.

## Status

Early prototype. Things are still moving fast and breaking.
