use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use editor_core::{EditorFactory, EditorMessage, Tab};
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use iced::advanced::widget::{operate, operation};
use iced::keyboard::key;
use iced::widget::{
    button, center, column, container, keyed_column, mouse_area, row, scrollable, stack, text,
};
use iced::{Center, Color, Element, Fill, Point, Subscription, Task, Theme, keyboard};

use crate::hotkey::{self, Hotkey};
use crate::session;
use crate::visor::{self, Animation};

const VISOR_ANIM_TICK: Duration = Duration::from_millis(16);
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(5);

/// How often `Message::HeightCollapseTestTick` advances the wait after
/// `Message::TriggerHeightCollapseTest` collapses the window's height to
/// `0` - see that message's doc comment.
const HEIGHT_COLLAPSE_TEST_TICK: Duration = Duration::from_millis(16);

/// How many `HEIGHT_COLLAPSE_TEST_TICK`s to wait, with the window's height
/// collapsed to `0`, before restoring it - see
/// `Message::TriggerHeightCollapseTest`'s doc comment.
const HEIGHT_COLLAPSE_TEST_WAIT_FRAMES: u8 = 1;

pub struct XizorApp {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    error: Option<String>,
    editor_factory: EditorFactory,
    theme: Theme,
    background_alpha: f32,
    height_collapse_test: Option<(iced::Size, u8)>,
    /// Flips every time the active tab changes.
    redraw_nudge_hack: bool,
    session_dir: PathBuf,
    /// List of tab ids waiting on a save
    pending_close_after_save: Vec<u64>,
    window: Option<iced::window::Id>,
    /// The registered global toggle hotkey
    hotkey: Option<Hotkey>,
    /// Whether the visor is (or is animating toward being) shown.
    visor_visible: bool,
    /// `Some` while a show/hide slide is in progress - see
    /// `Message::AnimationTick` and `subscription()`'s gating of the
    /// animation timer on this.
    animation: Option<Animation>,
    visor_enabled: bool,
    previous_active_id: Option<u64>,
    keybind_overrides: Arc<HashMap<(keyboard::Modifiers, key::Code), Message>>,
    /// The unsaved-changes prompt currently being shown, if any
    pending_close: Option<PendingClose>,
    /// Tab ids that asked to close while a prompt was already showing
    close_queue: Vec<u64>,
    file_dialog_active: bool,
}

/// State for the unsaved-changes modal (see `XizorApp::pending_close` and
/// `view()`) - `focused` indexes the three choices in their on-screen
/// left-to-right order (0=Save, 1=Don't Save, 2=Cancel), cycled by
/// `Message::KeyPressed` while a prompt is showing.
struct PendingClose {
    tab_id: u64,
    title: String,
    focused: usize,
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
    /// Fetches the window's current size, then collapses its height to `0`;
    /// `Message::HeightCollapseTestTick` waits `HEIGHT_COLLAPSE_TEST_WAIT_FRAMES`
    /// frames with it collapsed before restoring the original size.
    ///
    /// Fires two ways: automatically once, right after `WindowReady`, only
    /// when `background_alpha < 1.0` (see that handler) - and by hand, on
    /// Ctrl+` (matching `keybinds.toml`'s default `toggle` chord, though not
    /// actually routed through that config - see `handle_hotkey`), kept
    /// around so this can still be re-triggered on demand for testing
    /// without restarting the app.
    ///
    /// Why this exists: on Windows, a freshly created transparent `wgpu`
    /// surface renders fully opaque (or shows a wrong, additively-brightened
    /// tint at low alpha - looks like a straight-vs-premultiplied-alpha
    /// mismatch in how the DXGI swapchain gets negotiated) until the window
    /// goes through a real resize - confirmed reliable and permanent for the
    /// rest of that session once it happens, no matter whether that resize
    /// was a manual drag or this collapse-and-restore. Whether height alone
    /// is enough (vs. needing width too), and how short the wait can be, was
    /// worked out by hand via the Ctrl+` trigger before wiring this into
    /// startup automatically.
    TriggerHeightCollapseTest,
    /// The window size `Message::TriggerHeightCollapseTest` asked for,
    /// resolved - arms `height_collapse_test` with it and immediately
    /// collapses the window's height to `0`.
    HeightCollapseTestSized(iced::Size),
    /// Fires on a timer while `height_collapse_test` is `Some` (see
    /// `subscription()`'s gating); counts down the wait, then restores the
    /// window to its original size once `HEIGHT_COLLAPSE_TEST_WAIT_FRAMES`
    /// have passed.
    HeightCollapseTestTick,
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
            config.alpha.background,
            config.alpha.foreground,
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
            background_alpha: config.alpha.background.clamp(0.0, 1.0),
            height_collapse_test: None,
            redraw_nudge_hack: false,
            session_dir,
            pending_close_after_save: Vec::new(),
            window: None,
            hotkey,
            visor_visible: false,
            animation: None,
            visor_enabled,
            previous_active_id: None,
            keybind_overrides,
            pending_close: None,
            close_queue: Vec::new(),
            file_dialog_active: false,
        };

        let window_task = iced::window::latest().map(Message::WindowReady);

        let Some(manifest) = manifest.filter(|manifest| !manifest.tabs.is_empty()) else {
            let task = app.new_tab();
            // `new_tab()`'s own `switch_active(0)` always no-ops here (the
            // tab list was empty, so the new tab's index and `self.active`
            // are both `0`) - focus_next() is never redundant in this
            // branch, so it's safe to always add it (see the doc comment
            // on the final return below for why that's not true everywhere).
            return (
                app,
                Task::batch([task, window_task, operate(operation::focusable::focus_next())]),
            );
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
            // Same reasoning as the no-manifest branch above.
            return (
                app,
                Task::batch([task, window_task, operate(operation::focusable::focus_next())]),
            );
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
        // `switch_active` only calls `focus_next()` itself when
        // `desired_active != app.active` (still `0` here) - so when the
        // restored session's active tab genuinely was index 0, that
        // call never fires and needs adding explicitly. When it's
        // non-zero, `switch_active` already focused correctly:
        // `focus_next()` *cycles* (confirmed against iced_core's
        // `operation/focusable.rs`) - since this app only ever has one
        // focusable widget in the tree at a time, calling it again here
        // would unfocus that already-correct widget rather than being a
        // harmless no-op, so it must not be added in that case.
        let focus_task = if desired_active == 0 {
            operate(operation::focusable::focus_next())
        } else {
            Task::none()
        };
        let task = Task::batch(
            reload_tasks
                .into_iter()
                .chain([switch_task, focus_task, window_task]),
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
    /// immediately; dirty tabs get an unsaved-changes prompt first (see
    /// `view()`'s modal and `Message::KeyPressed`'s handling while
    /// `pending_close` is set), with the actual close (or save-then-close)
    /// happening once the user answers it - see `Message::CloseConfirmed`.
    ///
    /// If a prompt is already showing, this queues the request instead of
    /// showing a second one - only one prompt is ever on screen at a time,
    /// even for the same tab id (a rapid double-click on the same tab's "x"
    /// used to spawn two concurrent native dialogs before this existed).
    fn request_close(&mut self, index: usize) -> Task<Message> {
        let Some(tab) = self.tabs.get(index) else {
            return Task::none();
        };
        if !tab.dirty {
            return self.close_tab(index);
        }
        let id = tab.id;
        if self.pending_close.is_some() {
            if !self.close_queue.contains(&id) {
                self.close_queue.push(id);
            }
            return Task::none();
        }
        self.pending_close = Some(PendingClose {
            tab_id: id,
            title: tab.document.display_name(),
            focused: 0,
        });
        // Blur the editor so the same keystrokes that navigate the modal
        // (Enter, Tab, ...) can't also be typed into the hidden document -
        // see `Message::KeyPressed`'s handling while `pending_close` is set.
        operate(operation::focusable::unfocus())
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

    fn switch_active(&mut self, index: usize) -> Task<Message> {
        if index >= self.tabs.len() || index == self.active {
            return Task::none();
        }
        if let Some(previous) = self.tabs.get_mut(self.active) {
            previous.last_cursor = previous.editor.cursor_position();
            self.previous_active_id = Some(previous.id);
        }
        self.active = index;
        self.redraw_nudge_hack = !self.redraw_nudge_hack;
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

    /// Rewrites the session manifest from current state, pruning any 
    /// orphaned draft files. Synchronous since the file is small
    fn sync_session_metadata(&self) {
        let manifest = session::build_manifest(&self.tabs, self.active);
        session::write_manifest_sync(&self.session_dir, &manifest);
    }

    fn save_active_tab(&mut self, force_dialog: bool) -> Task<Message> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Task::none();
        };
        let id = tab.id;
        self.save_tab(id, force_dialog)
    }

    /// Saves the tab with the given id. Shows a file dialog if the tab has no
    /// associated file. Otherwise, saves to the associated file
    fn save_tab(&mut self, id: u64, force_dialog: bool) -> Task<Message> {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return Task::none();
        };
        let existing_path = tab.document.path.clone();
        let shows_dialog = force_dialog || existing_path.is_none();
        if shows_dialog {
            if self.file_dialog_active {
                return Task::none();
            }
            self.file_dialog_active = true;
        }
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .expect("checked above: a tab with this id exists");
        let text = tab.editor.text();
        Task::perform(save_to(existing_path, text, force_dialog), move |result| {
            Message::FileSaved(id, result)
        })
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NewTab => self.new_tab(),
            Message::OpenFile => {
                if self.file_dialog_active {
                    Task::none()
                } else {
                    self.file_dialog_active = true;
                    Task::perform(open_and_read(), Message::FileOpened)
                }
            }
            Message::FileOpened(Ok((path, contents))) => {
                self.file_dialog_active = false;
                let id = self.next_id;
                self.next_id += 1;
                self.tabs
                    .push(Tab::from_file(id, path, &contents, &self.editor_factory));
                self.switch_active(self.tabs.len() - 1)
            }
            Message::FileOpened(Err(OpenError::DialogClosed)) => {
                self.file_dialog_active = false;
                Task::none()
            }
            Message::FileOpened(Err(OpenError::Io { path, kind })) => {
                self.file_dialog_active = false;
                self.error = Some(format!("Couldn't open {}: {kind}", path.display()));
                Task::none()
            }
            Message::SaveFile => self.save_active_tab(false),
            Message::SaveFileAs => self.save_active_tab(true),
            Message::FileSaved(id, Ok(path)) => {
                self.file_dialog_active = false;
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
                // The user canceled the save dialog - leave the tab open
                // rather than closing it unsaved.
                self.file_dialog_active = false;
                self.pending_close_after_save.retain(|&pending_id| pending_id != id);
                Task::none()
            }
            Message::FileSaved(id, Err(SaveError::Io { path, kind })) => {
                self.file_dialog_active = false;
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
            Message::SelectPreviousActiveTab => match self.previous_active_id {
                Some(id) => match self.tabs.iter().position(|tab| tab.id == id) {
                    Some(index) => self.switch_active(index),
                    // That tab was closed since - do nothing
                    None => Task::none(),
                },
                None => Task::none(),
            },
            Message::KeyPressed(key, modifiers, physical_key) => {
                // Unsaved-changes modal intercepts all keystrokes while open
                if let Some(pending) = &mut self.pending_close {
                    match key {
                        keyboard::Key::Named(key::Named::ArrowLeft) => {
                            pending.focused = (pending.focused + 2) % 3;
                            return Task::none();
                        }
                        keyboard::Key::Named(key::Named::ArrowRight) => {
                            pending.focused = (pending.focused + 1) % 3;
                            return Task::none();
                        }
                        keyboard::Key::Named(key::Named::Tab) => {
                            pending.focused = if modifiers.shift() {
                                (pending.focused + 2) % 3
                            } else {
                                (pending.focused + 1) % 3
                            };
                            return Task::none();
                        }
                        keyboard::Key::Named(key::Named::Enter)
                        | keyboard::Key::Named(key::Named::Space) => {
                            let decision = match pending.focused {
                                0 => CloseDecision::Save,
                                1 => CloseDecision::DontSave,
                                _ => CloseDecision::Cancel,
                            };
                            let tab_id = pending.tab_id;
                            return self.update(Message::CloseConfirmed(tab_id, decision));
                        }
                        keyboard::Key::Named(key::Named::Escape) => {
                            let tab_id = pending.tab_id;
                            return self
                                .update(Message::CloseConfirmed(tab_id, CloseDecision::Cancel));
                        }
                        _ => return Task::none(),
                    }
                }
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
                // Defense in depth: `request_close` already blurs the
                // editor when the modal opens, but that takes effect on the
                // next diff/render pass, not synchronously against
                // messages already in flight this same tick.
                if self.pending_close.is_some() {
                    return Task::none();
                }
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
            Message::CloseConfirmed(id, decision) => {
                self.pending_close = None;
                let decision_task = match decision {
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
                };
                // If another tab is waiting to be confirmed, show its
                // prompt next; otherwise the modal's gone, so hand
                // keyboard focus back to the editor.
                let next_task = if self.close_queue.is_empty() {
                    operate(operation::focusable::focus_next())
                } else {
                    let next_id = self.close_queue.remove(0);
                    match self.tabs.iter().position(|tab| tab.id == next_id) {
                        Some(index) => self.request_close(index),
                        // That tab was closed some other way while queued -
                        // same stale-id-is-a-silent-no-op precedent used
                        // elsewhere in this file.
                        None => Task::none(),
                    }
                };
                Task::batch([decision_task, next_task])
            }
            Message::WindowReady(id) => {
                self.window = id;
                let snap_task = self.snap_to_monitor();
                // See `Message::TriggerHeightCollapseTest`'s doc comment -
                // same fix, fired automatically once at startup instead of
                // needing a manual Ctrl+`. Only when the window was
                // actually created transparent - a solid window never needs
                // this and shouldn't pay for it.
                let collapse_task = if self.background_alpha < 1.0 {
                    Task::done(Message::TriggerHeightCollapseTest)
                } else {
                    Task::none()
                };
                Task::batch([snap_task, collapse_task])
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
            Message::TriggerHeightCollapseTest => match self.window {
                Some(id) => iced::window::size(id).map(Message::HeightCollapseTestSized),
                None => Task::none(),
            },
            Message::HeightCollapseTestSized(size) => {
                let Some(id) = self.window else {
                    return Task::none();
                };
                self.height_collapse_test = Some((size, 0));
                iced::window::resize(id, iced::Size::new(size.width, 0.0))
            }
            Message::HeightCollapseTestTick => {
                let Some((original, waited)) = self.height_collapse_test else {
                    return Task::none();
                };
                let Some(id) = self.window else {
                    self.height_collapse_test = None;
                    return Task::none();
                };
                let next_waited = waited + 1;
                if next_waited >= HEIGHT_COLLAPSE_TEST_WAIT_FRAMES {
                    self.height_collapse_test = None;
                    return iced::window::resize(id, original);
                }
                self.height_collapse_test = Some((original, next_waited));
                Task::none()
            }
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
            let background_alpha = self.background_alpha;
            let frame = container(row![title, close].spacing(0).align_y(Center))
                .style(move |theme| tab_frame_style(theme, is_active, background_alpha));

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

        // Deliberately *not* one `container` painting a single background
        // behind the whole row (the previous approach): with the window
        // itself translucent, that meant every tab chip's own background
        // (painted on top, inside the row) blended twice against the
        // backdrop - once via this container's fill, once via the chip's
        // own - producing a visibly different, "double-blended" color than
        // the editor area's single-layer background, even though both use
        // the literal same base color. A `row` has no background of its
        // own to paint, so nothing sits between a chip and the true
        // backdrop except that one chip's own single fill - `filler` (the
        // same background color as an inactive tab, single-layered the same
        // way) covers only the leftover space past the last tab/`+` button,
        // never underlapping a real chip.
        // `.height(Fill)` here (tried first) let iced's flex layout blow the
        // whole row's height up to consume half the window: a `Length::Fill`
        // cross-axis child inside an otherwise-`Shrink` row isn't guaranteed
        // to simply match its shrink siblings' natural height the way a
        // `Length::Fill` *main*-axis child predictably divides remaining
        // space - so instead, matching `title`'s own padding makes the
        // filler's height come out the same as a real tab chip by
        // construction, no flex cross-axis inference involved at all.
        let background_alpha = self.background_alpha;
        let filler = container(text(""))
            .padding([6, 10])
            .width(Fill)
            .style(move |theme| tab_bar_style(theme, background_alpha));

        let tab_bar: Element<'_, Message> = row![
            scrollable(tabs_row).direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::new(),
            )),
            filler,
        ]
        .width(Fill)
        .align_y(Center)
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

        let Some(pending) = &self.pending_close else {
            return content.into();
        };

        // A choice's `on_press` is still wired up (not just keyboard-driven)
        // so a mouse click also works, and so it doubles as a visible
        // affordance for what Enter/Space would do.
        let choice = |label: &'static str, index: usize, decision: CloseDecision| {
            button(text(label))
                .padding([6, 14])
                .style(move |theme, status| modal_button_style(theme, status, pending.focused == index))
                .on_press(Message::CloseConfirmed(pending.tab_id, decision))
        };

        let dialog = container(
            column![
                text(format!(
                    "Do you want to save the changes you made to {}?",
                    pending.title
                )),
                row![
                    choice("Save", 0, CloseDecision::Save),
                    choice("Don't Save", 1, CloseDecision::DontSave),
                    choice("Cancel", 2, CloseDecision::Cancel),
                ]
                .spacing(10),
            ]
            .spacing(16)
            .padding(20),
        )
        .style(modal_dialog_style);

        // Covers the whole window so `Stack`'s event-capture-stops-
        // propagation behavior keeps clicks from reaching the tab bar or
        // editor underneath - deliberately has no `on_press` of its own, so
        // clicking it does nothing rather than dismissing the prompt (only
        // Escape/the Cancel button should risk losing unsaved work).
        let scrim = mouse_area(container(text("")).width(Fill).height(Fill).style(modal_scrim_style));

        stack![content, scrim, center(dialog)].into()
    }

    pub fn theme(&self) -> Theme {
        if self.redraw_nudge_hack {
            nudge_background(&self.theme)
        } else {
            self.theme.clone()
        }
    }

    /// The application-wide window `Style` - notably `background_color`,
    /// which the renderer uses to clear the whole window surface underneath
    /// every widget. Scaling this by `background_alpha` is what actually
    /// lets the desktop show through a `.transparent(true)` window; without
    /// it, the default (fully opaque) background_color would paint over the
    /// entire window regardless of any widget's own alpha.
    pub fn style(&self, theme: &Theme) -> iced::theme::Style {
        let mut style = iced::theme::default(theme);
        if self.background_alpha < 1.0 {
            style.background_color = style.background_color.scale_alpha(self.background_alpha);
        }
        style
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
                } => Some(Message::KeyPressed(key, modifiers, physical_key)),
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
                .push(iced::time::every(VISOR_ANIM_TICK).map(|_| Message::AnimationTick));
        }

        // Only ticks while `Message::TriggerHeightCollapseTest`'s wait
        // period is in progress. Same "no idle cost while settled" shape as
        // the other gated timers here.
        if self.height_collapse_test.is_some() {
            subscriptions.push(
                iced::time::every(HEIGHT_COLLAPSE_TEST_TICK)
                    .map(|_| Message::HeightCollapseTestTick),
            );
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

    // Ctrl+` - see `Message::TriggerHeightCollapseTest`'s doc comment.
    // Matched by physical code (like tier 1), not logical key, since
    // backquote/backtick shifts around by keyboard layout.
    if modifiers.control()
        && !modifiers.shift()
        && !modifiers.alt()
        && !modifiers.logo()
        && matches!(physical_key, key::Physical::Code(key::Code::Backquote))
    {
        return Some(Message::TriggerHeightCollapseTest);
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
/// border to distinguish them. `background_alpha` (`XizorApp::background_alpha`)
/// scales it the same way the window's own background is scaled (see
/// `XizorApp::style`), so a tab chip doesn't sit as an opaque island in an
/// otherwise-transparent window.
fn tab_frame_style(theme: &Theme, is_active: bool, background_alpha: f32) -> container::Style {
    let (background, _) = tab_chip_colors(theme, is_active);
    container::Style::default().background(apply_background_alpha(background, background_alpha))
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
/// in, not a transparent gap. `background_alpha` scales it the same way as
/// `tab_frame_style` - see that doc comment.
fn tab_bar_style(theme: &Theme, background_alpha: f32) -> container::Style {
    let background = darken(theme.extended_palette().background.base.color, TAB_ROW_DARKEN);
    container::Style::default().background(apply_background_alpha(background, background_alpha))
}

/// Scales `color`'s alpha by `background_alpha`, skipping the multiply
/// entirely at `1.0` (fully solid) - mirrors
/// `iced_text_editor`'s private `apply_alpha` (can't share it directly,
/// different crate), same "don't do transparency-related work when nothing's
/// actually transparent" intent as `XizorApp::background_alpha`'s doc
/// comment.
fn apply_background_alpha(color: Color, background_alpha: f32) -> Color {
    if background_alpha >= 1.0 {
        color
    } else {
        color.scale_alpha(background_alpha)
    }
}

/// One of the unsaved-changes modal's three choices - a colored border
/// (rather than a background fill, so it stays legible under hover/press
/// too) is the only visual difference for whichever one keyboard nav
/// currently has on `pending.focused` - see `view()`.
fn modal_button_style(theme: &Theme, status: button::Status, is_focused: bool) -> button::Style {
    let palette = theme.extended_palette();
    let border = if is_focused {
        iced::Border {
            color: palette.primary.strong.color,
            width: 2.0,
            radius: 4.0.into(),
        }
    } else {
        iced::Border {
            radius: 4.0.into(),
            ..iced::Border::default()
        }
    };
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => {
            Some(palette.background.weak.color.into())
        }
        _ => Some(palette.background.base.color.into()),
    };
    button::Style {
        background,
        text_color: palette.background.base.text,
        border,
        ..button::Style::default()
    }
}

/// The modal's own dialog box - opaque, so it reads as a real window sitting
/// on top of the scrim rather than another translucent layer.
fn modal_dialog_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style::default()
        .background(palette.background.base.color)
        .border(iced::Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 8.0.into(),
        })
}

/// The full-window backdrop behind the modal dialog - dark and translucent,
/// both to visually indicate the rest of the app is blocked and to give
/// `Stack`'s event capture something to swallow clicks with (see `view()`).
fn modal_scrim_style(_theme: &Theme) -> container::Style {
    container::Style::default().background(Color::from_rgba(0.0, 0.0, 0.0, 0.5))
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
            // Reports every message as an edit - lets tests observe whether
            // `Message::Editor` actually reached the editor (via
            // `tab.dirty`/`draft_generation`) without needing a real one.
            true
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
            background_alpha: 1.0,
            height_collapse_test: None,
            redraw_nudge_hack: false,
            session_dir: PathBuf::from("/tmp"),
            pending_close_after_save: Vec::new(),
            window: None,
            hotkey: None,
            visor_visible: false,
            animation: None,
            visor_enabled: false,
            previous_active_id: None,
            keybind_overrides: Arc::new(HashMap::new()),
            pending_close: None,
            close_queue: Vec::new(),
            file_dialog_active: false,
        }
    }

    #[test]
    fn switch_active_records_previous_tab_id() {
        let mut app = test_app(3);
        let _ = app.switch_active(1);
        assert_eq!(app.previous_active_id, Some(0));
        let _ = app.switch_active(2);
        assert_eq!(app.previous_active_id, Some(1));
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
        app.previous_active_id = Some(999); // no tab has this id - e.g. since closed
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

    fn key_press(named: Named, modifiers: Modifiers, code: key::Code) -> Message {
        Message::KeyPressed(Key::Named(named), modifiers, key::Physical::Code(code))
    }

    #[test]
    fn request_close_queues_a_second_request_instead_of_showing_a_second_prompt() {
        let mut app = test_app(3);
        app.tabs[0].dirty = true;
        app.tabs[1].dirty = true;
        let tab0_id = app.tabs[0].id;
        let tab1_id = app.tabs[1].id;

        let _ = app.request_close(0);
        assert_eq!(app.pending_close.as_ref().unwrap().tab_id, tab0_id);
        assert!(app.close_queue.is_empty());

        let _ = app.request_close(1);
        assert_eq!(app.pending_close.as_ref().unwrap().tab_id, tab0_id); // unchanged
        assert_eq!(app.close_queue, vec![tab1_id]);
    }

    #[test]
    fn request_close_does_not_duplicate_an_already_queued_id() {
        let mut app = test_app(3);
        app.tabs[0].dirty = true;
        app.tabs[1].dirty = true;
        let _ = app.request_close(0);
        let _ = app.request_close(1);
        let _ = app.request_close(1);
        assert_eq!(app.close_queue.len(), 1);
    }

    #[test]
    fn close_confirmed_opens_the_next_queued_prompt() {
        let mut app = test_app(3);
        app.tabs[0].dirty = true;
        app.tabs[1].dirty = true;
        let tab1_id = app.tabs[1].id;
        let _ = app.request_close(0);
        let _ = app.request_close(1);

        let tab0_id = app.tabs[0].id;
        let _ = app.update(Message::CloseConfirmed(tab0_id, CloseDecision::DontSave));

        assert!(app.close_queue.is_empty());
        assert_eq!(app.pending_close.as_ref().unwrap().tab_id, tab1_id);
    }

    #[test]
    fn key_pressed_cycles_focused_choice_both_directions_with_wraparound() {
        let mut app = test_app(1);
        app.tabs[0].dirty = true;
        let _ = app.request_close(0);
        assert_eq!(app.pending_close.as_ref().unwrap().focused, 0);

        let _ = app.update(key_press(Named::ArrowRight, Modifiers::empty(), key::Code::ArrowRight));
        assert_eq!(app.pending_close.as_ref().unwrap().focused, 1);

        let _ = app.update(key_press(Named::Tab, Modifiers::empty(), key::Code::Tab));
        assert_eq!(app.pending_close.as_ref().unwrap().focused, 2);

        // Wraps back around to 0.
        let _ = app.update(key_press(Named::ArrowRight, Modifiers::empty(), key::Code::ArrowRight));
        assert_eq!(app.pending_close.as_ref().unwrap().focused, 0);

        // Shift+Tab goes backward, wrapping to the last choice.
        let _ = app.update(key_press(Named::Tab, Modifiers::SHIFT, key::Code::Tab));
        assert_eq!(app.pending_close.as_ref().unwrap().focused, 2);

        let _ = app.update(key_press(Named::ArrowLeft, Modifiers::empty(), key::Code::ArrowLeft));
        assert_eq!(app.pending_close.as_ref().unwrap().focused, 1);
    }

    #[test]
    fn key_pressed_enter_resolves_whichever_choice_is_focused() {
        let mut app = test_app(2);
        app.tabs[0].dirty = true;
        let _ = app.request_close(0);
        // Move focus to "Don't Save" (index 1).
        let _ = app.update(key_press(Named::ArrowRight, Modifiers::empty(), key::Code::ArrowRight));
        let tabs_before = app.tabs.len();

        let _ = app.update(key_press(Named::Enter, Modifiers::empty(), key::Code::Enter));

        assert!(app.pending_close.is_none());
        assert_eq!(app.tabs.len(), tabs_before - 1); // Don't Save actually closed it
    }

    #[test]
    fn key_pressed_escape_always_cancels_regardless_of_focus() {
        let mut app = test_app(2);
        app.tabs[0].dirty = true;
        let _ = app.request_close(0);
        // Move focus to "Don't Save" - Escape should still cancel, not "Don't Save".
        let _ = app.update(key_press(Named::ArrowRight, Modifiers::empty(), key::Code::ArrowRight));
        let tabs_before = app.tabs.len();

        let _ = app.update(key_press(Named::Escape, Modifiers::empty(), key::Code::Escape));

        assert!(app.pending_close.is_none());
        assert_eq!(app.tabs.len(), tabs_before); // nothing closed
    }

    #[test]
    fn key_pressed_swallows_app_shortcuts_while_a_prompt_is_pending() {
        let mut app = test_app(1);
        app.tabs[0].dirty = true;
        let _ = app.request_close(0);
        let tabs_before = app.tabs.len();

        // Ctrl+N would normally fire NewTab (see hardcoded_default_still_fires_with_empty_overrides).
        let _ = app.update(Message::KeyPressed(
            Key::Character("n".into()),
            Modifiers::CTRL,
            key::Physical::Code(key::Code::KeyN),
        ));

        assert_eq!(app.tabs.len(), tabs_before);
        assert!(app.pending_close.is_some());
    }

    #[test]
    fn editor_messages_are_ignored_while_a_prompt_is_pending() {
        let mut app = test_app(1);
        app.tabs[0].dirty = true;
        let generation_before = app.tabs[0].draft_generation;
        let _ = app.request_close(0);

        let _ = app.update(Message::Editor(0, EditorMessage::Undo));

        assert_eq!(app.tabs[0].draft_generation, generation_before);
    }

    #[test]
    fn open_file_does_not_spawn_a_second_dialog_while_one_is_active() {
        let mut app = test_app(1);
        let _ = app.update(Message::OpenFile);
        assert!(app.file_dialog_active);
        // A second OpenFile while one's already in flight is a no-op - the
        // flag doesn't get set again, and (more importantly) no test here
        // can observe a second `rfd` dialog actually spawning, but the
        // guard itself not flipping is directly observable.
        let _ = app.update(Message::OpenFile);
        assert!(app.file_dialog_active);
    }

    #[test]
    fn file_dialog_flag_resets_on_every_file_opened_outcome() {
        let mut app = test_app(1);
        app.file_dialog_active = true;
        let _ = app.update(Message::FileOpened(Err(OpenError::DialogClosed)));
        assert!(!app.file_dialog_active);

        app.file_dialog_active = true;
        let _ = app.update(Message::FileOpened(Err(OpenError::Io {
            path: PathBuf::from("/tmp/x"),
            kind: std::io::ErrorKind::NotFound,
        })));
        assert!(!app.file_dialog_active);
    }

    #[test]
    fn file_dialog_flag_resets_on_every_file_saved_outcome() {
        let mut app = test_app(1);
        let id = app.tabs[0].id;

        app.file_dialog_active = true;
        let _ = app.update(Message::FileSaved(id, Err(SaveError::DialogClosed)));
        assert!(!app.file_dialog_active);

        app.file_dialog_active = true;
        let _ = app.update(Message::FileSaved(
            id,
            Err(SaveError::Io {
                path: PathBuf::from("/tmp/x"),
                kind: std::io::ErrorKind::NotFound,
            }),
        ));
        assert!(!app.file_dialog_active);

        app.file_dialog_active = true;
        let _ = app.update(Message::FileSaved(id, Ok(PathBuf::from("/tmp/x"))));
        assert!(!app.file_dialog_active);
    }

    #[test]
    fn save_tab_does_not_spawn_a_second_dialog_for_an_untitled_tab_while_one_is_active() {
        // A plain Save on a never-saved tab shows a file-picker just like
        // Open File does (`save_to`'s dialog gate is `force_dialog ||
        // existing_path.is_none()`) - confirm it's guarded the same way.
        let mut app = test_app(1);
        assert!(app.tabs[0].document.path.is_none());
        app.file_dialog_active = true;

        let _ = app.save_active_tab(false);

        // Still true - no second dialog-spawning task should have been
        // created (and, since the flag was already true, the guard should
        // have short-circuited before touching anything else).
        assert!(app.file_dialog_active);
    }
}
