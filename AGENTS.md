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
  iced_text_editor/  the one TextEditorWidget impl today, wrapping iced::widget::text_editor
  syntax_registry/   loads/caches/refcounts tree-sitter WASM grammars; no iced dependency
  jumppad_config/    config.toml loading + defaults; no iced dependency
syntaxes/            *.wasm grammar files + *.injections.scm queries (see below)
```

Dependency direction is one-way: `jumppad` depends on everything;
`editor_core`, `syntax_registry`, and `jumppad_config` depend on nothing
inside this workspace. `iced_text_editor` depends on `editor_core` (to
implement its trait) and `syntax_registry` (to drive highlighting), but
not on `jumppad`. This is deliberate - see `editor_core::widget::TextEditorWidget`'s
doc comment: a future non-iced or non-text_editor-based editor widget
should be a new crate implementing that trait, not a rewrite of `jumppad`.

## The `TextEditorWidget` boundary

`editor_core::widget::TextEditorWidget` (view / update / text / set_text /
poll_highlighting / has_pending_highlighting) is the entire contract
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

**Windows is the mirror image of that** - there it's `jumppad-gpu` that
can't be translucent, and `jumppad` that can. Full evidence later in this
section; the practical upshot is that neither binary does transparency
everywhere, so always establish which one a report came from first.

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

**Linux is deliberately not in the gate.** Wayland and compositing X11 are
premultiplied as well, so it probably belongs there too, but nobody has
reproduced the symptom on either. Turning it on blind would cost real
opacity on a window that currently looks right; wait for a screenshot.

**Windows is not in the gate either, for a much harder reason - see the
next section. Do not "fix" a washed-out Windows window by adding
`target_os = "windows"` to `CLEAR_COLOR_NEEDS_PREMULTIPLY`.** That was
tried, shipped, and reverted. It is the intuitive read of the symptom and
it is wrong.

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

**Gotcha - on Windows, transparency requires `jumppad`, not `jumppad-gpu`.**
The exact mirror image of the macOS rule near the top of this section, and
the two together mean **neither binary is translucent on every platform**.

Reported as "on Windows, in wgpu, the theme is much lighter when
transparent", with `jumppad` and `jumppad-gpu` side by side on the same
config. The tempting diagnosis is the macOS alpha-convention bug (a
straight clear color composited premultiplied lands `(1 - a) * rgb` too
bright, which also looks "lighter"). It isn't that. **`jumppad-gpu` on
Windows is not translucent at all** - it is a fully opaque window showing
the theme's background at its true color, which simply reads as "lighter"
next to a genuinely translucent one.

The chain, all confirmed in the vendored sources rather than guessed:

- `wgpu_hal::dx12::Adapter::surface_capabilities` returns
  `composite_alpha_modes: vec![Opaque]` for `SurfaceTarget::WndHandle` -
  a swapchain built directly from the window's `HWND`.
- That is the default. `Dx12SwapchainKind::DxgiFromHwnd` is
  `#[default]`, and its own rustdoc says verbatim: *"This does not support
  transparency."* The alternative, `DxgiFromVisual`, wraps the `HWND` in a
  DirectComposition visual and does report the full alpha-mode set.
- `iced_wgpu::window::Compositor::request` picks `PostMultiplied`, else
  `PreMultiplied`, else falls through to `Auto`. Given `[Opaque]` it takes
  `Auto`, and the swapchain discards alpha.
- Nothing app-side can change that. `iced_wgpu` builds its
  `wgpu::InstanceDescriptor` with `..Default::default()`, so
  `backend_options.dx12.presentation_system` is `DxgiFromHwnd`. The
  `WGPU_DX12_PRESENTATION_SYSTEM` env var looks like a lever but is
  **dead**: it is only read by `Dx12SwapchainKind::from_env`, which nothing
  in `wgpu`, `wgpu-core`, or `wgpu-hal` calls on the default path (grep for
  `with_env` in those crates - no hits). Setting it in the environment does
  nothing.
- The other backends are no better: `wgpu_hal::gles` hardcodes
  `vec![Opaque]`, and Win32 Vulkan surfaces report only
  `VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR`. `iced_wgpu` asks for
  `Backends::all()` anyway, so there is no backend to steer it toward.

So reaching transparency through `wgpu` on Windows needs
`[patch.crates-io]` on `iced_wgpu` to pass `Dx12SwapchainKind::DxgiFromVisual`.
That is the only known route; weigh it against `jumppad` already doing the
job (`tiny-skia` stores premultiplied bytes and softbuffer blits them to
the DIB, which is exactly what the DWM wants), and against DirectComposition
surfaces not being supported by RenderDoc.

**Why the premultiply made it worse, which is the useful diagnostic.** With
alpha discarded, the window shows the clear color's RGB at full opacity.
Straight, that is the theme background - correct color, just not
see-through. Premultiplied, it is `background * alpha` - a *darkened*
opaque window. If a Windows screenshot ever shows `jumppad-gpu` too dark
rather than too light, that is this gate being re-added, not a new bug.

`OPAQUE_WINDOW_REASON` in `lib.rs` warns at startup on both of these
platform/backend pairings, so the next report arrives as "it printed this"
rather than as another screenshot to re-derive.

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
them, with the bounds implied by the kind. `IcedTextEditor::selection`
tells them apart from a collapsed leftover range by whether selected text
exists (and picks Word vs Line by comparing it to the anchor's line), and
restore replays `SelectWord`/`SelectLine` at the anchor rather than
setting an anchor-to-cursor range. Don't "simplify" the saved state back
to a bare anchor pair - that's exactly the representation that can't
describe a word selection.

## The find palette

Cmd+F / Ctrl+F opens a floating palette at the top-right of the editor
area (`JumpPadApp::find_palette`, stacked over the editor so it never
covers the tab bar). Search is case-insensitive, single-file, no toggles.

Cmd+G / Ctrl+G is find-again. It works with the palette **closed**, using
that tab's stored query, and deliberately does not reopen it - so it
re-searches first (the document can have changed while the palette was
shut) and re-anchors to the cursor rather than to wherever the palette was
left. With the palette closed the editor holds focus, so the match shows
as an ordinary selection and `select_current_match` skips the tinting.

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
  `Buffer`. `iced_graphics::text::Editor::buffer()` is public, but
  `text_editor::Content` is a newtype over a private
  `RefCell<Internal<R>>` whose `editor` field is private, so there is no
  path to it from application code.
- Faking it with an overlay computed from monospace metrics founders on
  the scroll offset: the app sees `Action::Scroll` from the wheel, but
  cursor-driven auto-scroll (`shape_until_cursor`) is internal, so any
  tracked offset drifts.
- 0.14.0 is the latest published iced, so there is no version to upgrade
  into.

Getting real background highlighting would mean patching iced *and*
cosmic-text, or replacing `text_editor` with a custom widget - the
`TextEditorWidget` boundary exists precisely so the latter is possible.
The current match does get a genuine selection background whenever the
editor holds focus (after Escape, or during a Cmd+G find-again).

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
background.** The editor (`editor_style`, `iced_text_editor`) and the
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

## Config (`jumppad_config`)

`config.toml` is looked for next to the running executable first, then
in the current directory (a `cargo run` convenience). If nothing is
found, built-in defaults are written to the first location. A malformed
config file logs an error and falls back to in-memory defaults rather
than failing to start - a broken config should never be able to prevent
the editor from opening. Config sections (`syntaxes`, `theme`) are
independently defaulted so old config files stay valid as new sections
get added.

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
