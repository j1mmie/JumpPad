use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use editor_core::{EditorFactory, EditorMessage, Tab};
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use iced::advanced::widget::{operate, operation};
use iced::keyboard::key;
use iced::widget::{button, column, container, keyed_column, mouse_area, row, scrollable, text};
use iced::{Center, Color, Element, Fill, Point, Subscription, Task, Theme, keyboard};

use crate::hotkey::{self, Hotkey};
use crate::session;
use crate::visor::{self, Animation};

/// How often the visor's slide animation advances a frame while in
/// progress - see `subscription()`'s gating and `Message::AnimationTick`.
const ANIMATION_TICK: Duration = Duration::from_millis(16);

/// How often the autosave timer ticks while at least one tab has unflushed
/// dirty content - see `subscription()`'s gating (mirrors the existing
/// `PollHighlighting` pattern) and `Message::AutosaveTick`.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(5);

pub struct XizorApp {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    error: Option<String>,
    editor_factory: EditorFactory,
    theme: Theme,
    /// Flips every time the active tab changes. See `theme()` for what
    /// this actually does - it's not a display preference.
    redraw_nudge: bool,
    /// Where the session manifest and draft files live - resolved once at
    /// startup (see `session::candidate_dirs()`) and reused for every
    /// later write, never re-derived mid-session.
    session_dir: PathBuf,
    /// Tab ids waiting on a save (triggered by choosing "Save" in the
    /// unsaved-changes prompt - see `request_close`) before they can be
    /// closed. Consumed in `Message::FileSaved`'s success branch; cleared
    /// without closing on a failed/cancelled save so a later, unrelated
    /// save of the same tab doesn't unexpectedly close it.
    pending_close_after_save: Vec<u64>,
    /// The app's own (only) window - `None` until the startup
    /// `iced::window::latest()` task resolves in `Message::WindowReady`.
    /// Needed to target every `move_to`/`resize` call the visor makes.
    window: Option<iced::window::Id>,
    /// The registered global toggle hotkey, or `None` if registration
    /// failed (e.g. the combo is already claimed by another application) -
    /// see `hotkey::Hotkey::register`. The visor keybind just silently does
    /// nothing in that case rather than the app failing to start.
    hotkey: Option<Hotkey>,
    /// Whether the visor is (or is animating toward being) shown.
    visor_visible: bool,
    /// `Some` while a show/hide slide is in progress - see
    /// `Message::AnimationTick` and `subscription()`'s gating of the
    /// animation timer on this.
    animation: Option<Animation>,
    /// Mirrors `xizor_config::VisorConfig::enabled`. When `false`, the app
    /// behaves as an ordinary window: `snap_to_monitor`/`toggle_visor` never
    /// reposition or resize it (decorations and window level are set once,
    /// up front, in `lib.rs`), and no global hotkey is registered at all -
    /// see `new()` - so there's nothing for `HotkeyEvent` to match against
    /// and the toggle keypress is silently ignored.
    visor_enabled: bool,
    /// The id of whichever tab was active immediately before the current
    /// one, for `Message::SelectPreviousActiveTab` (Ctrl+Tab) - updated in
    /// `switch_active`. By id rather than index since the tab list can
    /// change shape (a close shifts every later index) without this needing
    /// to track that; a since-closed tab's id just fails the lookup and the
    /// shortcut is a no-op that turn, see `update()`.
    previous_active: Option<u64>,
    /// This app's keybind overrides, resolved from `keybinds.toml` once at
    /// startup (see `new()`/`build_app_overrides`) - checked by
    /// `handle_hotkey` ahead of its own hardcoded shortcuts. `Arc` so
    /// `subscription()` can cheaply clone it into the `keyboard::listen()`
    /// closure on every rebuild without cloning the map itself.
    keybind_overrides: Arc<HashMap<(keyboard::Modifiers, key::Code), Message>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    NewTab,
    OpenFile,
    FileOpened(Result<(PathBuf, Arc<String>), OpenError>),
    SaveFile,
    SaveFileAs,
    FileSaved(u64, Result<PathBuf, SaveError>),
    SelectTab(usize),
    CloseTab(usize),
    CloseActiveTab,
    DismissError,
    Editor(usize, EditorMessage),
    PollHighlighting,
    /// Fires on a timer while some tab has draft content newer than what's
    /// on disk (see `subscription()`); writes each stale tab's draft file.
    AutosaveTick,
    /// A background draft write for tab `.0` finished, bringing its draft
    /// file up to date with generation `.1`.
    DraftFlushed(u64, u64),
    /// The user asked the window to close (e.g. clicked its close button).
    /// Triggers a last-ditch synchronous draft flush before the window is
    /// actually allowed to close - see `iced::window::close_requests()` in
    /// `subscription()`.
    WindowCloseRequested(iced::window::Id),
    /// A restored tab's real file finished (re-)reading from disk at
    /// startup - see `new()`'s restore path and `reload_from_disk`.
    SessionFileLoaded(u64, Result<Arc<String>, std::io::ErrorKind>),
    /// The unsaved-changes prompt shown by `request_close` for a dirty tab
    /// (identified by id, not index - the tab list can change shape while
    /// the dialog is up) came back with the user's choice.
    CloseConfirmed(u64, CloseDecision),
    /// The startup `iced::window::latest()` task (see `new()`) resolved
    /// with the app's own window id - `None` should never actually happen
    /// for a single-window app, but the command's signature allows it.
    WindowReady(Option<iced::window::Id>),
    /// A global hotkey fired somewhere - see `hotkey::subscription()`.
    /// Filtered down to "was it a press of *our* toggle hotkey" before
    /// being acted on (see `toggle_visor`), since this fires for the raw OS
    /// event regardless of which hotkey or press/release state it was.
    HotkeyEvent(GlobalHotKeyEvent),
    /// Fires on a timer while `animation` is `Some` (see `subscription()`);
    /// advances the in-progress slide by one frame.
    AnimationTick,
    /// Cmd+Shift+] (mac) / Ctrl+Shift+] (elsewhere) - switch to the next tab,
    /// wrapping past the last back to the first.
    SelectNextTab,
    /// Cmd+Shift+[ (mac) / Ctrl+Shift+[ (elsewhere) - mirror of
    /// `SelectNextTab`.
    SelectPreviousTab,
    /// Ctrl+Tab, identical on every OS - swap back to whichever tab was
    /// active immediately before this one (see `previous_active`).
    SelectPreviousActiveTab,
    /// A raw key press straight from `keyboard::listen()` - resolved into a
    /// real command (if any) by `handle_hotkey` in `update()`, using
    /// `self.keybind_overrides`. Kept as its own variant, rather than
    /// resolving inside `subscription()`'s `filter_map` closure directly,
    /// because iced requires `Subscription::filter_map`'s closure to be
    /// non-capturing (zero-sized) - it can't reach `self.keybind_overrides`
    /// from there.
    KeyPressed(keyboard::Key, keyboard::Modifiers, key::Physical),
}

/// The three choices offered by the unsaved-changes prompt (see
/// `request_close` and `confirm_close`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    Save,
    DontSave,
    Cancel,
}

#[derive(Debug, Clone)]
pub enum OpenError {
    DialogClosed,
    Io {
        path: PathBuf,
        kind: std::io::ErrorKind,
    },
}

#[derive(Debug, Clone)]
pub enum SaveError {
    DialogClosed,
    Io {
        path: PathBuf,
        kind: std::io::ErrorKind,
    },
}

impl XizorApp {
    /// Takes the already-loaded config rather than loading it itself: `run()`
    /// (in `lib.rs`) loads it before the iced runtime starts, which is
    /// before this constructor ever runs.
    pub fn new(config: xizor_config::Config) -> (Self, Task<Message>) {
        let search_dirs = default_search_dirs();
        log_wasm_files_found(&search_dirs);
        // No push-based wake-up needed here (unlike egui's `ctx.request_repaint()`):
        // the highlighting-poll subscription in `subscription()` below re-checks
        // every pending tab on a timer instead, so there's nothing for this
        // callback to do.
        // Loaded unconditionally now (previously only loaded when visor mode
        // was on, since `toggle` was the only field anyone read) - the new
        // `overrides` map needs to apply regardless of visor mode. Only
        // `toggle`'s *use* below stays gated on `visor_enabled`.
        let keybinds = xizor_config::load_keybinds();
        let keybind_overrides = Arc::new(build_app_overrides(&keybinds));
        let editor_overrides = Arc::new(build_editor_overrides(&keybinds));
        warn_unrecognized_overrides(&keybinds.overrides);

        let registry = syntax_registry::SyntaxRegistry::new(
            search_dirs,
            config.syntaxes.extension_to_grammar(),
            || {},
        );
        // Which `TextEditorWidget` implementation new tabs are created with.
        // Swapping editor backends later means changing this one line (and
        // the `iced_text_editor` dependency) - nothing else in this file
        // needs to know.
        let editor_factory: EditorFactory = Box::new(iced_text_editor::IcedTextEditor::factory(
            registry,
            editor_overrides,
        ));

        let session_candidates = session::candidate_dirs();
        let session_dir = session_candidates
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("drafts"));
        let manifest = session::load_manifest(&session_candidates);

        let visor_enabled = config.visor.enabled;
        // Global hotkey registration is skipped entirely when visor mode is
        // off, rather than registered-then-ignored: that's what makes the
        // toggle keypress a no-op (ignored, not just invisible) system-wide,
        // and avoids claiming the combo with the OS for a feature that isn't
        // in use.
        let hotkey = if visor_enabled {
            Hotkey::register(keybinds.toggle)
        } else {
            None
        };

        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            error: None,
            editor_factory,
            theme: resolve_theme(&config.theme),
            redraw_nudge: false,
            session_dir,
            pending_close_after_save: Vec::new(),
            window: None,
            hotkey,
            visor_visible: false,
            animation: None,
            visor_enabled,
            previous_active: None,
            keybind_overrides,
        };

        let window_task = iced::window::latest().map(Message::WindowReady);

        let Some(manifest) = manifest.filter(|manifest| !manifest.tabs.is_empty()) else {
            let task = app.new_tab();
            return (app, Task::batch([task, window_task]));
        };

        // Restore each tab: dirty ones (with or without a real path) read
        // their draft file; clean, file-backed ones get a fresh async
        // re-read of the real file - never a stale cached copy, since the
        // file may have changed on disk since the last session.
        let mut reload_tasks = Vec::new();
        for entry in &manifest.tabs {
            if entry.dirty {
                let draft = session::draft_path(&app.session_dir, entry.id);
                match std::fs::read_to_string(&draft) {
                    Ok(content) => {
                        app.tabs.push(Tab::restored(
                            entry.id,
                            entry.path.clone(),
                            &content,
                            true,
                            &app.editor_factory,
                        ));
                    }
                    Err(_) if entry.path.is_some() => {
                        // Draft file missing/unreadable (e.g. the drafts
                        // dir was hand-edited): best effort is to fall back
                        // to a clean re-read of the real file instead of
                        // dropping the tab entirely.
                        let id = entry.id;
                        let path = entry
                            .path
                            .clone()
                            .expect("checked above: entry.path is Some");
                        app.tabs.push(Tab::restored(
                            id,
                            Some(path.clone()),
                            "",
                            false,
                            &app.editor_factory,
                        ));
                        reload_tasks.push(Task::perform(reload_from_disk(path), move |result| {
                            Message::SessionFileLoaded(id, result)
                        }));
                    }
                    Err(_) => {
                        // Untitled, no draft, nothing to recover - drop
                        // this entry rather than restoring an empty tab.
                    }
                }
            } else if let Some(path) = &entry.path {
                app.tabs.push(Tab::restored(
                    entry.id,
                    Some(path.clone()),
                    "",
                    false,
                    &app.editor_factory,
                ));
                let id = entry.id;
                reload_tasks.push(Task::perform(reload_from_disk(path.clone()), move |result| {
                    Message::SessionFileLoaded(id, result)
                }));
            } else {
                app.tabs.push(Tab::untitled(entry.id, &app.editor_factory));
            }
        }

        if app.tabs.is_empty() {
            let task = app.new_tab();
            return (app, Task::batch([task, window_task]));
        }

        app.next_id = manifest
            .tabs
            .iter()
            .map(|entry| entry.id)
            .max()
            .map(|max_id| max_id + 1)
            .unwrap_or(0);

        let desired_active = manifest.active.min(app.tabs.len() - 1);
        let switch_task = app.switch_active(desired_active);
        let task = Task::batch(
            reload_tasks
                .into_iter()
                .chain([switch_task, window_task]),
        );
        (app, task)
    }

    fn new_tab(&mut self) -> Task<Message> {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab::untitled(id, &self.editor_factory));
        self.switch_active(self.tabs.len() - 1)
    }

    /// Entry point for both the tab-bar "x" button and middle-click
    /// (`Message::CloseTab`) as well as `Ctrl+W`. Closes clean tabs
    /// immediately; dirty tabs get an async unsaved-changes prompt first,
    /// with the actual close (or save-then-close) happening once the user
    /// answers it - see `Message::CloseConfirmed`.
    fn request_close(&mut self, index: usize) -> Task<Message> {
        let Some(tab) = self.tabs.get(index) else {
            return Task::none();
        };
        if !tab.dirty {
            return self.close_tab(index);
        }
        let id = tab.id;
        let title = tab.document.display_name();
        Task::perform(confirm_close(title), move |decision| {
            Message::CloseConfirmed(id, decision)
        })
    }

    fn close_tab(&mut self, index: usize) -> Task<Message> {
        if index >= self.tabs.len() {
            return Task::none();
        }
        self.tabs.remove(index);
        // Whichever branch runs, the tab list just changed structurally -
        // sync unconditionally rather than threading a "did switch_active
        // already sync for me" flag through all three branches. A couple
        // of branches end up syncing twice back-to-back; that's cheap.
        let task = if self.tabs.is_empty() {
            self.new_tab()
        } else if self.active >= self.tabs.len() {
            self.switch_active(self.tabs.len() - 1)
        } else {
            Task::none()
        };
        self.sync_session_metadata();
        task
    }

    /// Switches the active tab, carrying each tab's cursor position across
    /// the switch: the previously active tab's cursor is saved, and the
    /// newly active tab's remembered cursor (if any - a never-visited tab
    /// just has the document-start default) is restored. Also re-focuses
    /// the editor, since a freshly switched-to tab's widget state starts
    /// unfocused (see the `keyed_column` comment in `view`) - without it,
    /// the caret would just be invisible until the user clicked.
    ///
    /// Focusing alone was tried as a fix for the tiny-skia stale-content
    /// bug (AGENTS.md) on the theory that it reproduces what a real click
    /// does - confirmed insufficient in practice, content still went
    /// stale. `redraw_nudge` (flipped here, consumed by `theme()`) is the
    /// actual fix for that; it's independent of the cursor/focus handling
    /// above and would still be needed even if this method didn't exist.
    fn switch_active(&mut self, index: usize) -> Task<Message> {
        if index >= self.tabs.len() || index == self.active {
            return Task::none();
        }
        if let Some(previous) = self.tabs.get_mut(self.active) {
            previous.last_cursor = previous.editor.cursor_position();
            self.previous_active = Some(previous.id);
        }
        self.active = index;
        self.redraw_nudge = !self.redraw_nudge;
        if let Some(tab) = self.tabs.get_mut(index) {
            let (line, column) = tab.last_cursor;
            tab.editor.move_cursor_to(line, column);
        }
        self.sync_session_metadata();
        operate(operation::focusable::focus_next())
    }

    /// Moves the active tab by `delta` positions, wrapping around at either
    /// end - `+1`/`-1` for `Message::SelectNextTab`/`SelectPreviousTab`.
    fn cycle_tab(&mut self, delta: isize) -> Task<Message> {
        if self.tabs.is_empty() {
            return Task::none();
        }
        let len = self.tabs.len() as isize;
        let next = (self.active as isize + delta).rem_euclid(len) as usize;
        self.switch_active(next)
    }

    /// Rewrites the session manifest (tab existence/path/dirty/active
    /// index) from current state, pruning any now-orphaned draft files.
    /// Deliberately synchronous (`std::fs`, not `tokio::fs`) - it's a small
    /// TOML file plus a `read_dir` scan, cheap enough to call straight from
    /// `update()` on every structural change. Draft *content* (potentially
    /// large document text) goes through the async path instead - see
    /// `Message::AutosaveTick`.
    fn sync_session_metadata(&self) {
        let manifest = session::build_manifest(&self.tabs, self.active);
        session::write_manifest_sync(&self.session_dir, &manifest);
    }

    fn save_active_tab(&self, force_dialog: bool) -> Task<Message> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Task::none();
        };
        self.save_tab(tab.id, force_dialog)
    }

    /// Saves the tab with the given id, whether or not it's the active tab -
    /// needed so `request_close`/`Message::CloseConfirmed` can save a
    /// background tab the user is closing without first switching to it.
    fn save_tab(&self, id: u64, force_dialog: bool) -> Task<Message> {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return Task::none();
        };
        let existing_path = tab.document.path.clone();
        let text = tab.editor.text();
        Task::perform(save_to(existing_path, text, force_dialog), move |result| {
            Message::FileSaved(id, result)
        })
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NewTab => self.new_tab(),
            Message::OpenFile => Task::perform(open_and_read(), Message::FileOpened),
            Message::FileOpened(Ok((path, contents))) => {
                let id = self.next_id;
                self.next_id += 1;
                self.tabs
                    .push(Tab::from_file(id, path, &contents, &self.editor_factory));
                self.switch_active(self.tabs.len() - 1)
            }
            Message::FileOpened(Err(OpenError::DialogClosed)) => Task::none(),
            Message::FileOpened(Err(OpenError::Io { path, kind })) => {
                self.error = Some(format!("Couldn't open {}: {kind}", path.display()));
                Task::none()
            }
            Message::SaveFile => self.save_active_tab(false),
            Message::SaveFileAs => self.save_active_tab(true),
            Message::FileSaved(id, Ok(path)) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.document.path = Some(path);
                    tab.dirty = false;
                }
                // The tab just went clean - rewrite the manifest so its
                // now-stale `.draft` file gets pruned.
                self.sync_session_metadata();
                // If this save was the "Save" branch of an unsaved-changes
                // prompt, the tab is now clean and can actually close.
                if let Some(pos) = self
                    .pending_close_after_save
                    .iter()
                    .position(|&pending_id| pending_id == id)
                {
                    self.pending_close_after_save.remove(pos);
                    if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                        return self.close_tab(index);
                    }
                }
                Task::none()
            }
            Message::FileSaved(id, Err(SaveError::DialogClosed)) => {
                // The user backed out of the save (e.g. the save-as dialog
                // for an untitled tab) - leave the tab open rather than
                // closing it unsaved.
                self.pending_close_after_save.retain(|&pending_id| pending_id != id);
                Task::none()
            }
            Message::FileSaved(id, Err(SaveError::Io { path, kind })) => {
                self.pending_close_after_save.retain(|&pending_id| pending_id != id);
                if let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) {
                    self.error = Some(format!(
                        "Couldn't save {}: {kind}",
                        tab.document.display_name()
                    ));
                } else {
                    self.error = Some(format!("Couldn't save {}: {kind}", path.display()));
                }
                Task::none()
            }
            Message::SelectTab(index) => self.switch_active(index),
            Message::CloseTab(index) => self.request_close(index),
            Message::CloseActiveTab => self.request_close(self.active),
            Message::SelectNextTab => self.cycle_tab(1),
            Message::SelectPreviousTab => self.cycle_tab(-1),
            Message::SelectPreviousActiveTab => match self.previous_active {
                Some(id) => match self.tabs.iter().position(|tab| tab.id == id) {
                    Some(index) => self.switch_active(index),
                    // That tab was closed since - nothing sensible to swap
                    // to, so this is a silent no-op rather than falling
                    // back to some other tab the user didn't ask for.
                    None => Task::none(),
                },
                None => Task::none(),
            },
            Message::KeyPressed(key, modifiers, physical_key) => {
                match handle_hotkey(key, modifiers, physical_key, &self.keybind_overrides) {
                    Some(resolved) => self.update(resolved),
                    None => Task::none(),
                }
            }
            Message::DismissError => {
                self.error = None;
                Task::none()
            }
            Message::Editor(index, editor_message) => {
                if let Some(tab) = self.tabs.get_mut(index) {
                    if tab.editor.update(editor_message) {
                        let just_became_dirty = !tab.dirty;
                        tab.dirty = true;
                        tab.draft_generation += 1;
                        // Only sync the manifest on the clean->dirty
                        // transition, not every keystroke - the periodic
                        // autosave timer (gated on this same dirtiness)
                        // handles the actual draft *content* writes.
                        if just_became_dirty {
                            self.sync_session_metadata();
                        }
                    }
                }
                Task::none()
            }
            Message::PollHighlighting => {
                for tab in &mut self.tabs {
                    tab.editor.poll_highlighting();
                }
                Task::none()
            }
            Message::AutosaveTick => {
                let dir = self.session_dir.clone();
                let writes = session::stale_tabs(&self.tabs)
                    .into_iter()
                    .map(|(id, generation, text)| {
                        Task::perform(
                            session::flush_draft_async(dir.clone(), id, generation, text),
                            |(id, generation)| Message::DraftFlushed(id, generation),
                        )
                    });
                Task::batch(writes)
            }
            Message::DraftFlushed(id, generation) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    if generation > tab.flushed_generation {
                        tab.flushed_generation = generation;
                    }
                }
                Task::none()
            }
            Message::WindowCloseRequested(id) => {
                session::flush_on_exit(&self.session_dir, &self.tabs, self.active);
                iced::window::close(id)
            }
            Message::SessionFileLoaded(id, Ok(contents)) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    // Only replaces the visible content - does not touch
                    // `dirty`, which stays `false` for these (clean,
                    // freshly-reloaded) tabs.
                    tab.editor.set_text(&contents);
                }
                Task::none()
            }
            Message::SessionFileLoaded(_, Err(kind)) => {
                self.error = Some(format!("Couldn't reload a restored tab: {kind}"));
                Task::none()
            }
            Message::CloseConfirmed(id, decision) => match decision {
                CloseDecision::Cancel => Task::none(),
                CloseDecision::DontSave => {
                    match self.tabs.iter().position(|tab| tab.id == id) {
                        Some(index) => self.close_tab(index),
                        None => Task::none(),
                    }
                }
                CloseDecision::Save => {
                    self.pending_close_after_save.push(id);
                    self.save_tab(id, false)
                }
            },
            Message::WindowReady(id) => {
                self.window = id;
                self.snap_to_monitor()
            }
            Message::HotkeyEvent(event) => {
                let is_our_toggle = self.hotkey.as_ref().is_some_and(|hotkey| {
                    event.state() == HotKeyState::Pressed && event.id() == hotkey.id()
                });
                if is_our_toggle {
                    self.toggle_visor()
                } else {
                    Task::none()
                }
            }
            Message::AnimationTick => self.advance_animation(),
        }
    }

    /// Snaps the window to the primary monitor's current bounds - full
    /// width, one third the height - and parks it off-screen above the top,
    /// ready for the next `ToggleVisor` to slide it into view. Called once
    /// at startup (`Message::WindowReady`); see `toggle_visor` for the
    /// per-toggle re-snap that also covers a monitor change mid-session.
    fn snap_to_monitor(&mut self) -> Task<Message> {
        if !self.visor_enabled {
            // An ordinary window: leave it wherever the OS placed it, at
            // the size `lib.rs`'s `window_size` requested, rather than
            // shrinking it to visor proportions and parking it off-screen.
            return Task::none();
        }
        let Some(id) = self.window else {
            return Task::none();
        };
        let Some(monitor) = visor::primary_monitor_bounds() else {
            eprintln!("xizor: couldn't determine the primary monitor's bounds");
            return Task::none();
        };
        Task::batch([
            iced::window::resize(id, visor::visor_size(monitor)),
            iced::window::move_to(id, visor::hidden_position(monitor)),
        ])
    }

    /// Starts (or reverses) the visor's show/hide slide. Snaps the window's
    /// width and x-position to the primary monitor's *current* bounds first
    /// - covers both "resolution changed since startup" and "the monitor
    /// layout is different than it was at the last toggle" - but only ever
    /// tweens `y` (see `visor::Animation`), never width/height/x.
    fn toggle_visor(&mut self) -> Task<Message> {
        // Unreachable in practice: with visor mode off, `new()` never
        // registers a hotkey, so `HotkeyEvent` (this method's only caller)
        // never matches `self.hotkey` and this never gets called. Guarded
        // anyway so the invariant doesn't depend on staying in sync with
        // that registration logic.
        if !self.visor_enabled {
            return Task::none();
        }
        let Some(id) = self.window else {
            return Task::none();
        };
        let Some(monitor) = visor::primary_monitor_bounds() else {
            eprintln!("xizor: couldn't determine the primary monitor's bounds");
            return Task::none();
        };

        // Reversing out of a not-yet-finished animation (rather than always
        // starting from the nominal settled position) is what keeps a rapid
        // double-press of the toggle keybind from glitching.
        let current_y = match &self.animation {
            Some(animation) => animation.current_y(),
            None if self.visor_visible => visor::shown_position(monitor).y,
            None => visor::hidden_position(monitor).y,
        };

        self.visor_visible = !self.visor_visible;
        let target = if self.visor_visible {
            visor::shown_position(monitor)
        } else {
            visor::hidden_position(monitor)
        };
        self.animation = Some(Animation::new(target.x, current_y, target.y));

        let mut tasks = vec![
            iced::window::resize(id, visor::visor_size(monitor)),
            iced::window::move_to(id, Point::new(target.x, current_y)),
        ];
        if self.visor_visible {
            // Lets the user start typing immediately after summoning the
            // visor, without an extra click. See the plan's "known
            // limitations" note: hiding doesn't explicitly hand focus back
            // to whatever was focused before.
            tasks.push(iced::window::gain_focus(id));
        }
        Task::batch(tasks)
    }

    fn advance_animation(&mut self) -> Task<Message> {
        let Some(id) = self.window else {
            self.animation = None;
            return Task::none();
        };
        let Some(animation) = &self.animation else {
            return Task::none();
        };
        let point = Point::new(animation.x, animation.current_y());
        let finished = animation.is_finished();
        if finished {
            self.animation = None;
        }
        iced::window::move_to(id, point)
    }

    pub fn view(&self) -> Element<'_, Message> {
        let tab_chips = self.tabs.iter().enumerate().map(|(index, tab)| {
            let is_active = index == self.active;

            let title = button(text(tab.title()))
                .padding([6, 10])
                .style(move |theme, status| tab_title_style(theme, status, is_active))
                .on_press(Message::SelectTab(index));

            let close = button(text("x").size(12))
                .padding([6, 8])
                .style(move |theme, status| tab_close_style(theme, status, is_active))
                .on_press(Message::CloseTab(index));

            // The frame is the *only* thing that paints this tab's
            // background - title and close are both styled fully
            // transparent at rest (see tab_title_style/tab_close_style).
            // Letting each button paint its own background instead (the
            // previous approach) meant two independently-sized rectangles
            // side by side, and any mismatch between them (e.g. the close
            // button's smaller font shrinking its own box) showed up as a
            // visible seam. With the frame owning the shape, future
            // additions to a tab (an icon, a dirty-dot) just join the row
            // and get centered - they can't reintroduce that seam.
            let frame = container(row![title, close].spacing(0).align_y(Center))
                .style(move |theme| tab_frame_style(theme, is_active));

            // Middle-click anywhere on the chip closes it, same as clicking
            // the "x" - both funnel through `Message::CloseTab` so a dirty
            // tab gets the same unsaved-changes prompt either way.
            mouse_area(frame)
                .on_middle_press(Message::CloseTab(index))
                .into()
        });

        let new_tab_button = button(text("+"))
            .padding([6, 10])
            .style(new_tab_style)
            .on_press(Message::NewTab);

        let tabs_row = row(tab_chips.chain(std::iter::once(new_tab_button.into())))
            .spacing(0)
            .align_y(Center);

        let tab_bar: Element<'_, Message> = container(
            scrollable(tabs_row).direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new(),
            )),
        )
        .width(Fill)
        .style(tab_bar_style)
        .into();

        let editor: Element<'_, Message> = if let Some(tab) = self.tabs.get(self.active) {
            let index = self.active;
            let tab_id = tab.id;
            let view = tab
                .editor
                .view()
                .map(move |message| Message::Editor(index, message));
            // Keyed by the tab's stable id (not its Vec index, which shifts
            // as tabs close) so switching tabs is a *key* change at this
            // tree position, not just a different `Content` behind the same
            // widget instance. Without this, iced's widget-tree diffing
            // reuses the previous tab's cached editor state in place and
            // the text_editor keeps showing whatever it last rendered
            // instead of the newly-selected tab's content.
            keyed_column([(tab_id, view)]).width(Fill).height(Fill).into()
        } else {
            text("No open tabs").into()
        };

        let mut content = column![tab_bar, editor];

        if let Some(error) = &self.error {
            content = content.push(
                row![
                    text(error.clone()).color(iced::Color::from_rgb8(220, 60, 60)),
                    button("Dismiss").on_press(Message::DismissError),
                ]
                .spacing(10)
                .padding(6),
            );
        }

        content.into()
    }

    pub fn theme(&self) -> Theme {
        if self.redraw_nudge {
            nudge_background(&self.theme)
        } else {
            self.theme.clone()
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            // iced 0.14 replaced the separate `on_key_press`/`on_key_release`
            // subscriptions with one unified `listen()` covering every
            // keyboard event - `filter_map` (also new in 0.14) picks out
            // just the presses, forwarded as `Message::KeyPressed` for
            // `update()` to actually resolve (see that variant's doc
            // comment for why the resolution can't happen right here).
            keyboard::listen().filter_map(|event| match event {
                keyboard::Event::KeyPressed {
                    key,
                    modifiers,
                    physical_key,
                    ..
                } => {
                    // TEMPORARY DEBUG - remove once the Cmd-modifier issue
                    // is diagnosed. Prints every key press this subscription
                    // sees (i.e. every press iced considers "ignored" /
                    // uncaptured by any focused widget), with the raw
                    // modifiers and physical key iced/winit reported.
                    eprintln!(
                        "xizor DEBUG: key={key:?} modifiers={modifiers:?} physical_key={physical_key:?}"
                    );
                    Some(Message::KeyPressed(key, modifiers, physical_key))
                }
                _ => None,
            }),
            // An event listener, not a timer - no idle cost, so this stays
            // unconditional (unlike the gated timers below), same as the
            // global-hotkey listener right after it.
            iced::window::close_requests().map(Message::WindowCloseRequested),
            hotkey::subscription(),
        ];

        // Only ticks while a show/hide slide is actually in progress - same
        // "no idle cost while settled" shape as the two timers below.
        if self.animation.is_some() {
            subscriptions
                .push(iced::time::every(ANIMATION_TICK).map(|_| Message::AnimationTick));
        }

        // Only ticks while some tab is actually waiting on a grammar load -
        // avoids a permanent background timer once every tab's highlighting
        // has settled.
        if self.tabs.iter().any(|tab| tab.editor.has_pending_highlighting()) {
            subscriptions.push(
                iced::time::every(Duration::from_millis(50)).map(|_| Message::PollHighlighting),
            );
        }

        // Only ticks while some tab has draft content newer than what's on
        // disk - same "costs nothing while idle" shape as the highlighting
        // poll above.
        if self
            .tabs
            .iter()
            .any(|tab| tab.dirty && tab.draft_generation != tab.flushed_generation)
        {
            subscriptions
                .push(iced::time::every(AUTOSAVE_INTERVAL).map(|_| Message::AutosaveTick));
        }

        Subscription::batch(subscriptions)
    }
}

/// App-level command names a `keybinds.toml` override may target - see
/// `xizor_config::KeybindsConfig::overrides`'s doc comment. The single
/// source of truth both `build_app_overrides` and `warn_unrecognized_overrides`
/// point at.
pub const APP_COMMAND_NAMES: &[&str] = &[
    "new_tab",
    "open_file",
    "save_file",
    "save_file_as",
    "close_active_tab",
    "select_previous_tab",
    "select_next_tab",
    "select_previous_active_tab",
];

/// Resolves `keybinds.toml`'s overrides into a lookup keyed by the same
/// `(Modifiers, key::Code)` pair `handle_hotkey` matches incoming presses
/// against - physical-key based (layout-independent), built once at
/// startup and reused for the app's lifetime (see `XizorApp::keybind_overrides`).
fn build_app_overrides(
    keybinds: &xizor_config::KeybindsConfig,
) -> HashMap<(keyboard::Modifiers, key::Code), Message> {
    let resolved = keybinds.resolved_overrides();
    let mut map = HashMap::new();
    for (name, message) in [
        ("new_tab", Message::NewTab),
        ("open_file", Message::OpenFile),
        ("save_file", Message::SaveFile),
        ("save_file_as", Message::SaveFileAs),
        ("close_active_tab", Message::CloseActiveTab),
        ("select_previous_tab", Message::SelectPreviousTab),
        ("select_next_tab", Message::SelectNextTab),
        ("select_previous_active_tab", Message::SelectPreviousActiveTab),
    ] {
        if let Some(resolved) = resolved.get(name) {
            map.insert((resolved.modifiers, resolved.code), message);
        }
    }
    map
}

/// Mirror of `build_app_overrides` for the editor-level commands
/// (`iced_text_editor::EDITOR_COMMAND_NAMES`) - built here, not inside
/// `iced_text_editor` itself, so that crate doesn't need to depend on
/// `xizor_config`/`global_hotkey` just to consume an already-resolved chord;
/// `app.rs` already depends on both and does the intersection for both
/// layers.
fn build_editor_overrides(
    keybinds: &xizor_config::KeybindsConfig,
) -> HashMap<(keyboard::Modifiers, key::Code), iced_text_editor::EditorCommand> {
    let resolved = keybinds.resolved_overrides();
    let mut map = HashMap::new();
    for (name, command) in iced_text_editor::EDITOR_COMMAND_NAMES {
        if let Some(resolved) = resolved.get(*name) {
            map.insert((resolved.modifiers, resolved.code), *command);
        }
    }
    map
}

/// Logs (doesn't fail) any `keybinds.toml` override whose command name
/// isn't recognized by either layer - a cheap typo-catcher, not a
/// validation framework.
fn warn_unrecognized_overrides(overrides: &HashMap<String, global_hotkey::hotkey::HotKey>) {
    for name in overrides.keys() {
        let known = APP_COMMAND_NAMES.contains(&name.as_str())
            || iced_text_editor::EDITOR_COMMAND_NAMES
                .iter()
                .any(|(known_name, _)| known_name == name);
        if !known {
            eprintln!(
                "xizor_config: keybinds.toml overrides an unrecognized command {name:?}, ignoring"
            );
        }
    }
}

fn handle_hotkey(
    key: keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical_key: key::Physical,
    overrides: &HashMap<(keyboard::Modifiers, key::Code), Message>,
) -> Option<Message> {
    // Tier 1: user override, matched by physical key (layout-independent) -
    // see `iced_text_editor`'s equivalent for why this deliberately differs
    // from tier 2's logical-key matching below.
    if let key::Physical::Code(code) = physical_key {
        if let Some(message) = overrides.get(&(modifiers, code)) {
            return Some(message.clone());
        }
    }

    // Tier 2: this repo's existing hardcoded shortcuts, unchanged.
    //
    // Ctrl+Tab: identical on every OS, so this is checked ahead of (and
    // deliberately doesn't use) the `command()` gate below - `command()`
    // resolves to Cmd on macOS, which isn't what this shortcut means.
    if modifiers.control()
        && !modifiers.shift()
        && !modifiers.alt()
        && !modifiers.logo()
        && matches!(key, keyboard::Key::Named(keyboard::key::Named::Tab))
    {
        return Some(Message::SelectPreviousActiveTab);
    }

    if !modifiers.command() {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character("n") => Some(Message::NewTab),
        keyboard::Key::Character("o") => Some(Message::OpenFile),
        keyboard::Key::Character("s") if modifiers.shift() => Some(Message::SaveFileAs),
        keyboard::Key::Character("s") => Some(Message::SaveFile),
        keyboard::Key::Character("w") => Some(Message::CloseActiveTab),
        keyboard::Key::Character("[") if modifiers.shift() => Some(Message::SelectPreviousTab),
        keyboard::Key::Character("]") if modifiers.shift() => Some(Message::SelectNextTab),
        _ => None,
    }
}

/// Moves each RGB channel toward black by a fixed absolute amount (not a
/// percentage of the channel's own value), clamped at 0. Deliberately not
/// multiplicative: this project's default theme (Kanagawa Wave, background
/// rgb(54,54,70) per config/config.toml) and several bundled dark themes
/// have backgrounds too dark for a percentage-based step to produce a
/// visible difference (0.94x of 54 is 51 - a 3/255 shift).
fn darken(color: Color, amount: f32) -> Color {
    Color {
        r: (color.r - amount).max(0.0),
        g: (color.g - amount).max(0.0),
        b: (color.b - amount).max(0.0),
        a: color.a,
    }
}

const INACTIVE_TAB_DARKEN: f32 = 0.035; // ~9/255 per channel
const TAB_ROW_DARKEN: f32 = 0.09; // ~23/255 per channel

/// The (background, text) pair for a tab chip - shared by the title button
/// and the close button so they always agree exactly, rather than each
/// computing it separately and risking drift.
fn tab_chip_colors(theme: &Theme, is_active: bool) -> (Color, Color) {
    let palette = theme.extended_palette();
    let background = palette.background.base.color;
    if is_active {
        (background, palette.background.base.text)
    } else {
        (
            darken(background, INACTIVE_TAB_DARKEN),
            palette.background.base.text.scale_alpha(0.7),
        )
    }
}

/// A tab's title button: always transparent - the enclosing `tab_frame_style`
/// container is what paints the tab's background, so title and close never
/// have to agree on a box size to look like one continuous surface.
fn tab_title_style(theme: &Theme, _status: button::Status, is_active: bool) -> button::Style {
    let (_, text_color) = tab_chip_colors(theme, is_active);
    button::Style {
        background: None,
        text_color,
        ..button::Style::default()
    }
}

/// A tab's close button: transparent at rest (same reasoning as
/// `tab_title_style`), with a faint highlight only on hover/press as the
/// only background it ever paints itself.
fn tab_close_style(theme: &Theme, status: button::Status, is_active: bool) -> button::Style {
    let (_, text_color) = tab_chip_colors(theme, is_active);
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => {
            Some(text_color.scale_alpha(0.15).into())
        }
        _ => None,
    };
    button::Style {
        background,
        text_color,
        ..button::Style::default()
    }
}

/// The frame behind a tab's title+close row - the only thing that paints a
/// tab's background. Active tab matches the editor's own background
/// (`tab_chip_colors`), so it reads as a seamless continuation of the
/// document below it; inactive tabs get a darker background instead of a
/// border to distinguish them.
fn tab_frame_style(theme: &Theme, is_active: bool) -> container::Style {
    let (background, _) = tab_chip_colors(theme, is_active);
    container::Style::default().background(background)
}

/// The "+" new-tab button: transparent and dim at rest so it doesn't
/// compete with the tabs themselves, brightening on hover/press as the
/// only affordance that it's interactive.
fn new_tab_style(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let text_color = match status {
        button::Status::Hovered | button::Status::Pressed => palette.background.base.text,
        _ => palette.background.base.text.scale_alpha(0.4),
    };
    button::Style {
        background: None,
        text_color,
        ..button::Style::default()
    }
}

/// The tab row's own background - darker than even an inactive tab, so any
/// empty space past the last tab reads as a distinct "frame" the tabs sit
/// in, not a transparent gap.
fn tab_bar_style(theme: &Theme) -> container::Style {
    let background = darken(theme.extended_palette().background.base.color, TAB_ROW_DARKEN);
    container::Style::default().background(background)
}

/// Nudges `theme`'s background alpha by an amount too small to see, but
/// large enough that `Color`'s `==` (which iced's tiny-skia compositor
/// uses to decide whether a repaint is even necessary) reports a
/// difference from last frame. See AGENTS.md's "Known upstream rendering
/// bug" section: without this, switching tabs can leave stale content on
/// screen because the compositor's own per-widget damage check has a gap
/// for text editors specifically. A background color mismatch bypasses
/// that check entirely and forces a full repaint - this makes one happen
/// deliberately, exactly when a tab switches.
///
/// The nudge is invisible because it's blended against a backdrop the
/// compositor itself just cleared to that same base color a moment
/// earlier - mixing a color with itself at less than full opacity still
/// produces that same color, regardless of how much less.
///
/// Uses `Theme::custom` because the built-in named variants (`Theme::Light`,
/// `Theme::KanagawaWave`, ...) don't have a way to override one color -
/// `Theme::palette()` returns a plain, adjustable `Palette` regardless of
/// which named variant produced it, so this works uniformly for all of
/// them without matching on the specific variant.
fn nudge_background(theme: &Theme) -> Theme {
    let mut palette = theme.palette();
    palette.background.a -= 0.001;
    Theme::custom("xizor (redraw nudge)".to_string(), palette)
}

/// Matches a config-file theme name against `Theme::ALL` by display name
/// (`"Dracula"`, `"Solarized Light"`, ...), case-insensitively so hand-edited
/// TOML doesn't have to get the exact casing right. Falls back to the
/// default theme - and logs why - rather than failing startup over a typo.
fn resolve_theme(name: &str) -> Theme {
    let theme = Theme::ALL
        .iter()
        .find(|theme| theme.to_string().eq_ignore_ascii_case(name.trim()));

    match theme {
        Some(theme) => theme.clone(),
        None => {
            let valid = Theme::ALL
                .iter()
                .map(Theme::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "xizor: unknown theme {name:?}, using default. Valid options: {valid}"
            );
            // Iced 0.13's `Theme::default()` (removed in 0.14 along with the
            // `auto-detect-theme` feature) auto-detected the OS's light/dark
            // preference via the `dark-light` crate - never something this
            // app itself surfaced or relied on beyond this one fallback, so
            // there's nothing to replace it with: just match the theme name
            // xizor_config::defaults already ships as its own default.
            Theme::Light
        }
    }
}

/// Where syntax-highlighting wasm grammars (`<extension>.wasm`) are looked
/// for. A placeholder for now - trivially replaceable once a real
/// download-manager/config-dir flow exists.
fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("syntaxes"));
        }
    }
    dirs.push(PathBuf::from("syntaxes")); // convenience for `cargo run`
    dirs
}

/// Startup diagnostic: lists the `.wasm` files actually found in each
/// syntax-highlighting search directory, so "highlighting isn't showing
/// up" can be narrowed down to "grammar file missing" vs. something else.
fn log_wasm_files_found(dirs: &[PathBuf]) {
    for dir in dirs {
        match std::fs::read_dir(dir) {
            Ok(entries) => {
                let mut wasm_files: Vec<String> = entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("wasm"))
                    .filter_map(|path| {
                        path.file_name().map(|name| name.to_string_lossy().into_owned())
                    })
                    .collect();
                wasm_files.sort();
                if wasm_files.is_empty() {
                    eprintln!("xizor: {}: exists but no .wasm files found", dir.display());
                } else {
                    eprintln!(
                        "xizor: {}: found {} .wasm file(s): {}",
                        dir.display(),
                        wasm_files.len(),
                        wasm_files.join(", ")
                    );
                }
            }
            Err(err) => {
                eprintln!("xizor: {}: couldn't read directory: {err}", dir.display());
            }
        }
    }
}

/// Re-reads a restored tab's real file fresh from disk at startup (see
/// `XizorApp::new`'s restore path) - always the *current* on-disk content,
/// never a cached copy, since the file may have changed since last session.
async fn reload_from_disk(path: PathBuf) -> Result<Arc<String>, std::io::ErrorKind> {
    tokio::fs::read_to_string(&path)
        .await
        .map(Arc::new)
        .map_err(|err| err.kind())
}

/// Shows the native "you have unsaved changes" prompt for a tab being
/// closed (see `XizorApp::request_close`) and maps its result down to the
/// three outcomes the caller cares about. Anything other than the "Save" or
/// "Don't Save" custom buttons (e.g. the dialog's own close button, or a
/// platform that doesn't support custom labels and falls back to its
/// default) is treated as `Cancel` - closing is the one destructive path
/// here, so an unrecognized answer should never be read as consent to lose
/// changes.
async fn confirm_close(tab_title: String) -> CloseDecision {
    let result = rfd::AsyncMessageDialog::new()
        .set_title("Unsaved Changes")
        .set_description(format!(
            "Do you want to save the changes you made to {tab_title}?"
        ))
        .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
            "Save".to_string(),
            "Don't Save".to_string(),
            "Cancel".to_string(),
        ))
        .show()
        .await;

    match result {
        rfd::MessageDialogResult::Custom(label) if label == "Save" => CloseDecision::Save,
        rfd::MessageDialogResult::Custom(label) if label == "Don't Save" => {
            CloseDecision::DontSave
        }
        // Not every backend's async path actually honors custom button
        // text - notably Windows' plain `MessageBoxW` fallback (this crate
        // isn't built with rfd's `common-controls-v6` feature, which is
        // what's needed for real custom-labeled buttons there). Those
        // backends silently render/report the built-in Yes/No/Cancel
        // instead, so without this arm every click here - including the
        // one meaning "yes, save" - fell through to `_` and was treated as
        // Cancel. The buttons read "Yes"/"No" instead of "Save"/"Don't
        // Save" on those platforms, but the semantics still line up.
        rfd::MessageDialogResult::Yes => CloseDecision::Save,
        rfd::MessageDialogResult::No => CloseDecision::DontSave,
        _ => CloseDecision::Cancel,
    }
}

async fn open_and_read() -> Result<(PathBuf, Arc<String>), OpenError> {
    let handle = rfd::AsyncFileDialog::new()
        .pick_file()
        .await
        .ok_or(OpenError::DialogClosed)?;
    let path = handle.path().to_owned();
    let contents = tokio::fs::read_to_string(&path)
        .await
        .map(Arc::new)
        .map_err(|err| OpenError::Io {
            path: path.clone(),
            kind: err.kind(),
        })?;
    Ok((path, contents))
}

async fn save_to(
    existing_path: Option<PathBuf>,
    text: String,
    force_dialog: bool,
) -> Result<PathBuf, SaveError> {
    let path = if force_dialog || existing_path.is_none() {
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(existing) = &existing_path {
            if let Some(dir) = existing.parent() {
                dialog = dialog.set_directory(dir);
            }
        }
        dialog
            .save_file()
            .await
            .map(|handle| handle.path().to_owned())
            .ok_or(SaveError::DialogClosed)?
    } else {
        existing_path.expect("checked above: existing_path is Some when not force_dialog")
    };

    tokio::fs::write(&path, text)
        .await
        .map_err(|err| SaveError::Io {
            path: path.clone(),
            kind: err.kind(),
        })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::TextEditorWidget;
    use iced::keyboard::key::Named;
    use iced::keyboard::{Key, Modifiers};

    /// A minimal `TextEditorWidget` for tests, so a `Tab`/`XizorApp` can be
    /// built without pulling in `iced_text_editor`/`syntax_registry` or any
    /// real rendering.
    struct StubEditor;

    impl TextEditorWidget for StubEditor {
        fn view(&self) -> Element<'_, EditorMessage> {
            iced::widget::text("").into()
        }
        fn update(&mut self, _message: EditorMessage) -> bool {
            false
        }
        fn text(&self) -> String {
            String::new()
        }
        fn set_text(&mut self, _text: &str) {}
        fn poll_highlighting(&mut self) {}
        fn cursor_position(&self) -> (usize, usize) {
            (0, 0)
        }
        fn move_cursor_to(&mut self, _line: usize, _column: usize) {}
        fn has_pending_highlighting(&self) -> bool {
            false
        }
    }

    fn stub_factory() -> EditorFactory {
        Box::new(|_text, _extension| Box::new(StubEditor) as Box<dyn TextEditorWidget>)
    }

    /// Builds an `XizorApp` with `tab_count` untitled tabs (ids `0..tab_count`)
    /// and none of the real I/O `XizorApp::new` would otherwise do - just
    /// enough state for the tab-switching logic under test.
    fn test_app(tab_count: u64) -> XizorApp {
        let factory = stub_factory();
        let tabs = (0..tab_count).map(|id| Tab::untitled(id, &factory)).collect();
        XizorApp {
            tabs,
            active: 0,
            next_id: tab_count,
            error: None,
            editor_factory: factory,
            theme: Theme::ALL[0].clone(),
            redraw_nudge: false,
            session_dir: PathBuf::from("/tmp"),
            pending_close_after_save: Vec::new(),
            window: None,
            hotkey: None,
            visor_visible: false,
            animation: None,
            visor_enabled: false,
            previous_active: None,
            keybind_overrides: Arc::new(HashMap::new()),
        }
    }

    #[test]
    fn switch_active_records_previous_tab_id() {
        let mut app = test_app(3);
        let _ = app.switch_active(1);
        assert_eq!(app.previous_active, Some(0));
        let _ = app.switch_active(2);
        assert_eq!(app.previous_active, Some(1));
    }

    #[test]
    fn select_previous_active_tab_toggles_back_and_forth() {
        let mut app = test_app(3);
        let _ = app.switch_active(2);
        assert_eq!(app.active, 2);
        let _ = app.update(Message::SelectPreviousActiveTab);
        assert_eq!(app.active, 0);
        let _ = app.update(Message::SelectPreviousActiveTab);
        assert_eq!(app.active, 2);
    }

    #[test]
    fn select_previous_active_tab_is_noop_for_a_stale_id() {
        let mut app = test_app(2);
        app.previous_active = Some(999); // no tab has this id - e.g. since closed
        let _ = app.update(Message::SelectPreviousActiveTab);
        assert_eq!(app.active, 0);
    }

    #[test]
    fn cycle_tab_wraps_around_in_both_directions() {
        let mut app = test_app(3);
        let _ = app.update(Message::SelectPreviousTab);
        assert_eq!(app.active, 2);
        let _ = app.update(Message::SelectNextTab);
        assert_eq!(app.active, 0);
        let _ = app.update(Message::SelectNextTab);
        assert_eq!(app.active, 1);
    }

    #[test]
    fn cycle_tab_on_single_tab_is_noop() {
        let mut app = test_app(1);
        let _ = app.update(Message::SelectNextTab);
        assert_eq!(app.active, 0);
    }

    fn no_overrides() -> HashMap<(Modifiers, key::Code), Message> {
        HashMap::new()
    }

    #[test]
    fn ctrl_tab_is_recognized_as_select_previous_active_tab() {
        let tab_key = Key::Named(Named::Tab);
        assert!(matches!(
            handle_hotkey(
                tab_key,
                Modifiers::CTRL,
                key::Physical::Code(key::Code::Tab),
                &no_overrides()
            ),
            Some(Message::SelectPreviousActiveTab)
        ));
    }

    #[test]
    fn ctrl_tab_with_extra_modifiers_does_not_match() {
        let tab_key = Key::Named(Named::Tab);
        let physical = key::Physical::Code(key::Code::Tab);
        assert!(handle_hotkey(
            tab_key.clone(),
            Modifiers::CTRL | Modifiers::SHIFT,
            physical,
            &no_overrides()
        )
        .is_none());
        assert!(
            handle_hotkey(tab_key, Modifiers::CTRL | Modifiers::ALT, physical, &no_overrides())
                .is_none()
        );
    }

    #[test]
    fn hardcoded_default_still_fires_with_empty_overrides() {
        assert!(matches!(
            handle_hotkey(
                Key::Character("n".into()),
                Modifiers::CTRL,
                key::Physical::Code(key::Code::KeyN),
                &no_overrides()
            ),
            Some(Message::NewTab)
        ));
    }

    #[test]
    fn override_wins_over_a_conflicting_hardcoded_default() {
        // Ctrl+N would otherwise hit the hardcoded default's NewTab binding
        // - override it onto OpenFile instead, and confirm the override
        // wins (proves tier-1-before-tier-2 ordering, not just "a new
        // binding got added").
        let mut overrides = HashMap::new();
        overrides.insert((Modifiers::CTRL, key::Code::KeyN), Message::OpenFile);
        assert!(matches!(
            handle_hotkey(
                Key::Character("n".into()),
                Modifiers::CTRL,
                key::Physical::Code(key::Code::KeyN),
                &overrides
            ),
            Some(Message::OpenFile)
        ));
    }

    #[test]
    fn build_app_overrides_ignores_unrecognized_command_name() {
        let mut keybinds = xizor_config::KeybindsConfig::default();
        keybinds.overrides.insert(
            "frobnicate".to_string(),
            global_hotkey::hotkey::HotKey::new(
                Some(global_hotkey::hotkey::Modifiers::CONTROL),
                global_hotkey::hotkey::Code::KeyN,
            ),
        );
        assert!(build_app_overrides(&keybinds).is_empty());
    }
}
