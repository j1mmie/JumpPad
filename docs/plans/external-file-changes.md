# Plan: react to files changing on disk (VS Code's behavior)

Implementation plan for making JumpPad notice when an open file changes
underneath it, and respond the way VS Code does. Written to be handed to a
fresh session; read `AGENTS.md` first, especially `## Opening files`,
`### Live reload`, `### Undo history`, and `### Revealing the cursor after a
change`.

Delete this file once the work lands.

---

## 1. The behavior being copied

VS Code's rule is "silently reload when it's safe, never clobber unsaved
edits when it isn't."

| On-disk event | Buffer clean | Buffer dirty |
| --- | --- | --- |
| File modified | Reload silently, no prompt. Reload is undoable (Ctrl+Z restores the pre-reload text and leaves the buffer dirty). Scroll position preserved. | Buffer is left alone. The conflict surfaces at save time: "the content of the file is newer" → Overwrite / Compare / Cancel. |
| File deleted | Editor stays open (`workbench.editor.closeOnFileDelete` is `false`), content preserved, buffer becomes dirty so the next save recreates the file. | Same — already dirty, nothing to preserve beyond keeping the tab. |

Two deliberate scope cuts for JumpPad:

- **No Compare.** JumpPad has no diff view and shouldn't grow one. The
  conflict dialog offers **Overwrite** / **Discard & Reload** / **Cancel**.
- **No auto-save interaction.** JumpPad has no `files.autoSave` equivalent
  (drafts are a separate crash-recovery mechanism and never write to the
  user's file), so there is no autosave-vs-conflict case to handle.

---

## 2. Architecture at a glance

The pieces mirror `crates/jumppad/src/reload.rs`, which already solves the
same problem for `config.toml`/`keybinds.toml`. Read it before writing
anything — the new code should look like a sibling of it, not a new idiom.

Three signals, one decider, exactly as `ConfigWatch` does it:

1. **Native file-system events** — `notify`, watching the *directories* of
   open files (a directory watch survives the delete/rename dance editors
   use for atomic saves; a file watch is lost with the old inode).
2. **A fingerprint sweep on window focus** — the safety net for events the
   watcher never delivered. `Message::WindowFocused` already exists.
3. **JumpPad's own saves** — handled implicitly by re-stamping the file
   after a write, so the resulting watcher event compares equal and is a
   no-op. No suppression list needed.

All three converge on one function, `JumpPadApp::sweep_documents`, which
stats every file-backed tab and applies the table in §1. There is no
path-matching or canonicalization anywhere: an event only pokes a debounce,
and the sweep re-derives the truth from `stat`. A spurious poke costs N
stats for N open tabs, which is nothing.

---

## 3. Phase 1 — `DiskStamp` and tab disk state

*No user-visible change. Land and test this alone.*

### 3.1 New: `crates/editor_core/src/disk.rs`

```rust
/// On-disk identity cheap enough to stat on demand. mtime granularity is
/// filesystem-dependent, so a same-length rewrite inside one tick can slip
/// past a comparison - accepted, same as `reload.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStamp {
    pub mtime: SystemTime,
    pub len: u64,
}

impl DiskStamp {
    /// `None` when the path doesn't exist or can't be stat'd.
    pub fn of(path: &Path) -> Option<Self>;
}
```

Re-export from `editor_core/src/lib.rs` next to the existing exports.

### 3.2 Refactor `reload.rs` onto it

`reload::Fingerprint` becomes `{ path: PathBuf, stamp: DiskStamp }` and its
`fingerprint()` helper uses `DiskStamp::of`. `Fingerprint` keeps its `path`
field — config has *candidate* paths and needs to notice which one is
effective; documents have one fixed path and don't. Its existing tests
(`observe_reports_every_kind_of_movement_once`, etc.) must still pass
unchanged apart from the constructor shape.

### 3.3 `Tab` gains disk state — `crates/editor_core/src/tab.rs`

```rust
pub struct Tab {
    // ...
    /// The file as JumpPad last saw it. `None` for an untitled tab, or one
    /// whose file isn't on disk (named on the command line and never saved,
    /// or deleted underneath us).
    pub disk: Option<DiskStamp>,
    /// The file changed on disk while this tab had unsaved edits. Drives the
    /// banner and the save-time conflict prompt; cleared by a reload, a
    /// successful save, or the user acknowledging it.
    pub externally_changed: bool,
}

impl Tab {
    /// Re-reads this tab's on-disk identity. Called after a load, a save, and
    /// every reload.
    pub fn restamp(&mut self);
}
```

Constructors (`untitled`, `from_file`, `restored`) default both to
`None`/`false` — deliberately no arity growth and no surprise I/O inside
`editor_core`. The app calls `restamp()` explicitly.

### 3.4 Stamp at every point a tab and its file agree

- `open_loaded_file` (app.rs:515) — both branches, after the tab is built.
- `Message::FileOpened(Ok(..))` (app.rs:925).
- `Message::FileSaved(id, Ok(..))` (app.rs:979) — see §3.5, use the stamp
  the save task returns rather than a fresh `restamp()`.
- `Message::SessionFileLoaded(id, Ok(..))` (app.rs:1243) — the startup
  re-read of a restored clean tab is the boot-time version of this whole
  feature.
- The session-restore loop in `new()` (app.rs:363-418), for tabs that get no
  re-read task.

### 3.5 Stamp the file the save just wrote

`save_to` (app.rs:2241) returns `Result<PathBuf, SaveError>`; widen it to
`Result<(PathBuf, Option<DiskStamp>), SaveError>`, stat'ing immediately
after `tokio::fs::write`. Stamping inside the task closes the window where
the app stamps a file that was already re-modified externally. `Message::
FileSaved`'s payload changes shape accordingly.

**Checkpoint:** `cargo test --workspace && cargo clippy --workspace`. Nothing
observable has changed yet.

---

## 4. Phase 2 — an undoable, view-preserving reload

`TextEditorWidget::set_text` (widget.rs:24) rebuilds `Content` wholesale:
the view jumps to the top of the document and no undo step is recorded.
That is right for "load a different file into this tab" and wrong for
"this file changed underneath you."

### 4.1 New trait method — `crates/editor_core/src/widget.rs`

```rust
/// Replaces the document with new on-disk contents, keeping the scroll
/// position, clamping the caret, and recording an undo step - an external
/// reload undoes like any other change.
fn reload_text(&mut self, text: &str);
```

### 4.2 `TextArea` impl — `crates/jumppad_textarea/src/lib.rs`

```rust
fn reload_text(&mut self, text: &str) {
    self.history.record_isolated(&self.source, self.cursor_state());
    let cursor = self.cursor_position();
    self.replace_document(text);
    self.move_cursor_to(cursor.0, cursor.1); // clamps if the file shrank
}
```

`replace_document` (lib.rs:278) is the `Content::with_text` + `restore_view`
tail shared with undo/redo and toggle-comment. **Never SelectAll+Paste** —
AGENTS.md measures that path at 35x slower on a 150K-line file.
`record_isolated` is right for the same reason toggle-comment uses it: the
reload neither joins the typing burst before it nor absorbs the keystroke
after it. `replace_document` already calls `resync_source`.

### 4.3 Stub impls to update

Adding a trait method breaks four test stubs. All need a no-op or
`self.0 = text.to_string()` body:

- `app.rs` — `StubEditor` (:2279), `RecordingEditor` (:2318),
  `SelectionSpyEditor` (:2364)
- `session.rs` — `StubEditor` (:170)

### 4.4 Tests

In `jumppad_textarea/src/lib.rs`, next to
`undo_and_redo_keep_the_cached_source_in_sync`:

- `reload_text_replaces_the_document_and_is_undoable` — undo returns the
  pre-reload text.
- `reload_text_keeps_the_cached_source_in_sync` — use the existing
  `assert_source_is_synced` helper. A stale `source` misaligns every syntax
  span past the divergence point (AGENTS.md, `### The source cache`).
- `reload_text_clamps_a_caret_past_the_end_of_a_shortened_file`.

---

## 5. Phase 3 — detection and the clean-tab silent reload

### 5.1 New: `crates/jumppad/src/docwatch.rs`

Deliberately much smaller than `reload.rs`, because it tracks no per-file
state — the sweep re-derives everything from `stat`.

```rust
/// Same window as `RELOAD_DEBOUNCE`: long enough to swallow another editor's
/// atomic-save dance (write, rename, write again), short enough to feel
/// immediate.
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(300);
pub const SETTLE_TICK: Duration = Duration::from_millis(100);

pub struct DocumentWatch { debounce: Debounce }

impl DocumentWatch {
    pub fn note_event(&mut self, now: Instant);   // pokes
    pub fn pending(&self) -> bool;                // gates the settle timer
    pub fn settled(&mut self, now: Instant) -> bool;
}

/// `paths` is every open file-backed tab's path, sorted and deduped.
pub fn subscription(paths: Vec<PathBuf>) -> Subscription<Message>;
```

**The watcher subscription is the one genuinely new mechanic.** `reload.rs`
uses `Subscription::run(watch_events)` with a fixed directory set;
documents come and go, so use `Subscription::run_with`:

```rust
// iced 0.14: pub fn run_with<D, S>(data: D, builder: fn(&D) -> S) -> Self
//            where D: Hash + 'static
Subscription::run_with(paths, watch_events).map(|_| Message::DocumentFileEvent)
```

Three things this implies, all worth a comment in the code:

- `builder` is a bare `fn(&D) -> S`, **not** a closure — it can capture
  nothing. Everything the watcher needs arrives through `data`.
- The recipe hashes `data`, so changing the path set tears the stream down
  and starts a fresh watcher over the new directories. That is the intended
  mechanism, and it is why the list must be **sorted and deduped**: an
  unstable order restarts the watcher on every unrelated tab switch.
- Derive the watched *directories* from the paths inside the builder, and
  watch those non-recursively — never the files themselves (see the comment
  at `reload.rs:265`).

Filter events in the callback to paths whose `file_name` matches one of the
open files, purely to keep a noisy directory (a Downloads folder, a build
output dir) from poking the debounce constantly. A false positive that slips
through is harmless — it costs one no-op sweep.

Wire `mod docwatch;` into `lib.rs` next to `mod reload;`.

### 5.2 New messages — `app.rs`

```rust
/// The OS file watcher saw activity in a directory holding an open file.
DocumentFileEvent,
/// Periodic while a document-change burst is pending.
DocumentSettleTick,
/// A tab's file finished re-reading after an external change. Carries the
/// stamp the read was taken against.
DocumentReloaded(u64, Option<DiskStamp>, Result<Arc<String>, std::io::ErrorKind>),
```

### 5.3 App wiring

- New field `document_watch: docwatch::DocumentWatch`.
- `subscription()` (app.rs:1621): add `docwatch::subscription(self.watched_paths())`
  to the base list, and — gated exactly like the config settle timer at
  app.rs:1726 — `if self.document_watch.pending() { iced::time::every(docwatch::SETTLE_TICK) }`.
  The gating is not optional: idle CPU is a stated design principle.
- `Message::DocumentFileEvent` → `self.document_watch.note_event(Instant::now())`.
- `Message::DocumentSettleTick` → `if self.document_watch.settled(now) { self.sweep_documents() }`.
- `Message::WindowFocused` (app.rs:1167) → keep the existing
  `config_watch.check`, and add `self.sweep_documents()`. A focus gain is
  already a settled moment; it needs no debounce.

### 5.4 The decision table — `JumpPadApp::sweep_documents`

Returns `Task<Message>`; batches one read task per tab that needs reloading.
For every tab with `document.path == Some(path)`:

```
current = DiskStamp::of(path)

current == tab.disk            -> nothing moved. Covers JumpPad's own save,
                                  and a touch that changed nothing. Skip.

current.is_none()              -> deleted or renamed away. Keep the tab and
                                  its content: tab.disk = None,
                                  tab.dirty = true, tab.draft_generation += 1,
                                  sync_session_metadata(). The next save
                                  recreates the file. No prompt.

!tab.dirty                     -> silent reload. Task::perform(read_path(..))
                                  -> Message::DocumentReloaded(id, current, ..).
                                  Do NOT stamp yet - stamp on arrival.

tab.dirty                      -> tab.externally_changed = true.
                                  Leave tab.disk at the OLD value: it is the
                                  expectation the save-time check compares
                                  against. Buffer untouched.
```

Iterate over *every* tab whose path matches, not the first — `tab_index_for`
prevents opening the same path twice, but Save As can still produce two tabs
pointing at one file.

### 5.5 `Message::DocumentReloaded` arm

Re-validate before applying — the read was async and the user may have typed
while it was in flight:

- Tab gone, or its `document.path` changed → drop the result.
- **Tab went dirty during the read** → drop the reload, set
  `externally_changed = true` instead. Applying it here would destroy the
  edits the user just made, which is the one thing this feature must never
  do.
- Otherwise → `tab.editor.reload_text(&contents)`, `tab.disk = stamp`,
  `tab.dirty` stays `false`, then `self.refresh_find()` (app.rs:616) because
  the document moved under the match list.
- `Err(kind)` → the file vanished between sweep and read; fall through to
  the delete branch's treatment rather than surfacing an error.

Do not touch the active tab, focus, or the scroll position. A background
tab reloading must be invisible.

**Checkpoint:** clean-tab reload and delete handling work end to end. Try it:
open a file, `echo hi >> file` from a terminal, watch it update; then
Ctrl+Z and confirm the old text comes back with the tab marked dirty.

---

## 6. Phase 4 — the dirty-tab conflict

### 6.1 Generalize the modal (do this first)

There is one modal today, `pending_close: Option<PendingClose>`, and five
places guard on it: `KeyPressed` (app.rs:1036), `FileDropped` (app.rs:953),
`Editor` (app.rs:1186), `CloseFind` (app.rs:1149), and `view` (app.rs:1563).
Adding a second `Option<..>` field means adding a second disjunct to each,
and a way to get them out of sync.

Replace with:

```rust
enum Modal {
    Close(PendingClose),
    SaveConflict(PendingConflict),
}
// field: modal: Option<Modal>,
```

Both are three-button dialogs with a `focused: usize`, so the
arrow/Tab/Enter/Escape handling at app.rs:1036-1071 generalizes to "cycle
`focused` mod 3, resolve to whichever variant is up." Every existing
`self.pending_close.is_some()` becomes `self.modal.is_some()`. The existing
tests at app.rs:3220-3348 pin this behavior — they should keep passing with
only the field name changed.

`PendingConflict { tab_id: u64, title: String, focused: usize }`.

### 6.2 Detect the conflict at save time

- `save_tab` (app.rs:891) passes `expected: Option<DiskStamp>` into
  `save_to`. Pass `tab.disk` **only** when `!force_dialog` and the tab
  already has a path — i.e. an in-place overwrite of a known file. A Save As
  target is the user picking a file in a dialog that already asks its own
  overwrite question, and an untitled tab has nothing to compare against.
- `save_to` (app.rs:2241), before writing: if `expected.is_some()` and
  `DiskStamp::of(&path) != expected` → `Err(SaveError::Conflict { path })`.
  There is an unavoidable stat-then-write TOCTOU window here; VS Code has
  the same one. Note it in a comment, don't try to close it.
- New `SaveError::Conflict { path: PathBuf }` variant.

### 6.3 The conflict dialog

`Message::FileSaved(id, Err(SaveError::Conflict { .. }))`:

- `self.file_dialog_active = false` (mirror the other error arms).
- Remove `id` from `pending_close_after_save` — a conflict aborts the close,
  same as VS Code leaving the editor open when the save fails.
- Open `Modal::SaveConflict`, unless a `Modal::Close` is already up, in
  which case queue it behind that one (reuse the shape of `close_queue`).

Text: *"`<name>` has changed on disk since you opened it."* Buttons:

| Button | Action |
| --- | --- |
| **Overwrite** | Re-run the save with `expected: None`, skipping the check. On success the normal `FileSaved(Ok)` path re-stamps and clears `externally_changed`. |
| **Discard & Reload** | Read the file, `reload_text`, `dirty = false`, `externally_changed = false`, restamp, `refresh_find()`, `sync_session_metadata()` (prunes the now-stale draft). |
| **Cancel** | Dismiss. Tab stays dirty and flagged; nothing on disk changed. |

### 6.4 Surfacing the flag before the user tries to save

VS Code shows a banner. JumpPad's `self.error` is a single global
dismissible row and the wrong home for per-tab state.

- **Tab title** — extend `Tab::title` (tab.rs:101). It currently appends
  `•` when dirty; append a second marker when `externally_changed`.
- **A bar above the editor**, shown only when the *active* tab is flagged:
  *"This file has changed on disk."* with **[Reload]** and **[Keep mine]**.
  Build it in `view` the same way the find palette and drop overlay are
  composed (app.rs:1520-1548) — a `stack!` over the editor element, so the
  tab bar stays visible and clickable.
  - **Reload** → the Discard & Reload action above.
  - **Keep mine** → clear `externally_changed` *and* `restamp()`. Re-stamping
    is the point: it records "I have seen this version," so the next save
    goes through without a second prompt.

---

## 7. Phase 5 (optional) — one config knob

Mirrors VS Code's `files.saveConflictResolution`:

```toml
[files]
save_conflict_resolution = "ask"        # or "overwrite"
```

`"overwrite"` skips §6.2's check entirely — saves always win. Add
`FilesConfig` to `jumppad_config/src/lib.rs` alongside `ScrollConfig`
(`#[serde(default)]`, so existing config files stay valid), and document it
in `config/config.sample.toml`.

It needs **no `apply_config` arm**: it is read from `self.config` at save
time, so a reload of `config.toml` picks it up for free. Say so in a comment
— AGENTS.md's rule is that live settings go in `apply_config` and nowhere
else, and a reader will look for the arm.

Do **not** add a "disable the watcher" knob unless someone actually asks for
one; the focus sweep already covers the cases a watcher misses.

---

## 8. Tests

Repo conventions: unit tests inline in the module, real `Tab`s built with
stub editors, `Instant` injected rather than slept on, temp dirs via the
`scratch_dir()` pattern in `session.rs:217`.

**`docwatch.rs`** — mirror `reload.rs`'s burst tests:
- `a_settled_burst_reports_once`
- `an_unsettled_burst_reports_nothing`

**`disk.rs`** — against a temp file:
- `a_stamp_changes_when_the_file_is_rewritten`
- `a_missing_path_has_no_stamp`

**`app.rs`** — the decision table, driving `sweep_documents` against real
temp files with stub editors:
- `a_clean_tab_reloads_silently_when_its_file_changes`
- `a_dirty_tab_keeps_its_buffer_and_is_flagged_instead`
- `a_deleted_file_leaves_the_tab_open_and_dirty`
- `our_own_save_does_not_read_as_an_external_change` (the stamp-equality
  guard — regression test for a reload loop)
- `a_reload_arriving_after_the_user_typed_is_dropped_and_flags_instead`
- `saving_over_a_changed_file_reports_a_conflict`
- `overwrite_writes_and_clears_the_flag`
- `discard_and_reload_replaces_the_buffer_and_goes_clean`
- `keep_mine_restamps_so_the_next_save_does_not_prompt`
- `a_conflict_during_a_close_prompt_save_aborts_the_close`
- `two_tabs_on_one_path_both_react`

**`jumppad_textarea/src/lib.rs`** — as listed in §4.4.

---

## 9. Gotchas

- **Don't let a save trigger a reload of itself.** The whole defense is
  stamping inside the save task and comparing stamps in the sweep. Get §3.5
  wrong and you get a reload loop that eats the user's cursor position on
  every save.
- **mtime granularity.** A same-length rewrite inside one filesystem tick
  can compare equal and be missed. `reload.rs:63` already accepts this;
  accept it here too rather than adding a content hash on a per-stat path.
- **`config.toml` open as a tab** is watched by both `ConfigWatch` and
  `DocumentWatch`. That is correct and independent: one reloads settings,
  the other reloads the buffer. Don't try to merge them.
- **The close modal's scrim doesn't stop window events** (AGENTS.md, drag
  and drop). It doesn't stop a sweep either — but a sweep is safe while a
  modal is up (a clean reload is invisible, a flag is just a flag). Only the
  *conflict modal* needs queueing behind the close modal.
- **Relative paths.** `jumppad newnote.md` produces a tab with a relative
  path. The sweep never compares paths — it stats whatever the tab holds —
  so this works, but keep it that way: any path-matching added later has to
  reckon with relative-vs-absolute and with deleted files, which can't be
  canonicalized.
- **`refresh_find()` after every buffer replacement.** The find match list
  holds byte ranges into the old text.
- **A reload is not an edit.** Don't route it through `Message::Editor` or
  set `dirty = true` on the clean path; that would arm draft autosave and
  write a pointless draft file for content that is already on disk.

---

## 10. Docs to update on the way out

- **`AGENTS.md`** — a new `## External file changes` section after
  `## Opening files`, holding: the decision table, why the sweep re-stats
  instead of matching event paths, the `run_with` restart mechanic, and the
  stamp-on-save self-trigger defense.
- **`README.md`** — one feature bullet: *"Notices files changed by other
  programs — reloads clean tabs silently, never overwrites unsaved edits
  without asking."*
- **`config/config.sample.toml`** — only if Phase 5 lands.

## 11. Suggested commit sequence

1. `DiskStamp`, tab disk state, stamp at load/save (§3)
2. `reload_text` on the widget trait (§4)
3. Watcher, sweep, silent reload, delete handling (§5)
4. Modal generalization (§6.1)
5. Save-conflict detection, dialog, banner (§6.2-6.4)
6. Config knob (§7, optional)
7. Docs (§10)

`cargo test --workspace && cargo clippy --workspace` at every step. Phases 1
and 2 are observably no-ops and should land green on their own before any
behavior changes.
