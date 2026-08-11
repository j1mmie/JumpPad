# AGENTS.md

Working notes for an AI agent operating on this codebase. See `README.md`
for the human-facing pitch; this file is the architecture map, the
"why is it built this way," and the list of things that will bite you if
you don't know about them going in.

## What this is, in one paragraph

JumpPad is a lightweight plaintext editor built on `iced` (Rust, Elm-style
GUI framework). It targets config files, notes, and structured data
(YAML/XML/CSV/etc.) - not source code. The overriding design constraint is
low idle memory footprint; every feature (rendering backend, syntax
highlighting) is built so it costs nothing until actually used, and can be
turned off entirely without touching the rest of the app.

The app was originally built on `egui` and ported to `iced`. Some comments
in the codebase still reference `egui` behavior for contrast (e.g. "unlike
egui's `ctx.request_repaint()`") - that's this history, not a mistake.

## Comment style

Comments stay short and sit right next to what they describe:

- High level, not step-by-step narration.
- Placed at the thing's first declaration, not cross-referencing other files/areas unless truly necessary.
- Short - no meandering.
- Prefer a self-documenting name over a comment.
- Skip the "why" (bug history, feature origin) unless it's a genuine non-obvious gotcha.

## Workspace layout

```
crates/
  jumppad/           the application shell: iced::Program, tabs, menus, file I/O, theming
  editor_core/       the abstraction boundary between the shell and the actual text widget
  jumppad_textarea/  the one TextEditorWidget impl today; owns a fork of iced's text_editor
  syntax_registry/   loads/caches/refcounts tree-sitter WASM grammars; no iced dependency
  jumppad_config/    config.toml loading + defaults; no iced dependency
syntaxes/            *.wasm grammar files + *.injections.scm queries (see below)
```

Dependency direction is one-way: `jumppad` depends on everything;
`editor_core`, `syntax_registry`, and `jumppad_config` depend on nothing
inside this workspace. `jumppad_textarea` depends on `editor_core` (to
implement its trait) and `syntax_registry` (to drive highlighting), but
not on `jumppad`. This is deliberate - see `editor_core::widget::TextEditorWidget`'s
doc comment: a future non-iced editor widget should be a new crate
implementing that trait, not a rewrite of `jumppad`.

`editor_core` also holds the two pieces of theming both sides need:
`darkening_wash` and `FLOATING_SURFACE_DARKEN` (see the transparent-windows
section). It's the only crate `jumppad` and `jumppad_textarea` share, so a
color both of them paint with lives there rather than being copied.

## The `TextEditorWidget` boundary

`editor_core::widget::TextEditorWidget` (view / update / text / set_text /
reload_text / poll_highlighting / has_pending_highlighting) is the entire contract
between the app shell and "the thing that actually renders and edits
text." `EditorMessage` is a concrete enum, not generic over the widget
implementation - there's only ever one editor implementation live at a
time, so there's no need to thread a generic `Message` type through the
whole app for it.

`Tab` (`editor_core::tab::Tab`) owns one `Box<dyn TextEditorWidget>` plus
tab metadata (path, dirty flag, id). `JumpPadApp` in `crates/jumppad/src/app.rs`
holds `Vec<Tab>` and an `active: usize` index; only the active tab's
`.view()` is ever placed in the widget tree, one tab at a time.

`poll_highlighting()` exists because iced, unlike egui, doesn't re-run
application code every frame - it only reacts to messages. `JumpPadApp`'s
`subscription()` runs a 50ms timer (`Message::PollHighlighting`) *only*
while some tab has a grammar load in flight, and stops it once nothing is
pending, so there's no permanent background timer once everything's
settled.

## The text-area fork (`jumppad_textarea`)

`crates/jumppad_textarea/src/text_editor.rs` is a fork of `iced_widget`
0.14.2's `text_editor`, and it is **JumpPad's widget now** - not a vendored
copy tracking upstream. Edit it freely; there is no re-sync to protect.

It was forked to reach one private field. `text_editor::Content` is a
newtype over `RefCell<Internal<R>>` whose `editor` field is private, so
application code cannot get at the `iced_graphics::text::Editor` - and with
it the cosmic-text `Buffer` - underneath. Everything below that field is
already public (`Editor::buffer()`, `Buffer::scroll()`, `Buffer::lines`),
which is why owning `Content` was the whole unlock. It's the same field
that blocks background highlighting for find matches and multiple cursors
(see the find-palette section), so those belong here too.

Porting notes, in case any of it looks accidental:

- Imports are `iced_core::`, where upstream says `crate::core::` - the
  crate takes `iced_core` as a direct dependency, and Cargo unifies it with
  the copy inside `iced`, so the types are the same ones the app sees.
- The `Widget` impl is pinned to
  `Renderer: text::Renderer<Font = iced_core::Font, Editor = graphics::text::Editor>`
  rather than generic over `text::Renderer`, since the scrollbar needs the
  concrete editor. That bound has to be repeated on the `Element`
  conversion.
- Upstream's `highlight()` (the `iced_highlighter` convenience) is gone;
  JumpPad drives `highlight_with` with its own tree-sitter highlighter.
  `text_editor()`, upstream's constructor from `iced_widget::helpers`, moved
  in.
- One let-chain was unwound - the workspace is edition 2021, `iced_widget`
  is 2024.

### The scrollbar

An overlay scrollbar: a rounded thumb on an invisible track, at the right
edge of the text area. Hidden until the pointer enters the right 100px or
the document scrolls, then held for 900ms and faded out. Drag the thumb to
scroll. Geometry and fade math are pure functions in `scrollbar.rs`, taking
`now: Instant` so they test without a window (same convention as
`history.rs`).

**Position is measured in logical lines, not wrapped visual rows.** That is
deliberate, and the obvious "fix" is a regression: counting rows means
summing `BufferLine::layout_opt()` over the document every frame, and
cosmic-text shapes lazily, so an off-screen line that wraps to three rows
reports one until it scrolls into view. The total - and the thumb's height
with it - would twitch as you scroll. Logical lines cost O(1), are exact for
anything that doesn't wrap, and are stable everywhere.

**Gotcha - `State::touch` reads the current opacity, so it has to run before
whatever flag is changing.** `hovered` and a live `drag` both freeze the
fade, which means `active_at` is deliberately stale while either holds;
touching *after* flipping the flag reads through the new one and sees a
fade that never actually ran. Two bugs came out of getting this wrong: the
thumb invisible for the whole of a drag, and re-entering a fade-out
snapping to full instead of resuming.

**Gotcha - the fade-in origin is its own field.** `revealed_at` is separate
from `active_at` because activity *repeats* - a drag or a spun wheel touches
the state every few milliseconds. Folding them together restarts the ramp on
every event, so the thumb stays pinned at invisible for exactly as long as
the user keeps going. Re-touching back-dates `revealed_at` by the opacity
already showing, so catching it mid-fade resumes rather than restarting.

**Redraws are scheduled, not subscribed.** `State::next_redraw` returns the
next instant a frame is actually needed - the following frame while a ramp
is running, the end of the hold while merely waiting it out, and `None` once
things settle - and `update` feeds it to `shell.request_redraw_at`, exactly
as the caret blink already does. No `subscription()` entry, and idle CPU
stays at zero with the thumb hidden *or* held open (measured, not assumed).

Colors come from `text_editor::Style`'s two added fields, filled by
`scrollbar_thumb_style` in `lib.rs`: the find palette's `darkening_wash`
plus its `background.strong` border, so the two surfaces that float over
document text read as one material. The wash is what keeps a transparent
window honest; the border is what keeps the thumb visible on near-black
themes, where a wash has almost nothing left to darken. A test in `app.rs`
asserts the two stay equal across every theme, since they're defined in
different crates.

### Pixel-granular scrolling (and the `iced_graphics` patch)

Scrolling is not quantized to lines. The wheel and the scrollbar thumb both
move the view by **pixels**, so it comes to rest wherever the input asked -
the topmost row clipped part-way by the top edge, every row below it a
whole line further down.

This needs one thing iced doesn't ship, so the workspace root carries a
`[patch.crates-io]` pointing `iced_graphics` at
`j1mmie/iced`, branch `jumppad/fractional-scroll`. The branch is the
`0.14.0` **tag** plus a single additive method:

```rust
pub fn scroll_by(&mut self, pixels: f32)
```

Three things about that patch are load-bearing:

- **Branched from the tag, not `master`.** Master is `0.15.0-dev`, and a
  patch has to keep satisfying the `^0.14` requirements every other iced
  crate states or Cargo rejects it outright.
- **Only `iced_graphics` is changed.** Widening `Action::Scroll` to a float
  instead would drag `iced_widget` into the patch set, and that cannot work
  here: the `0.14.0` tag has `iced_widget` at 0.14.0 while crates.io ships
  0.14.2, so the patched version wouldn't satisfy `iced`'s requirement.
  Keeping `Action::Scroll`'s `i32` alone is what keeps the blast radius to
  one crate.
- **`iced_core` and `iced_futures` are patched too, carrying no changes.**
  `iced_graphics` depends on its workspace siblings by *path*, so patching
  it alone pulls a second `iced_core` out of the fork while everything else
  keeps the registry copy - two `iced_core::Font` types that don't unify,
  and a wall of "expected X, found X". Check with
  `grep -c '^name = "iced_core"' Cargo.lock`; the answer must be 1.

Inside JumpPad the pixel path is deliberately *separate* from the
whole-line one, because both are still wanted:

- `Content::scroll_by(pixels)` - the wheel and the thumb, where the user is
  pointing at a position. Reaches the widget through its own
  `TextEditor::on_scroll` callback and `EditorMessage::Scroll(f32)`, since
  `Action` has no variant that can carry a fraction. `on_scroll` is
  optional; with no handler set the widget falls back to upstream's
  whole-line behavior, which is what `State::partial_scroll` is still there
  for.
- `Action::Scroll { lines }` - the cursor reveals in `shape_and_reveal`,
  which count in lines and mean it. Note these *preserve* any sub-line offset
  rather than re-snapping to a boundary, so a reveal never visibly
  straightens a view the user left between two lines.
- `scroll_by` again, also in `shape_and_reveal`: putting a rebuilt
  `Content`'s view back where the replaced one had it. That one is a
  position, not a count. Rounding it to whole lines is what used to snap a
  bottom row the user left cut off flush against the edge, on every undo and
  every line command.

### Why scrolling used to land on whole lines

Kept because it explains what the patch above is buying, and because the
same reasoning applies to anyone tempted to drop it.

The view used to always start a line at its top edge. That was **never** a
limit of the text control. cosmic-text scrolls by pixels
(`cosmic_text::Action::Scroll { pixels: f32 }`), keeps the sub-line
remainder in `Scroll::vertical`, and subtracts it from every layout run's
`line_top`. The test `the_buffer_can_sit_between_two_lines` pins that down
directly: shape a view that isn't a whole number of rows tall, scroll to the
end, and the buffer comes to rest half a line down and renders there.

The quantization was entirely in the way *in*. `iced_core`'s
`text::editor::Action::Scroll` carries whole `lines: i32`, and
`iced_graphics::text::Editor::perform` multiplies that by the line height on
its way to cosmic-text - so the smallest step iced could express was one
line, and `Editor` exposed `buffer()` immutably and nothing else, leaving no
public route to `set_scroll` either. `scroll_by` is exactly that missing
route, and nothing downstream needed changing: the renderer, `scrolled_to`,
and the scrollbar's `metrics` were all fractional already, which is why the
thumb was smooth long before the text was.

Checked against `iced-rs/iced` master on 2026-08-04 before forking: still
whole lines there, and `[Unreleased]` was empty. Master's own
`buffer.set_scroll` calls are the *horizontal* scrolling work and don't help.
**If a released iced ever grows a fractional lever of its own, drop the
patch and the fork and use it** - that is the whole reason `scroll_by` was
written as an additive, upstream-shaped method rather than a local hack.

Two things not to reach for if the patch ever has to go away:

- Scaling `LineHeight` to fake smaller steps. `set_metrics` resets shaping
  for the whole document.
- Offsetting the draw instead - keeping whole-line scroll in the buffer,
  tracking the remainder here, and shifting `fill_editor`'s position by it.
  It works without any dependency change, but the remainder then has to be
  added back into `Action::Click`/`Drag` positions, the cursor and selection
  quads, and the scrollbar position, and the buffer has to be laid out a row
  taller than the view or the bottom edge gaps. Strictly more moving parts
  than one patched method.

### Revealing the cursor after a change

A change made while the cursor is off screen (scrolled away with the wheel or
the thumb, then typed into) brings the cursor back into view with
`REVEAL_MARGIN_LINES` of context past the edge it came in from - it lands on
the sixth visible line, from the top or the bottom depending on which way the
view had to move. Typing with the cursor already visible doesn't scroll at all.
A change that rebuilds the document - undo, redo, the line commands - treats
that margin as a *band* instead: it scrolls once the cursor is inside the
margin and still heading for that edge, rather than waiting for it to leave
the view. **Heading for it** is half the rule. A cursor sitting in the bottom
margin that just moved *up* is walking away from the edge and gets nothing;
scrolling down to "reveal" it would drag the view the opposite way to the line
the user is moving.

Everything happens in `shape_and_reveal`, on the next `layout`, because that
is where the shape happens: cosmic-text reveals a moved cursor lazily, inside
the shape, long after `Content::perform` returned, and a rebuilt `Content` has
no line metrics to scroll by until it has been shaped once either. So the
change records where the view sat (`Internal::pending_view`) and `layout` acts
on it after shaping. Corrections that count lines go through `Action::Scroll`
and the one that restores a position goes through `scroll_by` (see the patch
section above); either way each needs a shape of its own to settle before the
frame draws.

The two variants of `PendingView` come at the same result from opposite ends:

- `Edited` - an `Action` that edited in place. cosmic-text has already chased
  the cursor, but by the bare minimum, leaving it hard against the edge, so
  `reveal_offset` only adds the margin.
- `Rebuilt` - undo/redo and the line commands, which replace `Content`
  wholesale (see the undo history section). The fresh buffer starts at the top
  of the document, so the first shape reveals the cursor from *there* - a view
  the user was never at. `layout` scrolls back to where the old `Content` had
  it and then places the cursor itself with `restore_offset`, since nothing is
  going to chase it a second time.

  Because it places the cursor outright, this path can hold the margin as a
  band: `restore_offset` scrolls once the cursor is nearer an edge than
  `REVEAL_MARGIN_LINES` *and* moved that way, and holds still otherwise.
  Firing only once the cursor was fully off screen is what made a held line
  command stutter - the caret crept onto the last visible row with nothing
  under it, the view sat there, and then it jumped a whole margin at once when
  the caret finally crossed. The scroll needs no clamping of its own;
  `Action::Scroll` already stops at both ends of the document.

  Which way the cursor moved is why `PendingView::Rebuilt` carries a whole
  `CapturedView` - the replaced `Content`'s scroll *and* its cursor row - and
  why `Content::capture_view` is the counterpart to `restore_view`. The
  margins say the cursor is running out of context; the row it came from says
  which side that context is on. A cursor already off screen is placed
  whichever way it went, having none either side.

  `capture_view` returns `None` until `layout` has shaped the `Content` once
  (`Internal::shaped`). The cursor's row comes from `editor.selection()`,
  which panics on a line cosmic-text has not cached a layout for - and a
  `Content` nobody has laid out has no view worth carrying across anyway.

**A view that moved is not proof the cursor was chased.** Deleting lines under
a view anchored at the end of the document clamps the scroll upwards on its
own, with the cursor still comfortably on screen; backing *that* off by the
margin would push the cursor out of view. `reveal_offset` only fires when the
view moved *and* the cursor came to rest on the first or last visible row,
which is where a real reveal - and nothing else - leaves it. The `Rebuilt`
path has no such ambiguity: it measures the cursor's row against the restored
view directly.

**Don't route undo through `Action::Edit`.** Replacing the document with
`SelectAll` + `Paste` would reuse the `Edited` path for free and keep the
buffer's scroll, so it looks like the obvious simplification. It is quadratic:
measured against `Content::with_text` on the same document, 2x slower at 10K
lines, 17x at 50K, and 35x at 150K (237 seconds). `Content::with_text` plus a
restored view is the cheap way.

### The source cache

`TextArea` keeps the document's full text in an `Arc<String>` (`source`) and
rebuilds it *only* when an edit changes the text. Two things read it on the
per-redraw path, and neither may do work proportional to document size:
`view` hands it to `HighlighterSettings`, and the widget's `layout` compares
those settings against the previous frame's to decide whether to re-run the
highlighter.

**`view` must never call `Content::text()`.** That reassembles the whole
document line by line, and `view` runs on every redraw - including redraws
caused by nothing but a mouse move mid-selection-drag, where the text
provably has not changed. It measured at ~18ms per redraw on a 150K-line
file in a release build (`opt-level = "z"`, as shipped), past a whole 16.7ms
60fps frame budget before anything is painted. With the cache it is ~350ns
and flat - constant in document size rather than linear.

**`HighlighterSettings::eq` compares source *pointers*, not bytes.** A byte
compare is the same linear cost, on the same per-redraw path. This is sound
only because `resync_source` is the single place that mints a new `Arc`, and
it runs on exactly the operations that change text: an `Action` where
`is_edit()`, undo/redo, and `set_text`. Every other action (`Move`,
`Select`, `Click`, `Drag`, `Scroll`) leaves the cache valid, which is what
keeps a selection drag off the rebuild path. The failure directions are
asymmetric: two equal-but-separately-allocated strings compare unequal and
cost one redundant reparse, where a missed change would leave stale
highlighting on screen.

**Add a `resync_source` call to any new code path that mutates the text.**
The tests in `lib.rs` assert `source` matches `content.text()` after every
mutating operation, so drift shows up as a failure rather than as
mysteriously misaligned syntax colors - the highlighter resolves byte
offsets against the cached string, so a stale cache misaligns every span
after the point it diverged. Deliberately *not* a `debug_assert`: that
would re-introduce the linear rebuild in dev builds, where it was worst
(~64ms per redraw at 150K lines).

### Undo history

`History` (`history.rs`) is a snapshot stack: each entry is the whole document
text plus a `CursorState` (caret position *and* selection) as of just before an
edit. `TextArea::update` records one on any `Action` where `is_edit()`, and
`apply_history` restores both halves - replaying the selection through
`restore_selection` rather than `move_cursor_to`, which clears one.

**Undo restores the selection, for every edit - typing included.** This is one
uniform rule, deliberately, matching VS Code: Monaco's `EditStack` restores
`beforeCursorState` on undo for all edit operations, not just cut and paste. So
selecting a word, typing over it, and undoing brings the word back *selected*.
Not a bug. Two things follow that look odd but are the same rule: undoing
Option/Ctrl+Backspace re-selects the deleted word (`word_delete_backward` is a
`Binding::Sequence`, so the `Select` half is already applied when the
`Backspace` records its caret), and a cut is indistinguishable from pressing
Delete with a selection - both publish `Edit::Delete`, so a cut-only variant of
this rule was never expressible.

**Coalescing keeps the *first* snapshot of a burst, not the last.** That is what
makes the restored selection the one the first keystroke replaced; overwriting
on each coalesced edit would destroy it, since keystrokes after the first have
no selection to record.

**`apply_history` carries the view across, not just the caret.** The rebuilt
`Content` starts at the top of the document, so without `restore_view` an undo
of an edit already on screen would still jump the document around. See the
reveal section above for what `layout` then does with it.

**Toggle-comment and the line commands ride the same replacement path.** The
transforms are pure functions in `comment.rs` and `lines.rs` (testable without
a window); `TextArea` applies them through `text_with_lines_spliced` +
`replace_document` - the `Content::with_text` + `restore_view` tail shared
with undo/redo, never SelectAll+Paste (see the quadratic warning above). Each
records via `History::record_isolated`, which resets the coalescing burst on
both sides so it neither joins the typing burst before it nor absorbs the
keystroke after. A command that turns out to be a no-op (move-line at the top
of the document, delete-line on an empty one) must return `false` *before*
recording: `record_isolated` clears the redo stack unconditionally, so a
phantom entry would silently throw a redo away.

**A spliced-in line inherits the ending of the line it displaces.** Only the
last line carries `LineEnding::None`, and `Content::text()` drops it - so a
line promoted into the last position, or a copy landing past it, has nothing
to inherit and borrows `document_line_ending()` instead. Without that, moving
or duplicating the last line of a CRLF file quietly splices in a lone LF.
`text_with_lines_spliced` also can't ask `lines.peek()` whether it's on the
last line the way `Content::text()` does - the output line count differs from
the input's - which is what `LineJoiner`'s up-front `total` is for.

**Redo's caret is the state at undo time, not at edit time.** VS Code records an
`afterCursorState` when the edit happens; JumpPad reuses whatever is live when
you press undo. Only observable if the caret moved in between. Left as-is - the
fix wants an `after` field on `Snapshot` and its own coalescing tests.

## Syntax highlighting (`syntax_registry`)

- Grammars are tree-sitter parsers compiled to WASM, run through
  `wasmtime` - not loaded as native shared libraries. This trades some
  speed for sandboxing untrusted grammar code and for being trivially
  removable (delete the `.wasm`, the feature just isn't there for that
  file type - no code path notices or cares).
- Grammars load **lazily and asynchronously**: nothing loads at startup;
  the first tab that opens a file of a given type spawns a background
  thread (`SyntaxRegistry::acquire` -> `finish_load`) to find and compile
  the matching `.wasm`. A tab's content is never blocked waiting on this -
  it just renders unhighlighted until the grammar resolves.
- Grammars are cached and refcounted by *grammar name*, not file
  extension (`yaml`/`yml` share one grammar). Dropping the last `Handle`
  referencing a grammar evicts it. Injection targets (e.g. embedded YAML
  inside Markdown, via `<grammar>.injections.scm`) are just more grammars
  acquired recursively through the same path.
- **Gotcha - an injection target loads *after* the grammar that injects
  it,** so a grammar going `Ready` is not the end of the story. Markdown is
  the visible case: `markdown.wasm` resolves first and colors headings and
  code fences, but everything inline (links, bold) comes from
  `markdown_inline.wasm`, acquired only once markdown itself has finished
  loading. Nothing about the tab changes when it lands - the grammar `Arc`
  is the same object - so two things have to notice it, and both are
  required:
  - `SyntaxRegistry.revision` counts resolved loads and rides along in
    `HighlighterSettings`. iced re-runs a `Highlighter` only when its
    settings compare unequal, and `source`/`grammar` don't move here; without
    the revision the incomplete first parse stayed on screen until an
    unrelated edit changed `source` (typing anywhere above the text was
    enough - that's the bug's signature). Registry-wide, not per grammar, so
    it covers injections nested any number of levels deep.
  - `Grammar::injections_unresolved` keeps `has_pending_highlighting` - and
    so the 50ms poll subscription - alive until the targets land. It ORs the
    flag recorded by the last `highlight` with a live check of the injected
    handles, because the subscription is re-evaluated *before* the first
    `view` after the grammar goes `Ready` (`iced_winit`'s `update` re-tracks
    recipes at the end of the message pass; `view` runs later, on the
    redraw). At that instant the recorded flag is still `false`, so the timer
    would shut off before anything had parsed.
- **An injected span overrides only the bytes it actually colors,** not the
  whole injected region. `# Title`'s text is an injection target, but the
  inline grammar has nothing to say about plain words - subtracting the
  region wholesale left the heading color on `# ` alone, so a heading
  visibly *lost* color the moment the inline grammar loaded.
- **Gotcha - compile serialization:** `SyntaxRegistry.compile_lock`
  serializes the actual wasmtime compile step across all background
  loader threads. This was not a defensive guess - concurrent first-time
  compiles through the same `wasmtime::Engine` were reproduced to hang
  indefinitely. Grammars can still be *requested* concurrently (refcounting
  in `state` is independent), only the compile itself is serialized.
- **Gotcha - lock reentrancy on eviction:** `SyntaxRegistry::release` drops
  the removed `Grammar` (which cascades into releasing any injected
  grammars' `Handle`s, which calls back into `release`) only *after*
  releasing its own mutex guard. Freeing it while still holding the lock
  self-deadlocks the moment any grammar with injections is evicted - also
  reproduced directly, not theoretical.
- Highlight categories (`HighlightCategory`) are a small fixed set (String,
  Comment, Number, Keyword, Heading, Emphasis, Link, Quote, Code) chosen to
  cover both code-like and markup-like grammars without a full
  scope/theme system. Don't expect fine-grained scopes here.

## Rendering backend

Which iced compositor backend gets compiled in is a build-time choice, not
a runtime one - `iced`'s `wgpu` and `tiny-skia` features are mutually
exclusive per binary (see `crates/jumppad/Cargo.toml`'s `[features]` and its
two `[[bin]]` entries). This produces two binaries from the same
`jumppad` package:

- `jumppad` - `tiny-skia` (default), pure software, ~22MB idle vs. ~146MB
  for `wgpu` in this app.
- `jumppad-gpu` - `wgpu`, hardware-accelerated.

Each `[[bin]]` has `required-features` set to the matching Cargo feature,
so plain `cargo build`/`cargo run` (default features) only ever touches
`jumppad`; building `jumppad-gpu` requires
`--no-default-features --features wgpu` explicitly (see
`scripts/build-release.sh`/`.ps1`). Since only one backend is ever
compiled into a given binary, there's no `ICED_BACKEND` env var or other
runtime selection to worry about - `iced_renderer` picks its `Renderer`
type solely from which feature(s) are active (both features enabled at
once, as in the old single-binary setup, would compile in a
runtime-switchable fallback compositor instead - not the case here).

**Gotcha - on macOS, transparency requires `jumppad-gpu`.** `softbuffer`'s
CoreGraphics backend builds its `CGImage` with
`CGImageAlphaInfo::NoneSkipFirst` (`softbuffer-0.4.8/src/backends/cg.rs`),
which discards the alpha channel outright. The `jumppad` (tiny-skia) binary
therefore cannot produce a translucent window on macOS at all, no matter
what `[alpha] background` says - it renders the alpha and CoreGraphics
throws it away. Anything transparency-related reported from macOS is by
definition the wgpu path, so don't debug it against `iced_tiny_skia`'s
compositor (this mistake has already been made once).

Windows has no such restriction - both binaries are translucent there. It
is still worth establishing which binary a transparency report came from
before debugging it, since they reach the screen by completely different
paths.

**Gotcha - macOS burns in a snapshot of the window taken at resize.** With
`jumppad-gpu` on a translucent window, content from an earlier moment stays
visible *behind* the live content. Established by testing, so don't
re-derive it:

- Its strength tracks `1 - background_alpha`, so it is *behind* what we
  draw, not something we draw.
- It is frozen, not accumulating. Forcing 128 extra frames does nothing;
  if each frame composited over the last, it would decay as `0.6^n`.
- It is a snapshot of the window as it looked at **the last drag-resize**.
  Resize, and it matches the live content so nothing looks wrong; change
  tabs, and the old content shows through.
- `iced_wgpu` is not the culprit: `present()` always passes
  `Some(background_color)`, so every frame does `LoadOp::Clear` over the
  whole surface. It has no damage tracking at all.

- It is not the window frame either: reproduced identically with
  `[window] decorations = false`, so it isn't the titlebar's view hierarchy.
- It is not in *any* layer in the process: a recursive dump of the whole
  window's layer tree showed every layer clean while the ghost was on
  screen.

The cache is the **window server's shadow-content cache** - the window
server keeps a copy of a translucent window's content to derive its shadow,
and composites that copy behind the window. It lives outside the process,
which is why no surface clear, layer clear, or redraw ever touched it.
Confirmed by elimination: `setHasShadow: NO` kills the ghost outright.

**The fix** (`crates/jumppad/src/macos.rs` + `app.rs`): keep the shadow, call
`NSWindow.invalidateShadow` after the content changes - but only after the
new frame has *presented*. Calling it the moment the tab switches re-caches
the outgoing frame and fixes nothing (tried, confirmed useless);
`shadow_refresh_frames` counts `SHADOW_REFRESH_FRAMES` presented frames
(via `iced::window::frames()`, gated like the other subscriptions) before
invalidating. Armed on tab switch, tab creation, and window resize.

One oddity found along the way, currently left alone: the layer dump showed
**two** `WgpuObserverLayer`s under the view's root layer - the older one a
leftover from the adapter-probe surface, `opaque=true`, stacked behind the
live one. It is visually inert (transparency worked correctly with it
present and un-hidden), so the code that hid it was removed in cleanup; if
an opaque-rectangle artifact ever appears on macOS, that layer is the first
suspect, and the hiding code is in git history.

Dead ends, so they aren't re-attempted (each confirmed no-op on a real
machine):

- **`NSWindow.preservesContentDuringLiveResize = NO`.** Sounds like exactly
  this bug; changed nothing.
- **`layerContentsRedrawPolicy` / clearing the root layer's `contents`.**
  The root layer never held the snapshot. (If a policy change is ever
  needed: not `...RedrawNever` - winit's view drives `handle_redraw` from
  `drawRect:`, so that stops the app painting.)
- **A programmatic 1px resize** (a "resize kick", removed in cleanup) does
  clear the ghost, but only as a side effect of the swapchain rebuild
  forcing the window server to retake its snapshot - it re-records a new
  one, so the ghost returns on the next content change.
- **`invalidateShadow` at content-change time.** Right lever, wrong moment;
  it must run after the new frame presents.

Removed along with the kick: its startup trigger on Windows, which existed
for a "wgpu renders fully opaque until a real resize" report from there.
The original implementation collapsed the window to zero height, which
never produced a resize at all (clamped/dropped), so that workaround had
been a no-op from the start. If the Windows bug resurfaces, git history has
the working 1px version.

**Also on this path - an alpha-convention mismatch, fixed app-side.** Metal
offers only `[Opaque, PostMultiplied]` and `iced_wgpu` picks
`PostMultiplied`, but that label is wrong upstream: the macOS window server
composites a `CAMetalLayer` as **premultiplied** alpha regardless. iced's
quad and glyph shaders premultiply before writing, so they come out right.
The *clear color* goes through `Color::into_linear()`, which does *not*
premultiply - and since the editor and active tab deliberately paint no
quad (see the transparency section below), the clear color is most of the
window. Composited as `src_rgb + (1 - a) * desktop`, a straight near-black
background (rgb ~ 0) happens to look right, but a straight white one
saturates to solid white at any alpha - light themes rendered as a fully
opaque window while dark themes looked fine, which is how this shipped
unnoticed. (An earlier revision of this file reasoned from the
`PostMultiplied` label and predicted the opposite - bare regions right,
quad regions too dark - and called it unfixable from here. The
solid-white-window evidence settled it.) **The fix** (`premultiply` in
`app.rs`): `JumpPadApp::style` premultiplies the translucent background color
itself. Gated on `CLEAR_COLOR_NEEDS_PREMULTIPLY` (also in `app.rs`) rather
than applied unconditionally, because `tiny-skia` premultiplies internally
and feeding it a premultiplied color would double-darken.

**Windows' DWM has the same convention, so it is in the gate too.**
Confirmed by the same symptom, reported from Windows with side-by-side
`jumppad`/`jumppad-gpu` screenshots: a *light* theme rendered opaque on
`jumppad-gpu` at any alpha - including 0.1 - while a dark theme at the same
alpha looked nearly identical to the software build. `transparent(true)`
gets per-pixel alpha there through winit's `DwmEnableBlurBehindWindow`, and
DWM's composition model is premultiplied. So `CLEAR_COLOR_NEEDS_PREMULTIPLY`
covers macOS *and* Windows on `wgpu`.

**Linux is the one holdout.** Wayland and compositing X11 are premultiplied
as well, so it very likely belongs there too, but nobody has reproduced the
symptom on either. Turning it on blind would cost real opacity on a window
that currently looks right; wait for a screenshot - **of a light theme**,
per the diagnostic below.

**Gotcha - the premultiply is per sRGB-encoded channel, not linear.** The
window server composites on *encoded* values, and
`encode(linear * a) > encode(linear) * a`, so premultiplying in linear
space before the surface's sRGB encode over-brightens: white at alpha 0.1
stores as `encode(0.1) ~ 0.35` and showed the desktop through a wash ~3.5x
brighter than configured (confirmed on a real machine; dark themes hide it
since 0 encodes to 0). This also means iced's own shaders - which
premultiply in *linear* space - land slightly bright under this compositor,
but every translucent quad this app paints is black (`darkening_wash`, the
modal scrim), and black is immune, so nothing visible is affected.

**Gotcha - the Windows redirection surface starts opaque, and stays that way
until something reallocates it.** The third and last of the Windows
transparency bugs, and the one that actually made `jumppad-gpu` look opaque
at startup even at `background = 0.1`.

Every Win32 window DWM composites has a **redirection surface** behind it,
and per-pixel alpha compositing reads *that surface's* alpha channel - not
the swapchain's. Nothing initialises it in this stack:

- winit registers its window class with `hbrBackground: 0`, a null brush, so
  the surface is never painted. Win32 references on per-pixel alpha
  specifically prescribe `BLACK_BRUSH` here, exactly so the surface starts
  all-zero (and therefore fully transparent).
- winit does call `DwmEnableBlurBehindWindow` - the call that asks DWM to
  honour per-pixel alpha at all - but from `on_create`, before any swapchain
  exists. Same "right lever, wrong moment" shape as the macOS shadow cache.

The diagnostic that identifies this one specifically: **resize the window
smaller and back, and the region the resize touched becomes correctly
translucent while the rest stays opaque.** A reallocated surface is a zeroed
surface. That is also why the old "resize kick" hack worked, and it is a
known issue outside this project - the same white-until-resized artifact is
reported against wgpu with decorated windows, with "use a borderless window"
as the going workaround (which is exactly why `decorations = false` appears
to fix transparency here).

**The fix** (`reset_redirection_surface` in `crates/jumppad/src/windows.rs`):
fill the client area with the black brush that should have been the class
background, then re-issue `DwmEnableBlurBehindWindow`. No size change, so no
visible kick. Armed on `WindowReady` and fired `SURFACE_RESET_FRAMES`
presented frames later, for the same reason the macOS shadow refresh waits -
doing it before a real frame has presented is what fails today.

`tiny-skia` never needed this: it presents by blitting through the
redirection surface every frame, so it initialises it as a side effect of
drawing. Another reason comparing the two binaries has been misleading
throughout - they do not reach the screen by the same route.

**Gotcha - Windows 11 paints its own backdrop behind a decorated window.**
Separate bug from the premultiply above, found while chasing it, and the two
stack: on `jumppad-gpu` with `[window] decorations = true` a translucent
window came out far brighter and more solid than configured even after the
premultiply landed. `[window] decorations = false` fixed it outright, which
is the whole diagnosis - an undecorated window has no frame for DWM to hang
a backdrop on.

winit calls `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE, ...)`
unconditionally at window creation, with whatever
`WindowAttributes::platform_specific.backdrop_type` holds. iced never
surfaces that field, so it stays at winit's default `BackdropType::Auto` =
`DWMSBT_AUTO`, "let DWM pick" - and on Windows 11 DWM's pick for a decorated
window is a Mica-style material drawn behind the client area. The window's
alpha then reveals *that*, not the desktop. Because the material is a light,
wallpaper-derived wash, a light theme is hit hardest, which is exactly the
confound that made the premultiply bug so hard to read: both symptoms are
"too bright, worst on light themes".

**The fix** (`crates/jumppad/src/windows.rs`, wired from
`JumpPadApp::disable_system_backdrop` on `WindowReady`): re-set the attribute
to `DWMSBT_NONE` once the window exists. Only on a translucent window - on a
solid one the backdrop is hidden anyway, and turning it off would be a
gratuitous difference from every other Windows app. Older Windows returns a
failure `HRESULT` and changes nothing, which is already the desired
behaviour there.

`tiny-skia` never showed this: softbuffer presents through a GDI blit into
the window's redirection bitmap rather than a flip-model swapchain
composited by DWM. That difference is also why comparing the two binaries
was misleading for so long - they do not reach the screen the same way.

**Always test the premultiply bug with a light theme.** It is invisible on a dark one,
and that asymmetry has now cost two wrong diagnoses. `src_rgb + (1 - a) *
desktop` saturates the channel from `src_rgb` alone once `rgb` nears 1, so
alpha stops mattering completely: a light theme reads as a fully opaque
window at *every* configured alpha, 0.1 included. A near-black theme
(`rgb ~ 0`) composites almost correctly regardless. Two consequences worth
internalising:

- "Dark themes look close, light themes are opaque" is the signature of
  **this** bug - a missing premultiply.
- "Everything is uniformly too dark, including dark themes" is the
  signature of the *opposite* mistake - premultiplying on a path that
  didn't need it.

Do not conclude anything about transparency from a dark-theme screenshot.
Ask for a light one, and preferably a low alpha, where the two hypotheses
diverge most.

**Red herring - `wgpu`'s DX12 surface reports `composite_alpha_modes:
[Opaque]` on Windows, and that does *not* mean the window is opaque.** This
was misread once as "Windows can't do transparency on `wgpu` at all", and a
working fix was reverted on the strength of it. The reading of the sources
is accurate; the conclusion drawn from it was not:

- `wgpu_hal::dx12::Adapter::surface_capabilities` really does return
  `composite_alpha_modes: vec![Opaque]` for `SurfaceTarget::WndHandle`, and
  the default `Dx12SwapchainKind::DxgiFromHwnd` really is documented as
  *"This does not support transparency."*
- `iced_wgpu::window::Compositor::request` therefore falls through its
  `PostMultiplied`/`PreMultiplied` preferences to `Auto`.
- But that field governs **DXGI's own composition alpha mode**, which
  applies to composition swapchains. It is not the mechanism JumpPad's
  transparency runs through. The window is translucent because winit's
  `transparent(true)` calls `DwmEnableBlurBehindWindow`, which makes the
  **DWM** honour the per-pixel alpha in the window's redirection surface -
  and the alpha `iced_wgpu` clears to still lands in those pixels.

So the DXGI capability list says nothing about whether the presented alpha
survives, and the empirical answer is that it does. If this needs settling
again, the discriminator is a light theme at low alpha: opaque *and* pure
saturated white means the alpha arrived and wasn't premultiplied; opaque at
the theme's own background colour would mean the alpha really was dropped.

(`Dx12SwapchainKind::DxgiFromVisual` and the DirectComposition path remain
unreachable from app code - `iced_wgpu` builds its `InstanceDescriptor` with
`..Default::default()`, and the `WGPU_DX12_PRESENTATION_SYSTEM` env var is
dead, read only by `Dx12SwapchainKind::from_env`, which nothing in `wgpu`,
`wgpu-core`, or `wgpu-hal` calls. That is all true and all irrelevant.)

## Why the software renderer isn't compiled at `opt-level = "z"`

`[profile.release]` uses `opt-level = "z"` for binary size, but
`[profile.release.package]` pulls the per-frame drawing crates back up to `3`.
Measured on a 1800x1200 surface (a default window at 2x), clearing plus 40 rows
of translucent quads: **43.6ms/frame at "z", 12.7ms at 3** - 23fps vs 79fps, for
one repaint. tiny-skia says why in `src/wide/u16x16_t.rs`: its blend pipeline is
plain `[u16; 16]` arrays that rely on autovectorization, which `-Oz` turns off,
and `#[inline]` hints it calls mandatory, which `-Oz` declines. `jumppad-gpu`
never runs any of it, which is why the two binaries felt so different.

Raising it costs ~320KB of binary. If a crate ever shows up hot on the draw
path, add it to that list. `-C target-cpu=x86-64-v3` would help further on Intel
(tiny-skia's `f32x8` only uses AVX under `target_feature = "avx"`), but keep it
out of `.cargo/config.toml` and `build-release.sh` - it produces binaries that
crash on older CPUs, and moot on Apple Silicon where NEON is baseline.

## macOS: softbuffer disables damage tracking entirely

`iced_tiny_skia`'s `present` only diffs damage when it can identify the previous
buffer, via `buffer.age()`; otherwise it falls back to
`vec![Rectangle::with_size(viewport.logical_size())]` - the whole window.
**softbuffer's CoreGraphics backend hardcodes `age() -> 0`**
(`src/backends/cg.rs`), because it has nothing to age: it allocates a fresh
zeroed framebuffer every frame and hands it to a `CGDataProvider`.

So on macOS there is no damage tracking. Per-frame cost is proportional to
**window area, not to what changed**, and a maximized Retina window is the worst
case. Selection drags and scrolling show it first, since both redraw in bursts -
one per mouse-move and one per wheel step. Windows and X11 return `1` once
presented and do get real diffing, so this is macOS-only.

Fixing it means forking softbuffer for buffer reuse plus a real `age()`. That is
the next big lever if the renderer still feels slow; nothing in JumpPad can work
around it.

One consequence for the "zero idle CPU" goal in `README.md`: it holds for
JumpPad's *scheduling* (see the scrollbar's `next_redraw`), but on macOS the
500ms caret blink costs two full-window repaints per second whenever the window
is focused.

## Known upstream rendering bug (tiny-skia + tab switching)

`iced`'s tiny-skia compositor skips presenting a frame it thinks looks
identical to the last one (a damage-tracking optimization). Its equality
check for a `text_editor`'s rendered content
(`iced_graphics::text::editor::Internal::eq`, in the `iced` crate itself,
confirmed still present as of iced 0.14.0) only compares font, bounds, and
line metrics - **never the actual text**. Switching JumpPad's active tab
lands on a different editor with identical font/bounds/metrics (same
pane), so the compositor concludes nothing changed and skips the repaint -
the old tab's text stays on screen until some unrelated redraw (hovering
a button, resizing the window) happens to touch that region for real.

There is no `redraw()`/`invalidate()`/dirty-flag hook exposed to
`Program::update` or `view` for this specific per-widget check - but there
*is* a coarser, application-reachable bypass one level up. Before
`iced_tiny_skia` even runs the per-widget check above, it compares the
whole frame's reported background color against last frame's
(`surface.background_color == background_color` in
`iced_tiny_skia::window::compositor::present`); if that differs at all, it
skips the per-widget check entirely and repaints the *entire* viewport.
That's the lever JumpPad actually uses (see below). This was also not fixed
by switching to `wgpu`-only (that backend has no damage-tracking at all
and isn't a real option here anyway - `tiny-skia`'s memory footprint is
the whole point of this project, see `README.md`).

**The fix in place:** `JumpPadApp::redraw_nudge_frames: u8` is set to
`REDRAW_NUDGE_FRAMES` every time the active tab changes (`switch_active`
and `new_tab`, `crates/jumppad/src/app.rs`). `JumpPadApp::theme()` checks it:
while it's non-zero, it returns the app's real theme run through
`nudge_background`, which drops the palette's background alpha by
`0.001 * frames_left` and rebuilds it as `Theme::custom(...)`. That's an
`f32` difference big enough for `Color`'s `==` to see, but small enough
to be invisible - the background quad it's drawn as blends against a
backdrop the compositor already cleared to that same base color moments
earlier, and mixing a color with itself at less than full opacity still
produces that same color.

**Gotcha - one nudged frame is not enough.** `softbuffer` presents through
several buffers in rotation, and `present()` only ever draws into the one
it's handed (`buffer_mut()`), using `buffer.age()` to decide what that
buffer already contains. Repainting a single frame therefore fixes exactly
one buffer; the others still hold the *previous* tab's text and come back
around a frame or two later, which is the faint ghost text seen on macOS.
Scaling the nudge by `frames_left` is what makes each frame of the
countdown a *different* color, so all of them repaint instead of just the
first - a plain flip-flop bool gives the same value on consecutive frames
and the compositor goes straight back to its damage check. The countdown
is driven by `iced::window::frames()` (real presented frames, not a
wall-clock tick - buffer rotation is counted in frames), gated on the
counter being non-zero so there's no idle cost, same as the other timers
in `subscription()`. If ghosting ever reappears on a platform with a
deeper swapchain, raise `REDRAW_NUDGE_FRAMES`; that's the knob.

**Gotcha - `switch_active` no-ops when the index doesn't move,** so it
can't be the only thing arming the nudge. Two paths land on an unchanged
index with a brand-new editor underneath: the first tab at startup
(`tabs` starts empty, `active` is already 0) and the replacement for a
closed last tab (`close_tab` -> `new_tab`). Both went unpainted *and*
unfocused before `new_tab` learned to arm the nudge itself.

Two things were tried and found insufficient before landing on this,
worth knowing so they aren't re-attempted as if untested:
- A per-tab `bool` toggling one pixel of container padding on the active
  editor (changing its `Internal.bounds`, one of the three fields the
  per-widget check *does* compare) - worked, but visibly janky (a
  perceptible 1px shift each switch).
- Re-focusing the editor on switch (still done today - see below for why)
  on the theory that it reproduces what a real click does - confirmed
  insufficient on its own; content still went stale.

**Also in `switch_active`, for an unrelated reason:** it saves the
outgoing tab's cursor and selection (`Tab::last_cursor`/`last_selection`)
and restores the incoming tab's, then re-focuses the editor. This is
needed regardless of the redraw bug above, because of the `keyed_column`
(keyed by `Tab::id`, not `Vec` index) around the active editor in
`view()` - it makes a freshly switched-to tab's editor widget state start
completely fresh (unfocused, no highlighter/click/drag state left over)
rather than silently reusing the previous tab's, so without restoring
focus/cursor/selection by hand, the caret and selection would simply be
invisible and typing would resume from the document start instead of
wherever the user left off.

**Gotcha - re-focusing must target the editor's stable widget id**
(`focus_editor` in `app.rs` / `editor_core::EDITOR_WIDGET_ID`), not
`focusable::focus_next`. `focus_next` moves focus to the widget *after*
whichever is focused when the operation runs; on a keyboard-driven switch
(Ctrl+Tab, Ctrl+N) the outgoing editor is still focused at that moment -
unlike a mouse switch, where the chip click unfocused it first - so
`focus_next` skipped the editor and the new tab came up unfocused: typing
dropped, caret and any restored selection invisible (iced's `text_editor`
only draws its selection while focused). Reproduced under Xvfb, fixed by
focusing the id directly.

**Selection save/restore fidelity:** double/triple-click selections are
stored by cosmic-text as `Selection::Word`/`Line` with the *anchor at the
click position* - `Content::cursor()` reports `selection == position` for
them, with the bounds implied by the kind. `TextArea::selection`
tells them apart from a collapsed leftover range by whether selected text
exists (and picks Word vs Line by comparing it to the anchor's line), and
restore replays `SelectWord`/`SelectLine` at the anchor rather than
setting an anchor-to-cursor range. Don't "simplify" the saved state back
to a bare anchor pair - that's exactly the representation that can't
describe a word selection. The undo history stores the same
`SavedSelection` for the same reason (see "Undo history").

## The find palette

Cmd+F / Ctrl+F opens a floating palette at the top-right of the editor
area (`JumpPadApp::find_palette`, stacked over the editor so it never
covers the tab bar). Search is case-insensitive, single-file, no toggles.

Cmd+G / Ctrl+G is find-again, Cmd+Shift+G steps backwards. Both work with
the palette open or closed, and with focus in either the editor or the
query field. With the palette **closed** they deliberately do not reopen
it - so they re-search first (the document can have changed while the
palette was shut) and re-anchor to the cursor rather than to wherever the
palette was left. Closed also means the editor holds focus, so the match
shows as an ordinary selection and `select_current_match` skips tinting.

**Gotcha - find-again is dispatched from two places, split by capture
status.** With the query field focused, macOS `text_input` swallows the
chord (see below), so `handle_hotkey` never sees it; with the editor
focused it comes through normally. The `event::listen_with` arm therefore
handles the chord *only* when `status == Captured`, leaving the uncaptured
case to `handle_hotkey`. Drop that condition and both fire, stepping two
matches per press.

**Gotcha - macOS leaks the shortcut's letter into the query.** Cmd doesn't
suppress character production there, and `text_input`'s insert branch
(`iced_widget-0.14.2` `text_input.rs:1015`) has no modifier guard at all -
it inserts any non-control character and captures the event. So Cmd+G
types a "g" into the query on its way to being a shortcut. `Message::
FindQueryChanged` discards a *one-character* growth arriving while
`command()` is held; the length test is what keeps a legitimate Cmd+V
paste working. This is the same quirk `jumppad_textarea::key_binding`
already works around for the document. Linux is unaffected - Ctrl
produces a control character, which the insert branch filters out - so
this cannot be reproduced under the Xvfb harness.

State is **per tab**, in `JumpPadApp.find: HashMap<u64, FindState>` keyed
by `Tab::id` - deliberately not a field on `Tab`, since `editor_core` is
the widget-abstraction boundary and a query string is shell UI state.
Closing the palette keeps the query (`FindState::open`); closing the tab
drops the entry.

**Gotcha - matches are colored by the highlighter, not selected.** iced's
`text_editor` draws its selection only while it is *focused* (the whole
selection-drawing block sits behind `if let Some(focus)`), and the find
field holds focus exactly when matches need to be visible. So matches are
pushed into the editor with `TextEditorWidget::set_find_matches` and
emitted as extra highlighter spans, which render regardless of focus.
Don't "simplify" this back to just selecting the match - it renders
nothing while the user is typing. The current match *is* also selected,
which is what makes it appear as a normal selection once Escape hands
focus back to the editor.

Two constraints fall out of that mechanism:

- The spans must be appended **after** the syntax spans in
  `highlight_line`. iced feeds them to `AttrsList::add_span`, whose range
  map overwrites on overlap, so the last span covering a byte wins.
- `HighlighterSettings`' hand-written `PartialEq` must compare the match
  fields. That impl is the only thing telling iced the highlighter needs
  re-running; omit them and the coloring freezes on screen.

**Gotcha - a background highlight for matches is not reachable in iced
0.14.** Matches are a **text tint** for a hard reason, not a stylistic
one; this was investigated in full, so don't re-derive it:

- `iced_core::text::highlighter::Format` has exactly two fields, `color`
  and `font`. No background.
- The attrs layer underneath can't carry one either - `background` does
  not appear anywhere in `cosmic-text-0.15`'s source.
- The widget fills exactly three quads (`iced_widget-0.14.2`
  `text_editor.rs`): the editor background, the caret, and the *current
  selection's* rectangles. Selection backgrounds come from
  `editor.selection()` returning `Vec<Rectangle>`, i.e. entirely outside
  the attrs system, and only for the one active selection.
- Computing those rectangles for arbitrary ranges needs the cosmic-text
  `Buffer`. `iced_graphics::text::Editor::buffer()` is public; reaching it
  from `Content` is what the fork now makes possible (see the text-area
  fork section) - before that, `Content`'s private `editor` field left no
  path to it at all.
- Faking it with an overlay in the *app* still founders on the scroll
  offset: the app sees `Action::Scroll` from the wheel, but cursor-driven
  auto-scroll (`shape_until_cursor`) is internal, so any offset tracked
  outside the widget drifts.
- 0.14.0 is the latest published iced, so there is no version to upgrade
  into.

So the remaining work is inside `jumppad_textarea`'s `text_editor.rs`,
drawing the quads in `draw` the way the scrollbar thumb already does -
not in the app shell, and no longer a matter of patching iced. It is still
real work: the match rectangles have to be derived from the buffer's layout
runs, which is more than reading a scroll offset. The current match does
get a genuine selection background whenever the editor holds focus (after
Escape, or during a Cmd+G find-again).

`FindState::origin` is the cursor position captured when the palette
opened, and live search anchors there rather than at the live cursor -
selecting a match moves the cursor, so re-anchoring per keystroke would
walk the selection down the document as the user types.

**Gotcha - Escape needs its own event listener.** `keyboard::listen()`
only yields events whose status is `Ignored`, i.e. ones no widget
captured (`iced_futures::keyboard::listen` filters on exactly that), and a
focused `text_input` handles Escape by unfocusing itself and calling
`shell.capture_event()`. So the app never saw the first Escape and the
palette took *two* presses to close. `subscription()` therefore has a
dedicated `iced::event::listen_with` entry that ignores capture status and
maps Escape to `Message::CloseFind`.

Two consequences of that listener firing unconditionally: `CloseFind`
no-ops while `pending_close` is up, so the unsaved-changes modal keeps
first claim on Escape; and Escape closes the palette even when focus is in
the editor, which matches VSCode.

## Transparent windows: why nothing repaints the background

With `[alpha] background < 1.0`, `JumpPadApp::style` hands iced a
`background_color` whose alpha is the configured value. The compositor
writes that straight into the framebuffer (`BlendMode::Source`, so it
*replaces* rather than blends), then every widget quad blends **on top**
of it. Two consequences that are easy to trip over:

- **Every extra layer compounds opacity.** A region painted at the same
  alpha `a` as the window background ends up at `2a - a²`, not `a`. At
  `a = 0.7` that's 0.91 - the window is far more solid than configured,
  and any region *not* painted (the "+" button paints no background at
  all) stays at 0.7 and reads as a conspicuously lighter block.
- **Gotcha - hairline seams.** `iced_tiny_skia` fills quads with
  `anti_alias: true`, and widget edges land mid-pixel constantly (iced's
  default line height is 1.3x the font size, text metrics decide chip
  widths, and a fractional DPI scale factor moves everything again). A
  partially covered edge pixel blends both sides at partial coverage, so
  where two quads *abut*, that pixel ends up less opaque than either
  neighbour - a faint 1px border along the seam. It's invisible at
  `a = 1.0` (the base layer is already opaque) which is why this only
  shows up with transparency on.

So the rule: **never paint a quad that merely reproduces the window
background.** The editor (`editor_style`, `jumppad_textarea`) and the
active tab (`tab_frame_style`) both deliberately paint *nothing* when
translucent - iced's `text_editor` and window backgrounds are both
`palette.background.base.color`, so matching them seamlessly means adding
no layer, not adding a matching one. Re-adding a background there as a
"fix" is what puts the seams back.

Shade differences that *are* wanted (inactive tabs, the row past the last
tab) go through `darkening_wash`, a thin black overlay sized to the
desired brightness drop, rather than a pre-darkened copy of the
background. It adds the least opacity that still produces the shade, and
because it overlays bare background instead of abutting another quad its
edges ramp smoothly instead of dipping. Darkening a translucent surface
inherently costs opacity, though, so a near-black theme can't reach the
full step without going solid - `WASH_ALPHA_CEILING` caps it and those
themes get a shallower step instead.

The other half of the fix is keeping quad edges *on* the pixel grid, so
there's no partial coverage to blend in the first place. The tab bar's
text carries absolute line heights (`TAB_TITLE_LINE_HEIGHT`,
`TAB_CLOSE_LINE_HEIGHT`) instead of iced's default
`LineHeight::Relative(1.3)`, which would make the strip 6 + 20.8 + 6 =
32.8px tall and put its bottom edge mid-pixel. Verified directly against
tiny-skia 0.11.4: two abutting quads at a 32.8 boundary leave the shared
row at alpha 225/255 against an interior of 233; at 33.0 the row reads a
flat 233. Two limits worth knowing:

- This only holds at integer scale factors. Nothing chosen in logical
  pixels survives a 1.25x or 1.5x display.
- It only fixes *horizontal* edges. Chip widths come from text
  measurement, so the left/right edges of a wash stay fractional; there's
  no app-level way to round them.

**Gotcha - `snap` does nothing on the default binary.** iced has a
pixel-snapping mechanism for exactly this: `renderer::Quad::snap`, exposed
as `container::Style::snap` / `button::Style::snap` and defaulted from
iced's `crisp` Cargo feature. `iced_tiny_skia` never reads the field -
zero occurrences in the whole backend - so setting it changes nothing in
`jumppad` and only takes effect in `jumppad-gpu` (`iced_wgpu` passes it
through to its quad shader). Don't reach for it expecting a fix on the
default build, and weigh the two binaries rendering differently before
turning it on for the wgpu one.

Disabling the antialiasing itself is not reachable either: `anti_alias:
true` is hardcoded in `iced_tiny_skia::engine`'s quad fill, so it would
take a `[patch.crates-io]` fork - and it's global, meaning every rounded
corner and stroked border in the app (the modal, its focused button, the
scrollbar) would go jagged to fix a seam the tab bar no longer has.

## Opening files

Three entry points reach the same place. `Cmd/Ctrl+O` goes through `rfd`'s
dialog, a drop through the window events below, and a path named on the command
line through `Message::OpenPaths` - all of them end at
`JumpPadApp::open_loaded_file`, which decides between reusing an untouched
scratch tab and pushing a new one. Add a fourth entry point there, not beside it.

`parse_args` in `lib.rs` treats everything that isn't `--help`/`--version` as a
path. Resist growing that into a flag parser; the two flags exist because
packagers expect them.

Argv paths are read *synchronously*, unlike a drop's `Task::perform`. That's
deliberate: concurrent reads would land in nondeterministic order, and
`jumppad a.txt b.txt` has to produce tabs in that order. They're also deferred
one message past `JumpPadApp::new` (via `Task::done`) so `next_id` is already
settled by whatever the session restored before any argv tab is pushed.

A named file that doesn't exist opens an empty tab bound to its path rather
than erroring - that's how you start a new file. Two consequences: this is the
one place argv behaves differently from a drop (where a missing file *is* an
error, though a dropped file always exists), and session restore has to
tolerate a clean file-backed tab whose file was never created, which is why the
restore branch in `new()` skips `reload_from_disk` for a path that doesn't
exist. Undo that skip and you get a spurious "Couldn't reload a restored tab"
on every launch after someone runs `jumppad newfile.txt` and quits.

## External file changes

JumpPad notices when an open tab's file is changed by something else, and
copies VS Code's rule: silently reload when it's safe, never clobber unsaved
edits when it isn't.

| On-disk event | Buffer clean | Buffer dirty |
| --- | --- | --- |
| Modified | Reload silently. Scroll position kept, and the reload is undoable - Ctrl+Z restores the pre-reload text and leaves the tab dirty. | Buffer untouched, `externally_changed` set. The conflict surfaces in the tab title, in a bar over the editor, and at save time. |
| Deleted | Tab stays open with its content, goes dirty, `disk` becomes `None` - the next save recreates the file. No prompt. | Same. |

The conflict dialog offers **Overwrite** / **Discard & Reload** / **Cancel**.
No Compare: JumpPad has no diff view and shouldn't grow one.

Three signals feed one `DocumentWatch` (`crates/jumppad/src/docwatch.rs`) -
the same shape `reload.rs` uses for `config.toml`, and for the same reason:
native `notify` events over the *directories* holding open files, a sweep on
window focus (the safety net for events the watcher never delivered), and
JumpPad's own saves. The third needs no suppression list - `save_to` stamps
the file it just wrote, so the watcher event that write causes compares
equal and sweeps to nothing. **That stamp is the entire defense against a
save triggering a reload of itself**; move it back into the `FileSaved` arm
and you get a reload loop that eats the cursor position on every save.

`JumpPadApp::resolve_disk_changes` is the one decider. It **stats every
file-backed tab and compares stamps** rather than matching event paths
against open files. That's what keeps relative paths (`jumppad newnote.md`),
deleted files (which can't be canonicalized), and atomic-save renames from
each needing their own case. A spurious event costs N stats for N open tabs.
Any path-matching added later has to reckon with all three.

The watcher subscription is the one genuinely new mechanic against
`reload.rs`: documents come and go, so it's `Subscription::run_with(paths,
..)` rather than `Subscription::run`. The recipe hashes `paths`, so changing
the set tears the stream down and starts a fresh watcher over the new
directories - which is the intended mechanism, and why `watched_paths()`
sorts and dedupes. An unstable order would restart the watcher on every
unrelated tab switch. `run_with`'s builder is a bare `fn(&D) -> S`, not a
closure: everything the watcher needs has to arrive through `paths`.

Things that will bite:

- **`config.toml` open as a tab** is watched by both `ConfigWatch` and
  `DocumentWatch`. That's correct and independent - one reloads settings, the
  other reloads the buffer. Don't merge them.
- **A reload is not an edit.** It doesn't route through `Message::Editor` and
  doesn't set `dirty` on the clean path; doing either would arm draft
  autosave and write a draft for content already on disk.
- **`refresh_find()` after every buffer replacement** - the match list holds
  byte ranges into the old text.
- **A read is async.** `DocumentReloaded` re-validates against the tab as it
  stands *now*: gone, retargeted by a Save As, or gone dirty while the read
  was in flight all drop the result (the last one flags instead). Applying a
  stale read over edits the user just made is the one thing this must never
  do.
- **mtime granularity.** A same-length rewrite inside one filesystem tick can
  compare equal and be missed, same as `reload.rs` accepts. The focus sweep is
  the backstop; a content hash on a per-stat path is not the fix.
- `[files] save_conflict_resolution` is read from `self.config` at save time,
  **not** through `apply_config` - so a reloaded `config.toml` picks it up for
  free. It's the one live setting that legitimately has no `apply_config` arm.

## Drag and drop

No crate needed for this - iced already delivers it. `iced_winit`'s
`conversion.rs` maps winit's `HoveredFile`/`DroppedFile`/`HoveredFileCancelled`
onto `iced::window::Event::{FileHovered, FileDropped, FilesHoveredLeft}`, and
`subscription()` picks those three off `iced::event::listen_with`. Don't add a
DnD dependency.

Two things that will bite:

- **A completed drop emits no `FilesHoveredLeft` on Windows or macOS.** The
  `FileDropped` arm has to clear `files_hovered` itself, or the overlay sticks
  after a successful drop.
- **Native Wayland delivers nothing.** winit's Wayland backend has no file-DnD
  implementation (0.30.13 and earlier), so drops are silently inert there while
  X11, Win32, and macOS all work. Nothing in this repo can fix that; the file
  dialog is the fallback. Don't chase a bug report about it as if it were
  JumpPad's.
- **The unsaved-changes modal's scrim doesn't stop a drop.** Window events
  never go through the widget tree, so the scrim - which only swallows clicks -
  is no defense. The `FileDropped` arm turns drops away while a prompt is up,
  the same way `KeyPressed` intercepts keystrokes.

A multi-file drop arrives as one `FileDropped` per file, so nothing special is
needed to open several at once. Dropping onto an untouched scratch tab rebuilds
that tab in place rather than calling `set_text` - the editor factory takes the
file extension, and that's what selects the grammar, so a reused editor built
for an untitled buffer would render unhighlighted.

## Config (`jumppad_config`)

`config.toml` is looked for next to the running executable first, then
in the current directory (a `cargo run` convenience). If nothing is
found, built-in defaults are written to the first location. A malformed
config file logs an error and falls back to in-memory defaults rather
than failing to start - a broken config should never be able to prevent
the editor from opening. Config sections (`[[languages]]`, `theme`) are
independently defaulted so old config files stay valid as new sections
get added.

`[scroll] sensitivity` is a plain multiplier on wheel and trackpad
scroll distance, defaulting to `1.0`. The shipped speed is deliberately
*half* of upstream `iced_widget`'s (`LINES_PER_WHEEL_NOTCH` is 2, not
4; `PIXELS_PER_LINE` is 8, not 4), so the knob reads as "×1 is normal"
rather than "×0.5 is normal" - the old speed is `2.0`. The range check
lives in the widget (`clamp_scroll_sensitivity`), not in
`jumppad_config`, which keeps this crate free of a `iced` dependency
the same way `AlphaConfig` does. The multiplier is applied to a *float*
line count, which is then turned into pixels and sent through
`Content::scroll_by` - so a fractional sensitivity means fractional
*movement*, not a banked remainder waiting to make a whole line. That is
why the floor of `SCROLL_SENSITIVITY_RANGE` can sit as low as `0.05`
and still be a usable setting rather than a dead wheel.

Before pixel scrolling this knob could only make the wheel *slower*, not
smoother: every event still moved the view a whole line or not at all,
and a low sensitivity just meant more events that moved nothing. Anyone
reintroducing a whole-line path should expect that complaint back.

`[[languages]]` is the one place a language is described: `name` (for
the file's readability), an optional `syntax` (the `<syntax>.wasm`
grammar its extensions highlight with), `extensions`, and an optional
`comment` - exactly one of `comment.single` or `comment.multi.left`/
`.right`; defining both fails the whole file's parse (enforced by a
serde `try_from` on `CommentSyntax`). A user-provided array replaces the
built-in default list wholesale. Comment tokens can't come from the
tree-sitter grammars - a `.wasm` carries only parse tables and node-kind
names, the comment token exists solely inside its compiled lexer - which
is why this is config, like every other editor does it.

### Live reload

`config.toml` and `keybinds.toml` reload while the app runs, debounced.
Three signals feed one `ConfigWatch` (`crates/jumppad/src/reload.rs`):
the editor saving a watched file itself (`FileSaved` → `note_saved`),
native file-system events (`notify`, watching the candidate
*directories* so atomic-save renames don't lose the watch), and a
fingerprint check on window focus as the safety net for missed events.
Each signal only marks a file dirty and pokes a shared
`editor_core::Debounce`; `settled` is the single decider of what
reloads, so the signals can't conflict or double-apply.

**Adding a live setting means adding an arm to `apply_config` (or
`apply_keybinds`) in `app.rs` - nowhere else.** Both diff against the
stored `self.config`/`self.keybinds` baseline and only touch what
changed. A setting that can only apply at startup (window decorations,
visor mode, and `alpha.background` on a window that booted opaque - the
`transparent` window flag is creation-time) gets a `restart_required`
log line instead of a half-working apply. One `[[languages]]` edit can
feed two consumers, so `apply_config` diffs its *derived views*: the
comment-style map applies live, the extension-to-grammar map is baked
into the registry at startup and logs restart-required.

The one exception is `[files] save_conflict_resolution`, which nothing
holds a copy of: `save_expectation` reads it out of `self.config` at save
time, so a reload is picked up with no arm to write (see `## External file
changes`). A setting only earns that treatment if the code that needs it
already runs at the moment it's needed - anything cached, pushed to a
widget, or applied to the window still belongs in `apply_config`.

Reloads go through `try_load`/`try_load_keybinds`, which never write
default files and never fall back to `Default` - a half-edited file
keeps the last good in-memory config, with the parse error in the
dismissible banner. Contrast with startup's `load()`, which must never
fail (above).

Three things reach the *editor* widgets on reload, all via the
`SharedEditorConfig` handle every `TextArea` reads per view (the app
can't reach into a `Box<dyn TextEditorWidget>`): the editor keybind
overrides, `alpha.background`, and `scroll.sensitivity`. The scroll
sensitivity is the one that arms no repaint - nothing on screen changes
until the next wheel event, and the `view` that event causes is where
the widget picks the new value up. `alpha.foreground` rides the same
setter but is stored in a static atomic (`to_format` must stay a bare
`fn` pointer), and it also sits in `HighlighterSettings`' hand-written
`PartialEq` - without that, syntax-colored spans keep their old alpha
until an unrelated edit, same bug class as the find-state fields. Any
visual apply also arms `redraw_nudge_frames`, since tiny-skia's damage
tracking can otherwise skip presenting a change that moved no widget.

## Where the grammar files live

`syntaxes/` at the repo root holds the `.wasm` grammars and
`.injections.scm` queries actually used by this checkout.
`default_search_dirs()` in `crates/jumppad/src/app.rs` looks next to the
executable first, then `./syntaxes` for `cargo run` convenience -
mirroring `config_paths()`'s search order in `jumppad_config`.

`syntaxes/` is gitignored, not committed - these are compiled binaries
built from *other projects'* tree-sitter grammar sources, not something
derived from code in this repo. Run `./scripts/build-grammars.sh` (needs
`git` and `npx`) to populate it: it clones each upstream grammar repo
listed in the script and compiles it with `tree-sitter build --wasm`. If
a grammar ever needs updating (new file type, upstream fix), edit that
script rather than hand-placing a `.wasm` file.

The app still starts and runs fine with `syntaxes/` empty or missing
entirely (see `log_wasm_files_found` in `app.rs`) - it just opens files
unhighlighted, consistent with "highlighters are optional" in
`README.md`. Don't mistake that startup diagnostic for a real error;
only chase it if highlighting is actually expected to be working.

## Miscellaneous things worth knowing before you "fix" them

- `lib.rs` has `windows_subsystem = "windows"` (which hides the console
  window on release builds) temporarily disabled, with a comment saying
  why: an in-progress Windows highlighting bug needed console output to
  debug. Don't silently re-enable it as a "cleanup" without checking
  whether that investigation is actually finished.
