# AGENTS.md

Working notes for an AI agent operating on this codebase. See `README.md`
for the human-facing pitch; this file is the architecture map, the
"why is it built this way," and the list of things that will bite you if
you don't know about them going in.

## Never edit README.md

`README.md` is hand-maintained by the repository owner. Do not touch it -
not to document a feature you added, not to fix a typo, not to keep it in
sync with a change elsewhere. Edit it only when the owner asks for a README
change in so many words.

If a change you made makes the README wrong, say so and leave it alone.

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

## Naming

Files, types, functions and variables all carry their own explanation. A
reader should be able to tell what a thing is for without reading its body
and without reading a comment above it.

Name the reason something exists, not the machinery inside it. `undo_depth`
over `step_cap`, `close_open_burst` over `seal`.

Two rules that follow from that:

- **No metaphors.** A name that needs a sentence of decoding - "footprint",
  "sticky note", "envelope" - is worse than a plain one, however apt it feels
  while you are writing it.
- **If a name needs a comment to explain it, the name is wrong.** Rename it
  and delete the comment. Do this before reaching for either.

## When to split something up

None of these are hard limits. They are the signals that something has grown
past the shape it should be in:

- **A long file.** Usually several responsibilities sharing one file. Split
  it into modules named for what each one is for.
- **A long function.** Break the steps out into helpers whose names say what
  each step accomplishes.
- **Four or more levels of indentation.** Almost always a helper waiting to
  be extracted; deep nesting usually means a decision is being made in the
  wrong place.
- **A long comment.** The name it sits above is not pulling its weight, or
  the thing it describes is doing too much.
- **A file thick with comments.** The same problem at scale. Read it as a
  naming failure before you read it as documentation.

## Comments

Use comments sparingly and intentionally. Where one sits decides whether it
belongs at all:

- **On a type: welcome, and length is fine.** Describe what it is
  responsible for and why it exists. This is the most useful comment in the
  codebase and the one most worth writing well.
- **On a function: fine when needed.** Can run past a few lines when it is
  clarifying a nuance of the behaviour or of an argument.
- **Inside a function body: the one to worry about.** A body that needs
  running commentary is a body that wants splitting into named helpers. If
  you find yourself narrating steps, extract them instead.

Keep every comment high level. Do not reach for details the reader has no
context for. If something is genuinely required to understand the feature,
explain it; otherwise generalize it in plain English or leave it out.

Bad - narrates mechanics, and props up a name that says nothing:

```rust
/// The whole lines an edit disturbed plus the exact bytes on either side.
/// `between` finds one by trimming matching runs; `inverted` makes one
/// footprint serve undo and redo alike.
pub struct EditFootprint {
```

Good - the name carries the idea, the comment stays at the level of the
feature:

```rust
/// A single delta of the text's content. Deltas are blocks of line
/// differences.
pub struct LineChange {
```

Two older rules still hold: put the comment at the thing's first
declaration rather than cross-referencing other files, and skip bug history
and feature origin unless it is a genuine non-obvious gotcha.

## Workspace layout

```
crates/
  jumppad/           the application shell: iced::Program, tabs, menus, file I/O, theming
  editor_core/       the abstraction boundary between the shell and the actual text widget
  jumppad_textarea/  the one TextEditorWidget impl today; owns a fork of iced's text_editor
  syntax_registry/   loads/caches/refcounts tree-sitter WASM grammars; no iced dependency
  jumppad_config/    config.toml loading + defaults; no iced dependency
  jumppad_actions/   every action the product can perform; NO dependencies at all
  jumppad_keybinds/  default key chords, mapping presses onto actions
syntaxes/            *.wasm grammar files + *.injections.scm queries (see below)
```

Dependency direction is one-way: `jumppad` depends on everything;
`editor_core`, `syntax_registry`, and `jumppad_actions` depend on nothing
inside this workspace (`jumppad_config` depends only on `jumppad_actions`,
to validate override names). `jumppad_textarea` depends on `editor_core` (to
implement its trait) and `syntax_registry` (to drive highlighting), but
not on `jumppad`. This is deliberate - see `editor_core::widget::TextEditorWidget`'s
doc comment: a future non-iced editor widget should be a new crate
implementing that trait, not a rewrite of `jumppad`.

`editor_core` also holds the two pieces of theming both sides need:
`darkening_wash` and `FLOATING_SURFACE_DARKEN` (see the transparent-windows
section). It's the only crate `jumppad` and `jumppad_textarea` share, so a
color both of them paint with lives there rather than being copied.

## Actions and keybindings

**An action is what the product can do; a key is one way to ask for it.**
Those are two crates, and the arrow between them points one way:

```
jumppad_actions      no dependencies at all - not even iced
        ^
jumppad_keybinds     + iced_core
        ^
     jumppad         the only consumer of jumppad_keybinds
```

`jumppad_keybinds` knows about actions; `jumppad_actions` must never learn
about keys. That is what lets a mouse or gesture binding arrive later as a
sibling crate rather than a rewrite - it would map its own input onto the same
`Action`. The day `jumppad_actions` gains an input dependency, that stops
being true.

**Adding an action is one row in `actions!`, plus one arm wherever it is
performed.** The macro generates the `Action` enum, the `ACTIONS` table and
`Action::ALL` from a single list, so they cannot drift. Then:

- a default chord in `DEFAULT_KEYS` (`jumppad_keybinds`), if it wants one -
  an action with no default is still bindable in `keybinds.toml`;
- an arm in `jumppad_textarea::binding_for` *or* `jumppad`'s `message_for`,
  never both. `every_action_is_wired_to_exactly_one_layer` fails the build
  otherwise, and `sample_keybinds_document_every_action` fails until the name
  is documented in `config/keybinds.sample.toml`.

**`jumppad_textarea` depends on `jumppad_actions` only, never on
`jumppad_keybinds`.** Resolving a press needs both the default chords and the
user's overrides, and the widget crate has no business seeing either - the
same reason `build_editor_overrides` always lived in the app. So the app
builds a `KeyResolver` closure and injects it through `SharedEditorConfig`,
the rail a `keybinds.toml` reload already used. Override-beats-default
precedence therefore exists in exactly one place, `resolve_action`; it used
to be written out twice, once per layer, which is the kind of duplication
that eventually disagrees with itself.

**Default chords match the character your layout produces; overrides match
the physical key.** Deliberate, and the reason `keybinds.sample.toml` tells
German-layout users to override `toggle_comment` - their layout cannot type
an unshifted `/`, but the physical key is still there. `Trigger::Latin` goes
through `Key::to_latin`, which falls back to the physical key for letters and
digits, so Cmd+Z survives a Cyrillic layout; punctuation has no such fallback.

**Modifier matching is exact.** `Mods::matches` accounts for every modifier,
held or not. The `if` chains this replaced tested only what they cared about,
so Cmd+Alt+N opened a tab as readily as Cmd+N, and rows had to be ordered -
`Character("s") if shift` above plain `Character("s")`. Exactness makes
`DEFAULT_KEYS` order-independent. `command` and `jump` in a `Mods` are
platform *roles* (Cmd/Ctrl and Option/Ctrl); `control` is literal Ctrl, which
is what Ctrl+Tab needs and why it used to have to dodge the `command()` gate
by hand.

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
- One let-chain was unwound, back when the workspace was edition 2021 and
  `iced_widget` was 2024. Both are 2024 now, so that constraint is gone and
  the unwinding can be undone whenever someone is in there anyway.

### The scrollbar

An overlay scrollbar: a rounded thumb on an invisible track, at the right
edge of the text area. Hidden until the pointer enters the right 100px or
the document scrolls, then held for 900ms and faded out. Drag the thumb to
scroll. Geometry and fade math are pure functions in `scrollbar.rs`, taking
`now: Instant` so they test without a window (same convention as
`history.rs`).

**Everything is measured in wrapped rows, and the document's are estimated.**
All three of `Metrics` count rows, so the thumb's length is
`viewport / content` and its travel is `max_position`. Rows are what the eye
compares - a screen of paragraphs shows less document than a screen of short
lines - and a scroll moves in them, so a drag needs no unit conversion at all.

Counting them is the problem. cosmic-text lays out only what is on screen, so
summing `BufferLine::layout_opt()` over the document reports one row for every
off-screen line that wraps to three, and the total grows as you scroll into
them; laying the rest out costs the memory this editor exists not to spend. So
`State::metrics` estimates instead: how wide each line would draw (its byte
length against the characters that fit on a row, measured from the glyphs on
screen), which is *exact* for every line that fits - all of most documents -
and close for the ones that wrap.

**Being the same answer everywhere matters more than being exact.** Two bugs
came out of measuring the document from the lines on screen instead:

- the thumb grew and shrank as a drag crossed a paragraph, because the rows a
  screen holds per line changed under it;
- worse, a drag in a document with blocks of wrapped and unwrapped lines never
  arrived. The row it was aiming for is a fraction of `max_position`, and
  `max_position` moved every time the view did, so past a certain pointer
  position there is no row that answers to it: the view lands in a paragraph,
  which moves the target back out of it, forever.

An estimate a row or two out costs a thumb a pixel of length. A measure that
moves with the view costs the drag its fixed point.

**A thumb drag is a correction per frame, not a delta per pointer move.**
`State::drag_to` only records where the pointer has the thumb;
`scroll_to_pointer`, asked once per frame from the redraw event, answers the
pixels between where the view *is* and the row the pointer is asking for. It
has to be a correction rather than a delta because the rows it crosses were
estimated - a scroll can land a little short or long, and the frame after
closes the difference - and it has to be once a frame because several
`CursorMoved` events land in one input batch, all of them before any of their
scrolls has reached the document, so answering each would stack the same
correction into an overshoot.

**"Once a frame" means the redraw event's own instant, not a fresh clock
reading.** iced re-runs that event at the same instant after a widget
publishes anything, laying the whole window out again each time, so a drag
that answered every pass bought a fraction of a row's accuracy with two extra
layouts - and left iced logging `More than 3 consecutive RedrawRequested
events produced layout invalidation` on most frames of a drag. `Drag`'s
`scrolled_at` is what draws that line; `drag_scroll`'s `Step::Waiting` is the
same guard, arrived at from the same warning.

Two rules keep that loop honest at the ends of the document, where an estimate
that is a row out is the difference between arriving and not:

- A thumb against the end of its track asks for the end of the *document*,
  aiming a screenful past the last row the estimate knows about and letting
  cosmic-text clamp it where the document really ends - but never backwards,
  or an estimate that ran long would pull the view back off the end it had
  just reached. The top is exempt: row zero is known exactly.
- A scroll that moved nothing means the document is against an end, and the
  drag stops asking until the pointer moves again. Without it the over-aim
  above would re-publish on every frame for as long as the button was held.

`advance_scrollbar_drag` asks for another frame whenever it moved the view, so
a correction that landed short still finishes with the pointer sitting still.

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
`0.14.0` **tag** plus two additive methods, both on the concrete
`graphics::text::Editor`:

```rust
pub fn scroll_by(&mut self, pixels: f32)
pub fn set_tab_width(&mut self, tab_width: u16)
```

The second is what `[indentation] width` reaches the screen through, and it
is here for the same reason the first is: `Editor::buffer()` hands out a
shared reference and nothing else on the editor reaches the buffer, so the
tab stops sat at cosmic-text's default of eight for the life of the program.
The branch name has outlived its subject; it stays put because renaming it
would break every checkout's `Cargo.lock`.

It also carries one fix rather than an addition: `PartialEq for Weak`
compares which editor the reference points at (`Arc::ptr_eq`) before asking
`Internal` how it is laid out. See "The repaint nudge, and why it is gone"
for what that one is holding up.

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

**A pixel scroll moves in visual rows; `scrolled_to` counts logical lines.**
Restoring a view is the one place that difference bites, and it is why
`CapturedView` holds the buffer's own `Scroll` - a logical line plus the
pixels into it - rather than the single `scrolled_to` number. Subtracting one
`scrolled_to` from another and spending the difference as pixels is only
correct while nothing wraps; the widget wraps by default (`Wrapping::default()`
is `Word`, and nothing overrides it), so in a document with a long line in it
the restore missed, dropped the cursor past the safe area's low boundary, and
the reveal then "corrected" it onto that boundary - the view lurching on every
single line command. `restore_pixels` measures the gap in rows that have
actually been laid out instead. That is bounded and local - the handful of lines
between a fresh buffer's reveal and the view it is returning to, all of them
in or beside the view - and runs once per rebuild, so it is *not* the
whole-document row count the scrollbar section rejects. A line cosmic-text has
not shaped falls back to counting logical lines.

**Test the wrapped layout, not just the flat one.** Two bugs shipped because
every reveal test shaped with `Wrapping::None` while the widget runs
`Wrapping::Word`, so visual rows and logical lines coincided in the tests and
came apart in the app. `wrapped_text`/`shape_wrapped` are the harness for it,
and `a_wrapped_documents_lines_really_do_wrap` guards the guard - the moment
that text stops wrapping the cases around it pass for the wrong reason.

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

### Selecting past the edges of the text

A selection drag follows the pointer wherever it goes - over the tab bar, out
of the window, off the screen - and while the pointer sits above or below the
text the view walks that way until the button comes up. That is what a native
Windows or macOS text field does, and it is two mechanisms, not one:

- **The pointer's position is read unclamped** (`text_position` in
  `text_editor.rs`). Upstream asks `cursor.position_in(bounds)`, which is
  `None` the moment the pointer leaves the widget - that is what used to
  freeze a selection as soon as the pointer crossed onto the tab bar. The
  platforms keep the moves coming: winit grabs the pointer for the duration
  of a press on Windows and macOS alike, so `CursorMoved` keeps arriving with
  coordinates outside the window, negative ones included, and cosmic-text
  resolves a position past the text against its nearest row.
- **`drag_scroll.rs` walks the view** while that position is past the top or
  bottom edge - `EDGE_SPEED` right at the edge, up to `TOP_SPEED` once the
  pointer is `TOP_SPEED_REACH` beyond it, times `[scroll] drag_speed`. Each
  frame it scrolls, drags again so the selection takes in the rows that came
  into view, and asks for the next frame, which is what keeps a drag moving
  while the pointer sits perfectly still outside the window. The distance is
  measured in pixels and lands in the buffer through the same fractional
  `scroll_by` the wheel uses, so the walk glides rather than stepping.

**The ramp is squared, not straight.** A pointer just outside the window is
asking to pick an exact line, which wants a line or two a second; a pointer
flung to the far side of the screen is asking to cross the document. A
straight ramp spends nearly all of its reach above the first of those and
leaves a sliver for it.

**A hit test only sees the rows that are on screen.** cosmic-text's
`Buffer::layout_runs` yields visible runs only, so a drag above the view
selects up to the top *visible* row and no further. The walk isn't a nicety
on top of the unclamped position; it is the only way a selection reaches a
line that was never drawn.

**Gotcha - a walk must never drag at the pointer's own height.** cosmic-text
scrolls to reveal a caret an edge is clipping (`shape_until_cursor`), and a
pointer past the edge hit-tests onto exactly such a row - so the reveal added
a whole line to whatever pixels the walk had just asked for. Measured before
the fix: 2px asked, 20px moved, at every speed the ramp could name. So
`Drag::selecting_at` pulls the caret back onto the nearest row the edge is
*not* cutting through, and the two stop fighting. The pointer's own position
is still what the selection follows everywhere else, including a pointer
merely off to the side, and a partly-clipped row still reveals itself when
the pointer is genuinely *on* it - that nudge is how a drag inside the window
scrolls.

**Gotcha - the drag has to aim at the rows the scroll is about to leave.**
The scroll message reaches the document before the drag does, so
`Walk::after_scrolling` advances the row geometry by the pixels in flight
first. Aiming at the rows as they sit during the frame put the caret back on
a clipped row at high speeds, and the reveal came straight back.

**A drag ends on the button coming up or on the window losing focus, and
there is no third way out.** The pointer grab goes back to the system along
with the focus and no release ever arrives, so a drag left running would walk
the view on its own until the next click.

**Gotcha - the next frame has to be asked for even on a frame that moved
nothing.** iced re-runs the redraw event at the same instant after a widget
publishes anything, and keeps the *last* pass's redraw request. The second
pass has no time in it, so it earns no pixels - and a `request_redraw` made
only when the view actually moved is thrown away with the first pass's
answer, leaving the walk to advance at the caret blink's pace. That is what
`Step::Waiting` is for: past an edge, nothing to scroll yet, still worth a
frame.

Only the vertical edges walk. The widget wraps (`Wrapping::default()` is
`Word`, and nothing overrides it), so there is never anything to scroll
sideways to.

### Clipping the text (and why the tab bar collected old text)

The editor hands `fill_editor` a clip a sliver shorter than the text area
(`text_clip` in `text_editor.rs`), and that sliver is load-bearing.

`iced_tiny_skia` builds a clip mask for text only when the text's own bounds
reach past the clip it was given; text that fits inside its bounds, it
reasons, cannot paint outside them. An editor breaks that reasoning. Its
declared bounds are the text area exactly, but cosmic-text draws the rows the
top and bottom edges cut through *whole* - that is what scrolling by pixels
means - so the overhang lands outside the editor with no mask to stop it. It
also skips the damage region that way, which is how the overhang reaches the
tab bar at all.

On Windows that overhang is permanent. `softbuffer` reports `age() == 1`
there, so the compositor repaints only the regions its damage tracking names,
and nothing ever damages the band above the editor - each scroll leaves
another sliver of text up there until the band is a smear of old rows. macOS
never shows it because the fork reports `age() == 0` (see the softbuffer
section), so every frame is a full repaint; `wgpu` never shows it because it
clips with a scissor rectangle instead.

**A clip the editor demonstrably doesn't fit inside is what gets the mask
built.** A tenth of a pixel does it, and costs nothing: the mask is not
anti-aliased, so a pixel belongs to it by its centre, and no centre moves.

The bottom half of this predates pixel scrolling - a view whose height isn't
a whole number of rows always cuts its last row - and it landed in the
editor's own padding, which is why it read as a smudge at the bottom edge
rather than as text.

`tests/repaint.rs` paints real frames through `iced_tiny_skia` and asserts
nothing lands outside the text area, driving the compositor's damage
tracking the way the Windows build does. It is the only test here that looks
at pixels; a fix for this that isn't in `text_clip` should keep it passing.

**Not the same bug as the tab-switch ghosting**, though they share a
framework. That one is iced computing *too little* damage - its editor
comparison looks at font, bounds and metrics and never at the text, so a
switched tab could be declared unchanged (see "The repaint nudge, and why it
is gone"). This one is the editor painting *outside the damage it asked for*.
Both come of `iced_graphics::text::editor` misdescribing the editor: once by
saying it is unchanged when it isn't, once by saying it fits inside bounds it
paints past. That crate is already in the patch set, so bounds that cover what
is actually painted would retire `text_clip` outright.

### Revealing the cursor after a change

`safe_area.rs` defines the region all of this aims at: the rows of the viewport
a cursor is allowed to come to rest on, everything but `INSET_LINE_COUNT` lines
held back at each edge. `SafeArea::of(rows)` gives it two boundaries - `high`
nearest the top of the screen, `low` nearest the bottom - and the inset gives
way at the viewport's middle so the two can never cross. It is pure geometry;
which way to scroll is decided by the two callers below.

A change made while the cursor is off screen (scrolled away with the wheel or
the thumb, then typed into) brings the cursor back into the safe area - it
lands on the sixth visible line, from the top or the bottom depending on which
way the view had to move. Typing with the cursor already visible doesn't scroll
at all. A change that rebuilds the document - undo, redo, the line commands -
honours the safe area as a whole region instead: it scrolls once the cursor is
past a boundary and still heading for that edge, rather than waiting for it to
leave the view. **Heading for it** is half the rule. A cursor sitting past the
low boundary that just moved *up* is walking away from the edge and gets
nothing; scrolling down to "reveal" it would drag the view the opposite way to
the line the user is moving.

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
  `reveal_offset` only adds the inset.
- `Rebuilt` - `reload_text` and an undo whose delta is too big to splice.
  Undo, redo, toggle-comment and the line commands are `Spliced` instead. The
  fresh buffer starts at the top
  of the document, so the first shape reveals the cursor from *there* - a view
  the user was never at. `layout` scrolls back to where the old `Content` had
  it and then places the cursor itself with `restore_offset`, since nothing is
  going to chase it a second time.

  Because it places the cursor outright, this path can honour the whole
  region: `restore_offset` scrolls once the cursor is past a boundary *and*
  moved that way, and holds still otherwise. Firing only once the cursor was
  fully off screen is what made a held line command stutter - the caret crept
  onto the last visible row with nothing under it, the view sat there, and then
  it jumped a whole inset at once when the caret finally crossed. The scroll
  needs no clamping of its own; `Action::Scroll` already stops at both ends of
  the document.

  Which way the cursor moved is why `PendingView::Rebuilt` carries a whole
  `CapturedView` - the replaced `Content`'s scroll *and* its cursor row - and
  why `Content::capture_view` is the counterpart to `restore_view`. The
  boundaries say the cursor is running out of context; the row it came from
  says which side that context is on. A cursor already off screen is placed
  whichever way it went, having none either side.

  `capture_view` returns `None` until `layout` has shaped the `Content` once
  (`Internal::shaped`). The cursor's row comes from `editor.selection()`,
  which panics on a line cosmic-text has not cached a layout for - and a
  `Content` nobody has laid out has no view worth carrying across anyway.

**A view that moved is not proof the cursor was chased.** Deleting lines under
a view anchored at the end of the document clamps the scroll upwards on its
own, with the cursor still comfortably on screen; backing *that* off by the
inset would push the cursor out of view. `reveal_offset` only fires when the
view moved *and* the cursor came to rest on the first or last visible row,
which is where a real reveal - and nothing else - leaves it. The `Rebuilt`
path has no such ambiguity: it measures the cursor's row against the restored
view directly.

**Don't replace a whole document through `Action::Edit`.** Swapping the
document with `SelectAll` + `Paste` would reuse the `Edited` path for free and
keep the buffer's scroll, so it looks like the obvious simplification. It is
quadratic in what it pastes: measured against `Content::with_text` on the same
document, 2x slower at 10K lines, 17x at 50K, and 35x at 150K (237 seconds).
For a *whole document*, `Content::with_text` plus a restored view is the cheap
way.

This is about size, not mechanism. Pasting over the handful of lines an edit
touched is how undo, redo, toggle-comment and the line commands all work, and
it is orders of magnitude cheaper than rebuilding. `LINES_WORTH_SPLICING` is
where one turns into the other.

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

**A path that knows what it changed can splice the cache instead.**
`splice_source` does one `replace_range` on a copy of the string, where
`resync_source` reassembles line by line - one allocation per line, the 18ms
above. Undo and redo use it; typing can't, since finding the delta would need
the post-edit text `Content::text()` is what produces.

**Add a `resync_source` call to any new code path that mutates the text.**
The tests in `lib.rs` assert `source` matches `content.text()` after every
mutating operation, so drift shows up as a failure rather than as
mysteriously misaligned syntax colors - the highlighter resolves byte
offsets against the cached string, so a stale cache misaligns every span
after the point it diverged. Deliberately *not* a `debug_assert`: that
would re-introduce the linear rebuild in dev builds, where it was worst
(~64ms per redraw at 150K lines).

### Undo history

`History` (`history.rs`) is a stack of deltas. Each step is an
`TextDelta` - the run of characters an edit changed, plus the text on either
side - and a `CursorState` (caret position *and* selection) from just before
the edit. `apply_history` splices the delta back in place and restores both.

**A step costs the edit, not the document.** It used to hold a full copy of
the document per step and rebuild `Content` wholesale, so undoing one
character in a 20K-line file re-shaped 20,000 lines. Measured on 20K lines in
release: a one-word undo is **430us**, against 160-240ms just to rebuild and
reassemble the source - and that excludes the re-shape a rebuilt buffer forces
(most of the 2.16s above). `what_an_undo_costs_on_a_long_document` is the
`#[ignore]`d measurement.

**A burst is held open, not snapshotted per keystroke.** `BurstInProgress`
keeps an `Arc` clone of the source cache from before the burst - a refcount,
not a copy - and `close_open_burst` turns it into one delta when the burst
ends. Only the first edit of a burst is kept, so the step's cursor is the
state from before the whole burst. A burst that ends where it began records
nothing.

**Undo restores the selection, for every edit - typing included.** Matches VS
Code: Monaco's `EditStack` restores `beforeCursorState` for all edits, not
just cut and paste. Selecting a word, typing over it, and undoing brings the
word back *selected*. Two consequences that look odd but are the same rule:
undoing Option/Ctrl+Backspace re-selects the deleted word, and a cut is
indistinguishable from Delete with a selection.

**Deltas are character-precise, not line-rounded.** Typing `my ` stores
`my `, not the line it landed in. Line-rounding used to be how the ends were
made sliceable; with word-sized steps it meant every step carried the whole
growing line - `N x W` bytes for a line of N characters typed in W words,
which on a minified-JSON single line is megabytes per word.

**`TextDelta::between` advances both ends by one shared distance.** Both ends
round out to a UTF-8 character boundary, because a matching run can end
mid-codepoint. The end advances by a distance applied to *both* documents,
never worked out per side - the unchanged tail is identical in each, and one
shared advance is what keeps the two sides the same length, and so describing
a single change.

**Past `LINES_WORTH_SPLICING` (500), undo gives way to a rebuild.** Pasting is
quadratic in what it pastes; a rebuild is linear in the document. Measured
into 20K lines: 0.6ms at 1 line, 38ms at 100, 173ms at 500, 371ms at 1000,
1.50s at 5000, against a 160-240ms rebuild floor. Err low when revisiting -
past the crossover the rebuild grows linearly and the splice does not.

**Undo and redo reveal like a splice, not a rebuild.** They keep the buffer's
scroll and record `reveal_caret_from` (`PendingView::Spliced`), as the line
commands do. `PendingView::Rebuilt` and `capture_view`/`restore_view` remain
for `reload_text` and the oversized delta above.

**Toggle-comment and the line commands ride the same in-place splice.** The
transforms are pure functions in `comment.rs` and `lines.rs`; `TextArea`
applies them through `splice_lines_in_place`, which works the endings out and
hands off to the shared `paste_over`. Each records via
`History::record_isolated`, which breaks the coalescing burst on both sides. A
command that turns out to be a no-op must return `false` *before* recording -
`record_isolated` clears the redo stack unconditionally.

**A spliced-in line inherits the ending of the line it displaces.** Only the
last line carries `LineEnding::None`, and `Content::text()` drops it, so a
line promoted into the last position borrows `document_line_ending()` instead.
Without it, moving the last line of a CRLF file splices in a lone LF. That is
`splice_lines_in_place`'s job. A delta needs none of it - it carries exact
bytes and splices by position.

**A step ends at a word, a caret move, or the timer.** Whitespace and Enter
close it (the space rides with the word it follows, so undoing `hello world`
leaves `hello `), and so does any caret move - typing here, clicking there and
typing again is two edits. `COALESCE_WINDOW` stays as the fallback for edits
with no word boundary, or a held backspace becomes one enormous step.
`ends_undo_step` decides; `History::end_burst` applies it.

**Depth is `[history] depth` in `config.toml`, default 200.** A step is now
roughly a word. `TextArea::update` pushes the live value
into `History::set_depth` on every message, which is how a config reload
reaches tabs that already exist. Clamped to at least one.

**Redo's caret is the state at undo time, not edit time.** Only observable if
the caret moved in between. Left as-is.

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
- **Highlighting resumes where the change was, not at line 0.** iced colors
  `lines[highlighter.current_line() ..= last_visible_line]`, so rewinding to 0
  on every settings change recolors from the top of the document to the bottom
  of the viewport - 19,000 lines per keystroke when scrolled deep.
  `TreeSitterHighlighter::resume_line` picks the line instead. Only an *edit*
  narrows it (`only_the_text_moved_since` is the gate); a find query, a grammar
  landing or a config reload still start at 0. It is `min`-ed with
  `first_recolored_byte`, the first byte where the new spans diverge from those
  on screen - typing `*/` closes a comment opened far above, which cannot be
  read off the edit's own position. `change_line` takes the lower of its
  argument and the current value, since iced calls it afterwards knowing only
  where its edit landed.
- **`highlight_line` binary-searches the spans; it must not scan them.** It
  runs once per line, so filtering the whole span list inside it cost lines
  times spans. Spans are ordered by `start` and never overlap, so the ones on a
  line are one contiguous run. Find matches group by line and get the same
  treatment.
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
  for `wgpu` in this app. Neither figure was measured on macOS and neither
  describes it - see "what the memory numbers actually mean" below.
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
what a theme's `background.alpha` says - it renders the alpha and CoreGraphics
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

## macOS: the softbuffer fork (`j1mmie/softbuffer`)

`iced_tiny_skia`'s `present` only diffs damage when it can identify the previous
buffer, via `buffer.age()`; otherwise it falls back to
`vec![Rectangle::with_size(viewport.logical_size())]` - the whole window.
**Upstream softbuffer's CoreGraphics backend hardcodes `age() -> 0`**, because
it has nothing to age: `buffer_mut` allocates a fresh zeroed `Vec<u32>` every
frame and hands it to a `CGDataProvider`, which frees it once the layer's
contents move on.

That cost two things on every frame, however little had changed: an 8MB
allocation at a Retina window size (mapped fresh, so ~528 page faults as
tiny-skia writes into it), and a full-window rasterization - 2.16M pixels to
blink a caret. Windows and X11 return `1` once presented and get real diffing,
so this was macOS-only.

**The fix** (`src/backends/cg.rs`, `BufferPool`): recycle the allocations
instead of freeing them, which is what makes an honest `age` possible, which is
what lets `iced_tiny_skia` repaint only what moved. The mechanism was already
sitting unused in the upstream code - `CGDataProvider`'s `info` pointer, passed
as `ptr::null_mut()` and ignored by the release callback. The fork passes a
leaked `Arc<BufferPool>` reference there, and the callback reclaims it and
returns the buffer to the pool.

The fork lives at `j1mmie/softbuffer`, branch `jumppad/buffer-pool`, wired in
through `[patch.crates-io]` in the root `Cargo.toml` - same arrangement as the
`iced_graphics` fork below, and branched from the `v0.4.8` release commit
rather than `master` for the same reason: the patch has to keep satisfying the
`^0.4` requirement `iced_tiny_skia` states.

`src/backends/cg.rs` is the only file that differs, so every other platform
compiles upstream's code untouched. Keep it that way - a change reaching into a
second file is a signal to check whether it belongs upstream instead.

**Gotcha - `cargo test` in that fork fails one doctest, and it is not ours.**
The crate's docs are `#![doc = include_str!("../README.md")]`, and the example
block there does `#[path = "../examples/utils/winit_app.rs"] mod winit_app;`
while the published package sets `exclude = ["examples"]`. Reproduced against
pristine crates.io 0.4.8 sources - same test, same line - so it predates this
fork and is not worth fixing here. `cargo test --lib` runs the pool's own tests
without it.

**The rotation settles at two buffers, so `age` settles at 2.** Core Graphics
holds the frame on screen until the layer takes the next one, so a buffer is
never free the frame after it went out: frame N's buffer comes back during
frame N+1's present and is reused for frame N+2. `MAX_POOLED_BUFFERS` is 3 -
two for the rotation, one of slack for when Core Animation holds one longer.
Unit tests in that file pin the arithmetic, `the_rotation_settles_at_two_buffers`
most directly. They are `cfg(target_vendor = "apple")` along with the rest of
the module, so they only run on a Mac.

Gotchas, all of them load-bearing:

- **A recycled buffer must not be zeroed.** Its stale pixels are the entire
  point - `age` promises the caller they survived, and the caller draws only
  the damage on top of them. Zeroing while reporting a non-zero age would show
  a window with holes in it.
- **Too small an age is the dangerous direction, not too large.** It points the
  caller at a more recent frame than the buffer actually holds and skips the
  difference; too large only costs redundant drawing. So an age that overflows
  `u8` reports `0` (undefined, repaint everything) rather than clamping.
- **`reclaim` removes the pointer from `held_by_core_graphics` before the
  size check**, so a buffer retired by a resize still clears its entry. Leaving
  it would strand a stale present index under an address the allocator can hand
  out again.
- **The release callback can run on any thread.** Core Graphics gives no
  guarantee about which, hence the `Mutex` rather than a `RefCell`.
- **Rotation depth is what damage tracking is measured against.** A frame is
  diffed against whatever the buffer it lands in still holds, which is
  `MAX_POOLED_BUFFERS` frames ago - `a_document_swap_repaints_through_a_rotating_swapchain`
  in `jumppad_textarea`'s `tests/repaint.rs` covers that case.

**Gotcha - `age` deliberately reports `0` today, and the reason is not in this
crate.** The pool tracks the real age correctly and reporting it works
spectacularly on the numbers: measured with `SOFTBUFFER_TRACE_AGE=1` (plus
`SOFTBUFFER_TRACE_FRAMES` to raise the frame budget), idle drawing fell from
~35ms to **0.02ms**, at both a 3/4-screen and a maximized window - it stopped
scaling with window area at all, which was the entire point.

It also rendered incorrectly, which is why it is off - a strip of
superimposed text above and below the editor while scrolling, refreshed on
every caret blink. **That has since been diagnosed and fixed**, in
`text_clip` (see "Clipping the text"): the editor paints the rows the edges
cut through whole, and the software renderer was skipping the clip mask that
should have stopped the overhang, so it landed outside the regions anything
repaints. The prediction made here held - Windows and X11 report a real age
and had the same strips all along, which is where the bug was finally
reported from.

The diagnosis written here at the time was that
`iced_graphics::text::editor::Internal`'s `PartialEq` compares font, bounds
and line metrics and nothing else, so a scrolled editor compares equal to its
own previous frame and damages nothing. **That part is wrong**, and worth
knowing before it sends someone chasing it again: `Editor::update` mints a
fresh `Arc<Internal>` on every layout, so the previous frame's weak reference
dangles and the comparison fails before it compares anything. The editor
damages its own bounds every frame. What it never damaged was the band
*outside* those bounds, which is exactly where the strips were.

**To re-enable, in order:**

1. On a Mac, `SOFTBUFFER_DAMAGE_TRACKING=1 cargo run_release`. The variable
   makes `age` report the real number without a rebuild.
2. Scroll a syntax-highlighted file for a while, at speed and by single
   lines, and watch the bands directly above and below the text. Leave it
   sitting still afterwards - the caret blink is what used to bring the old
   strips back. Switch tabs a few times too, now that nothing forces a full
   repaint there any more.
3. If it is clean, make it the default in the `j1mmie/softbuffer` fork
   (`jumppad/buffer-pool`): `age` becomes `self.age` with the
   `damage_tracking_enabled()` gate and its two helpers deleted, and this
   section becomes a note that it is on. `cargo update -p softbuffer` brings
   it in.
4. Re-measure. The number this buys back is idle drawing at 0.02ms instead of
   ~35ms; **give it the ~150 frames it takes to settle** (see the gotcha
   below), or the measurement will say it did nothing.

Nothing in the iced fork needs changing first any more, and
`jumppad_textarea`'s `tests/repaint.rs` covers the failure this was reverted
for, on the same damage-tracking path - but it covers the widget, not the
window, which is why steps 1 and 2 are still a real Mac.

The recycling half of this fork is unaffected and still pays for itself. It was
always two wins; only the second one is blocked.

One number worth not re-deriving: `present` sits at ~5ms and rises to ~16ms
once the app settles. That 16ms is one frame at 60Hz - `CATransaction::commit`
waiting on vsync, not CPU being spent.

**Gotcha for whoever re-runs this - the idle plateau takes ~150 frames to
arrive**, and at 2 blinks per second that is over a minute. A shorter sample
catches the app mid-settle and reads as "damage tracking is not working" -
which it did, twice, and cost several rounds of chasing a bug that was not
there. There is also an intermediate plateau around 2.8ms before the final
0.02ms one; it has not been identified, and syntax highlighting's 50ms poll is
the first suspect.

**This does nothing for `jumppad-gpu`.** `iced_wgpu` has no damage tracking at
all, and its cost is not per-pixel anyway - see the memory note below.

One consequence for the "zero idle CPU" goal in `README.md`: it holds for
JumpPad's *scheduling* (see the scrollbar's `next_redraw`), but with `age`
reporting `0` the 500ms caret blink still costs two full-window repaints per
second whenever the window is focused. Shrinking those to caret-sized is
exactly what the blocked half of this fork would buy. Either way it is two
presents per second for as long as the window is focused, which is what the
memory note below is about.

## macOS: what the memory numbers actually mean

Measured with `vmmap --summary`, which is what Activity Monitor's "Memory"
column reports (`phys_footprint`). Numbers from a 900x600 window at 2x on
macOS 15.7.

`jumppad-gpu` reads ~530MB focused and ~75MB unfocused. **95% of the focused
figure is the Metal driver's own arena, not JumpPad**: `IOAccelerator
(graphics)` alone is 456MB across ~146 regions, of which 360MB is marked
`VOLATILE` - discardable by the kernel on demand. JumpPad's actual heap across
every malloc zone is ~14MB, and `Writable regions: written` is ~17MB.

Three things follow, and each has been mistaken for a bug at least once:

- **The mapped driver is not in the footprint.** `__TEXT` is 1.0G virtual and
  663MB resident but **0K dirty**; clean file-backed pages do not count. That
  is why RSS reads 1.4G while the footprint reads 530MB.
- **The arena is held while the app presents frames, not while it is focused.**
  Terminal.app, with cursor blinking off (its default), sits at ~100MB; turn
  blinking on and it holds ~419MB. Same mechanism, same driver. JumpPad blinks
  unconditionally, so it never reaches the quiet state.
- **A comparison against TextEdit is not like-for-like.** TextEdit never
  creates an `MTLDevice`, so it pays none of this. The fair comparison for
  `jumppad-gpu` is Terminal at ~392MB.

So the GPU build's memory is a fixed driver cost plus ~75MB of JumpPad, and the
only lever that would move it is not presenting frames when nothing changed -
i.e. stopping the caret blink after an idle period. That is a deliberate
behaviour change and has not been made.

**The software build is 55.5MB, and 60% of it is the window's own pixels.**
Same window, same machine. Every row below is `vmmap` dirty size, and they sum
to the reported footprint:

| What | Dirty | Share |
| --- | --- | --- |
| Frame buffers - `MALLOC_LARGE` 16.5M live, 8.26M freed-but-charged | 24.8M | 45% |
| CoreAnimation's own copy of the presented frame | 8.5M | 15% |
| Heap - document text, undo history, spans, widget state | 14.7M | 27% |
| Private writable data from ~950 loaded libraries | 3.7M | 7% |
| Page tables, stacks, ColorSync, CoreUI, misc | 3.9M | 7% |

Three things worth knowing before optimising against this:

- **`MALLOC_LARGE` is exactly two regions of 8.24MiB**, which is 1800x1200x4 -
  the pool, visible in the allocator. It scales with window area and Retina
  scale, so a maximised 5K window moves this number a lot and nothing else on
  the list moves at all.
- **CoreAnimation copies the frame.** 8752K here against 288K on the GPU build,
  a difference of exactly one buffer. Presenting through `setContents` with a
  `CGImage` appears to cost a CA-side copy. Avoiding it means presenting an
  `IOSurface`-backed layer instead, which is a much larger softbuffer change
  than the pool was.
- **`MAX_POOLED_BUFFERS = 3` costs 8.26MB**, 15% of the whole footprint - the
  peak footprint of 63.8M against 55.5M current is that third buffer, allocated
  once and since freed with its pages still charged. Dropping the cap to 2 is a
  one-line change, but do not make it before confirming the pool still reaches
  a non-zero `age`; two buffers is the exact rotation depth, with no slack for
  Core Animation holding one longer.

## The repaint nudge, and why it is gone

`JumpPadApp` used to hold a `redraw_nudge_frames` counter that dropped the
theme's background alpha by an invisible amount for a few frames after a tab
switch. The compositor repaints everything when a frame's background color
differs from the last one's, so that bought a full repaint - working around
`iced_graphics::text::editor::Internal`'s `PartialEq`, which compares font,
bounds and line metrics and never the text, and so could call a switched tab
unchanged.

**That comparison is answered in the fork now, rather than dodged from
outside.** `PartialEq for Weak` compares which editor the reference points at
before comparing how it is laid out, so a different document is never
"unchanged" however identical its font, bounds and metrics are. The counter,
its message, its `window::frames()` subscription and `nudge_background` are
gone with it, and `tests/repaint.rs` in `jumppad_textarea` pins what they were
covering on the real damage-tracking path: a document swap repaints, a
document swap repaints through a rotating swapchain, a switch between two
*live* documents repaints, and a palette change repaints.

**The one that got away, and why the fourth of those tests exists.** Deleting
the nudge rested on the previous frame's weak reference always dangling -
`Editor::update` mints a fresh `Arc<Internal>` on every layout, so `Weak::eq`
gives up before comparing anything. True while the same document is being
redrawn. Not true of a tab switch: the previous frame's reference points at
the tab being *left*, which is no longer laid out and whose `Content` still
holds it, so both sides upgraded, the comparison ran, and two documents in the
same widget answered it identically. The editor asked for no damage at all,
the old text stayed on screen, and the first event after the switch - a
pointer move, a modifier coming up - laid the widget out again and repainted
it. Switching tabs looked like a delay rather than a wrong picture, which is
what took so long to place.

**If ghosting ever comes back, it is one of two things, and they are worth
telling apart before reaching for a nudge again.** Content that is stale
*inside* the editor means something the comparison still cannot see has
changed - fix it in the `iced_graphics` fork, next to the identity check,
rather than by defeating the check from outside. Old text *outside* the
editor is the overhang instead (see "Clipping the text"), and no amount of
repainting fixes it - the paint is going where nothing repaints on purpose.

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

With a theme's `background.alpha < 1.0`, `JumpPadApp::style` hands iced a
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
lives in the widget (`clamp_scroll_multiplier`), not in
`jumppad_config`, which keeps this crate free of a `iced` dependency
the same way `BackgroundConfig` does. The multiplier is applied to a *float*
line count, which is then turned into pixels and sent through
`Content::scroll_by` - so a fractional sensitivity means fractional
*movement*, not a banked remainder waiting to make a whole line. That is
why the floor of `SCROLL_MULTIPLIER_RANGE` can sit as low as `0.05`
and still be a usable setting rather than a dead wheel.

`[scroll] drag_speed` is the same shape of knob for the other way the view
moves on its own: how fast a selection dragged past the top or bottom edge
walks the document (see the text-area fork's section on selecting past the
edges). It defaults to `1.0`, takes the same range through the same clamp,
and is read live on each `view` like the rest of `SharedEditorConfig`.

Before pixel scrolling this knob could only make the wheel *slower*, not
smoother: every event still moved the view a whole line or not at all,
and a low sensitivity just meant more events that moved nothing. Anyone
reintroducing a whole-line path should expect that complaint back.

`[indentation]` is one setting with two halves, deliberately: `style` decides
what Tab inserts - one tab character, or the spaces that reach the next stop
from where the caret is drawn - and `width` is the columns between stops. The
width applies in **both** styles, because it is also how wide every tab already
in the file is drawn; a document whose tabs draw at one width and indent at
another is the thing this exists to prevent. It defaults to tabs at 4, and
reaching the screen at all needs `Editor::set_tab_width` from the
`iced_graphics` fork (see the patch section above).

**`style`, not `mode`.** `[mode]` is already a section of this file - the
theme's light/dark `detection` - so a second, unrelated "mode" in the same
config read as the same word twice. Nothing else answers to `style` here.

The range check lives in `Indentation::new` (`jumppad_textarea`'s `indent.rs`),
which is the only constructor there is, so a width out of range cannot reach
either the arithmetic or the buffer - the same division of labour as
`clamp_scroll_multiplier` and `font::clamp_size`, and for the same reason:
`jumppad_config` stays free of any opinion the widget owns. Zero matters more
than the ceiling does, since cosmic-text ignores a zero outright and the drawn
width would then disagree with the inserted one.

**Tab is an `Action`, not a `Binding::Insert('\t')`.** The spaces style needs
a string computed from the caret's *visual* column, which `binding_for` cannot
see, so `Action::Indent` arrives as `EditorMessage::Indent` and `TextArea::
indent` builds it. Visual, not byte: `indent.rs`'s `visual_column` walks the
line counting a tab as a jump to its stop, so an indent typed after existing
tabs lands on a stop rather than a byte count. That is exact in the monospace
face the editor defaults to and approximate in a proportional one, where
cosmic-text puts its stops at multiples of a space's advance instead.

The indent rides the undo history the way any other whitespace does - back
with the word it followed, closing the step behind it - which `Edit::Insert`
gets from `ends_undo_step` for free and `Edit::Paste` does not, hence the
explicit `end_burst` in `indent`. Teaching `Paste` to end a step instead would
change what the clipboard does.

Shift+Tab is deliberately unbound: outdent, multi-line indent, indent-aware
Enter, autodetection and a status readout are all still to come, and
`Mods::matches` being exact is what keeps Shift+Tab from falling through to
plain Tab in the meantime.

### Themes and the base theme

`[themes.base]` is the defaults every other theme inherits, **property by
property, not section by section**: `[themes.base] editor.font.family`
plus `[themes.dark] editor.font.size` gives the dark theme both. A
theme's own property wins, and what neither theme names takes JumpPad's
own default.

That per-leaf rule is the whole reason every leaf in `ThemeConfig` is an
`Option` and `ResolvedTheme` is a separate flat type. `background.alpha
= 1.0` over a translucent base has to be distinguishable from saying
nothing at all, so `BackgroundConfig::default()` means *unset*, not
*solid*. Anyone "simplifying" those `Option`s back to plain floats
breaks a theme's ability to be solid over a see-through base, and no
type error will say so.

The two `[mode] theme` slots default to `light` and `dark`, so a file
holding `[themes.base]`, `[themes.light]` and `[themes.dark]` needs no
`[mode]` section at all. A slot naming no theme is still read as a
palette name, and `theme_named` expresses that as a theme naming only
that palette - which is what makes the slot's palette outrank the base
theme's rather than the other way round. `Appearance::default_palette()`
serves both readings with one lowercase string; palettes are matched
case-insensitively where they're applied (`resolve_palette` in
`app.rs`), theme keys are not.

Two things that look like bugs and aren't. `wants_transparency` does not
merge the base theme in first: base is itself a `[themes]` entry, so a
translucency only it names is already counted, and every other theme's
alpha is either its own or the one base contributed - which does mean a
base a theme fully overrides still opens a transparent window, the same
rule as any theme no slot names. And a theme literally called `base` is
the feature, not a collision: an old config that happened to have one
changes meaning.

**`[mode] detection = "auto"` needs the window's appearance left alone, on
macOS.** winit reports a light/dark switch by watching the window's
`effectiveAppearance` and deliberately says nothing while an appearance is
pinned - a pinned one only ever changes because the app changed it. iced pins
one from `theme()`'s mode as every window opens, so the switch never reached
the runtime, nothing was broadcast, `system::theme_changes()` never fired, and
the theme sat where it was for the rest of the session.

`macos::pin_appearance` is what settles it, from `sync_window_appearance` in
`app.rs`: the appearance is pinned to the slot `[mode]` names, and cleared
whenever `detection` is `auto`. That reads as one rule - **the window's chrome
is pinned exactly when the theme is** - and it keeps the title bar honest in
both directions, since a following theme already agrees with the OS.

The same call takes the OS's own answer on the way past
(`macos::system_appearance`, read from `NSApp` rather than from a window,
which would answer with its pin). That is not belt and braces: a session that
has been pinned heard nothing from the OS while it was, so its answer is stale
exactly when a reload turns `auto` back on. The read travels as
`Message::SystemAppearanceReported`, the same message the runtime's own report
arrives as.

**No other platform needs this.** Windows keeps its `preferred_theme` at
whatever the *window attributes* said, which iced never sets, so
`WM_SETTINGCHANGE` still reports switches whatever `set_theme` did to the
title bar; Linux reads the XDG portal instead. The rule above is a no-op
there, and `sync_window_appearance` is a no-op function.

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
changed. Anything a *theme* carries goes in `apply_theme` instead, which
`apply_config` and an OS light/dark switch both call, so the two can't
drift. One `[[languages]]` edit can feed two consumers, so `apply_config`
diffs its *derived views*: the comment-style map applies live, the
extension-to-grammar map is baked into the registry at startup and logs
restart-required - the one setting still waiting on a restart.

A window is handed its transparency, its decorations and its level when
it is created and has no setter for any of them after, so those don't get
an arm: they get a new window. `window::settings` (`window.rs`) says what
window a config describes - `run()` builds the first one from it,
`window::replace` builds every later one - and `window::needs_replacing`
diffs two configs through it, so **a new creation-time setting joins by
being named in `settings`, not by growing an arm.** The replacement opens
before the old window closes, because iced ends the program when the last
window is destroyed. It ends at `Message::WindowReady`, the arm every
window arrives through, which is also where a new window is handed the
platform setup and the keyboard focus its own widget tree doesn't carry;
the caret and the selection do carry, since they live in the document.

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
overrides, a theme's `background.alpha`, and the two `[scroll]` speeds. The
scroll speeds are the ones that arm no repaint - nothing on screen changes
until the next wheel event or selection drag, and the `view` that one causes
is where the widget picks the new value up. `foreground.alpha` rides the same
setter but is stored in a static atomic (`to_format` must stay a bare
`fn` pointer), and it also sits in `HighlighterSettings`' hand-written
`PartialEq` - without that, syntax-colored spans keep their old alpha
until an unrelated edit, same bug class as the find-state fields. A visual
apply needs nothing else to reach the screen: every one of them moves a color
the compositor compares, and `a_theme_change_repaints_the_editor` in
`jumppad_textarea`'s `tests/repaint.rs` pins that.

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

## The icon font (`jumppad_icons`, `icon_font_builder`)

Icons are glyphs in a font JumpPad builds itself, drawn as text like any
other label. That is the cheap way to do it here: iced already rasterizes
and caches a glyph for every label on screen, so an icon costs one more
entry in a cache the app pays for anyway. The alternatives both cost
something the app has deliberately given up - drawing the shapes as geometry
wants the multisampling `run` turns off, and rasterizing SVG at runtime
wants `resvg` in the binary.

`assets/icons/*.svg` are [Lucide] drawings, vendored as they ship.
`jumppad_icons` says which of them are in the font and what codepoint each
one answers to. `cargo build_fonts` runs `icon_font_builder` over that list
and writes `assets/fonts/jumppad-icons.ttf`, which is committed: the build
is deterministic, so rebuilding without changing an icon leaves the tree
clean, and nobody needs the tool to compile the app.

Adding an icon is a file in `assets/icons/`, a row in `jumppad_icons::ICONS`
with the next free codepoint, and `cargo build_fonts`.

### The stroke width is baked in, and has to be

Lucide draws with strokes. OpenType has no stroke - a glyph is filled
contours and nothing else - so `icon_font_builder` expands each centreline
into the outline a round 3-unit pen would leave, and that weight is fixed
from then on. No draw call can vary it. Wanting a second weight means a
second glyph, not an argument.

What does scale is the size: the weight is a proportion of the icon like
everything else in it, so a 3-unit stroke on Lucide's 24-unit grid draws 2px
at `size(16)`.

The other thing the builder settles is where an icon sits beside text. The
24-unit box maps onto a whole em, so `size(16)` draws what a browser would
draw at `width="16"`, and the box straddles the middle of a capital letter
rather than resting on the baseline.

### Codepoints are ours

They start at U+E000 in the Private Use Area and are assigned in
`jumppad_icons`, not copied from Lucide's own font. Lucide reassigns those
between releases; these never move, so re-vendoring a drawing cannot quietly
change what the UI draws.

The font is stored in Git LFS. A clone made without it leaves a text
pointer where the font should be, and nothing downstream would say so - a
face that fails to parse is declined, `.notdef` is empty on purpose, and
every icon draws as nothing at all. `jumppad_icons` checks the sfnt tag at
compile time so that clone fails with a sentence instead.

### Drawing one

`run` hands the bytes to iced at startup; `app.rs` selects the face by the
family name the font records, and `UiText::tab_icon`/`control_icon` draw a
glyph at the size of the text it sits among:

```rust
button(ui.control_icon(jumppad_icons::CLOSE))
```

Icons are drawn a little larger than that text - `ICON_SCALE` - because a
letter's ink fills about half its em box while an icon's fills a bit more,
so at a matched size an icon reads as the smaller of the two. Only the size
scales: the line height stays the text's, so the tab strip keeps the
whole-pixel height the transparent-window seams depend on.

[Lucide]: https://lucide.dev

## Miscellaneous things worth knowing before you "fix" them

- `lib.rs` has `windows_subsystem = "windows"` (which hides the console
  window on release builds) temporarily disabled, with a comment saying
  why: an in-progress Windows highlighting bug needed console output to
  debug. Don't silently re-enable it as a "cleanup" without checking
  whether that investigation is actually finished.
