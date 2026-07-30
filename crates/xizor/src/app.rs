use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use editor_core::{EditorFactory, EditorMessage, Tab};
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use iced::advanced::widget::{operate, operation};
use iced::keyboard::key;
use iced::widget::{
    button, center, column, container, keyed_column, mouse_area, row, scrollable, stack, text,
};
use iced::{Center, Color, Element, Fill, Pixels, Point, Subscription, Task, Theme, keyboard};

use crate::hotkey::{self, Hotkey};
use crate::session;
use crate::visor::{self, Animation};

const VISOR_ANIM_TICK: Duration = Duration::from_millis(16);
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(5);

/// How many consecutive frames to hold a nudged background color for after the
/// active tab changes (see AGENTS.md's stale-content bug). One frame is not
/// enough: `softbuffer` presents through several buffers in rotation, and
/// `iced_tiny_skia` only repaints the one it was handed. The buffers it skipped
/// still hold the previous tab's text, and come back around a frame or two
/// later. Nudging for several frames running repaints every buffer in the
/// rotation.
///
/// Zero on the wgpu backend: it has no damage tracking to defeat (it clears and
/// redraws the whole surface every frame), so a nudge there buys nothing and
/// the forced frames are pure waste. Zero also switches the whole mechanism
/// off, since `theme()` and `subscription()` both gate on the counter.
const REDRAW_NUDGE_FRAMES: u8 = if cfg!(feature = "tiny-skia") { 4 } else { 0 };

/// How many presented frames to wait before refreshing the macOS window
/// shadow after the content changes (see `macos.rs`). Refreshing on the same
/// frame re-caches the outgoing content; a couple of frames later, the new
/// content has actually presented.
const SHADOW_REFRESH_FRAMES: u8 = 3;

pub struct XizorApp {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    error: Option<String>,
    editor_factory: EditorFactory,
    theme: Theme,
    background_alpha: f32,
    /// Frames left to hold a nudged background color for, counted down from
    /// `REDRAW_NUDGE_FRAMES` every time the active tab changes.
    redraw_nudge_frames: u8,
    /// Frames left until the macOS window shadow is refreshed - see
    /// `SHADOW_REFRESH_FRAMES`.
    shadow_refresh_frames: u8,
    session_dir: PathBuf,
    /// List of tab ids waiting on a save
    pending_close_after_save: Vec<u64>,
    window: Option<iced::window::Id>,
    /// The registered global toggle hotkey
    hotkey: Option<Hotkey>,
    /// Whether the visor is (or is animating toward being) shown.
    visor_visible: bool,
    /// `Some` while a show/hide slide is in progress.
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

/// State for the unsaved-changes modal - `focused` indexes the three
/// choices left-to-right (0=Save, 1=Don't Save, 2=Cancel).
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
    /// A background draft write for tab `.0` finished at generation `.1`.
    DraftFlushed(u64, u64),
    /// The window's close button was clicked - flushes drafts before closing.
    WindowCloseRequested(iced::window::Id),
    /// A restored tab's real file finished (re-)reading from disk at startup.
    SessionFileLoaded(u64, Result<Arc<String>, std::io::ErrorKind>),
    /// The unsaved-changes prompt for tab `.0` came back with the user's choice.
    CloseConfirmed(u64, CloseDecision),
    /// The app's window id, resolved at startup.
    WindowReady(Option<iced::window::Id>),
    /// A global hotkey fired - filtered down to "was it a press of our
    /// toggle hotkey" before acting on it.
    HotkeyEvent(GlobalHotKeyEvent),
    /// Advances the visor's slide animation by one frame.
    AnimationTick,
    /// One presented frame elapsed - counts down `redraw_nudge_frames`.
    RedrawNudgeFrame,
    /// One presented frame elapsed - counts down `shadow_refresh_frames`,
    /// refreshing the macOS window shadow when it hits zero.
    ShadowRefreshFrame,
    /// The window was resized - the macOS shadow cache needs retaking.
    WindowResized,
    /// Cmd+Shift+] (mac) / Ctrl+Shift+] (elsewhere) - switch to the next tab,
    /// wrapping past the last back to the first.
    SelectNextTab,
    /// Cmd+Shift+[ (mac) / Ctrl+Shift+[ (elsewhere) - mirror of
    /// `SelectNextTab`.
    SelectPreviousTab,
    /// Ctrl+Tab - swap back to whichever tab was active immediately before this one.
    SelectPreviousActiveTab,
    /// A raw key press, resolved into a command by `handle_hotkey`. Kept as
    /// its own variant since `Subscription::filter_map`'s closure can't
    /// capture `self.keybind_overrides`.
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
        // No push-based wake-up needed (unlike egui's `ctx.request_repaint()`) -
        // the highlighting-poll subscription below re-checks periodically instead.
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
        // Skip registering the global hotkey entirely when visor mode is off,
        // rather than claiming the combo and then ignoring it.
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
            redraw_nudge_frames: 0,
            shadow_refresh_frames: 0,
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
            return (
                app,
                Task::batch([task, window_task, operate(operation::focusable::focus_next())]),
            );
        };

        // Dirty tabs read their draft file; clean, file-backed tabs get a
        // fresh async re-read of the real file rather than a stale cache.
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
                        // Draft unreadable - fall back to a clean re-read of the real file.
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
                        // No draft to recover - drop this entry.
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
        // `switch_active` only focuses when the index actually changes, so
        // a restored active index of 0 needs an explicit focus here too.
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
        let index = self.tabs.len() - 1;
        if index != self.active {
            return self.switch_active(index);
        }
        // The very first tab, or the stand-in for a closed last one: the index
        // didn't move, so `switch_active` would call it a no-op, but the editor
        // sitting at it is brand new and still needs focus and a repaint.
        self.redraw_nudge_frames = REDRAW_NUDGE_FRAMES;
        self.arm_shadow_refresh();
        self.sync_session_metadata();
        operate(operation::focusable::focus_next())
    }

    /// Closes a clean tab immediately; a dirty tab gets an unsaved-changes
    /// prompt first. Queues the request instead of showing a second prompt
    /// if one's already up.
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
        // Blur the editor so modal-navigation keystrokes don't also type
        // into the hidden document.
        operate(operation::focusable::unfocus())
    }

    fn close_tab(&mut self, index: usize) -> Task<Message> {
        if index >= self.tabs.len() {
            return Task::none();
        }
        self.tabs.remove(index);
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
        self.redraw_nudge_frames = REDRAW_NUDGE_FRAMES;
        if let Some(tab) = self.tabs.get_mut(index) {
            let (line, column) = tab.last_cursor;
            tab.editor.move_cursor_to(line, column);
        }
        self.sync_session_metadata();
        self.arm_shadow_refresh();
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

    /// Arms the shadow-refresh countdown (see `macos.rs`): the window server's
    /// shadow cache goes stale whenever the content changes, and refreshing it
    /// too early re-caches the outgoing frame, so the invalidation waits
    /// `SHADOW_REFRESH_FRAMES` presented frames.
    fn arm_shadow_refresh(&mut self) {
        if cfg!(target_os = "macos") && self.background_alpha < 1.0 {
            self.shadow_refresh_frames = SHADOW_REFRESH_FRAMES;
        }
    }

    #[cfg(target_os = "macos")]
    fn refresh_window_shadow(&self) -> Task<Message> {
        match self.window {
            Some(id) => iced::window::run(id, |window| {
                crate::macos::invalidate_window_shadow(window);
            })
            .discard(),
            None => Task::none(),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn refresh_window_shadow(&self) -> Task<Message> {
        Task::none()
    }

    /// Rewrites the session manifest, pruning orphaned draft files.
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
                // Tab just went clean - prune its stale draft file.
                self.sync_session_metadata();
                // If this was the "Save" branch of an unsaved-changes prompt, close now.
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
                // Belt and suspenders: the editor is blurred when the modal
                // opens, but that doesn't take effect until the next render.
                if self.pending_close.is_some() {
                    return Task::none();
                }
                if let Some(tab) = self.tabs.get_mut(index) {
                    if tab.editor.update(editor_message) {
                        let just_became_dirty = !tab.dirty;
                        tab.dirty = true;
                        tab.draft_generation += 1;
                        // Only sync on the clean->dirty transition, not every keystroke.
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
                        // Already closed some other way while queued.
                        None => Task::none(),
                    }
                };
                Task::batch([decision_task, next_task])
            }
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
            Message::WindowResized => {
                self.arm_shadow_refresh();
                Task::none()
            }
            Message::RedrawNudgeFrame => {
                self.redraw_nudge_frames = self.redraw_nudge_frames.saturating_sub(1);
                Task::none()
            }
            Message::ShadowRefreshFrame => {
                self.shadow_refresh_frames = self.shadow_refresh_frames.saturating_sub(1);
                if self.shadow_refresh_frames == 0 {
                    self.refresh_window_shadow()
                } else {
                    Task::none()
                }
            }
        }
    }

    /// Snaps the window to the primary monitor's current bounds - full
    /// width, one third the height - and parks it off-screen above the top,
    /// ready to slide into view. Called once at startup.
    fn snap_to_monitor(&mut self) -> Task<Message> {
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
        Task::batch([
            iced::window::resize(id, visor::visor_size(monitor)),
            iced::window::move_to(id, visor::hidden_position(monitor)),
        ])
    }

    /// Starts (or reverses) the visor's show/hide slide. Re-snaps width and
    /// x-position to the primary monitor's current bounds first, then tweens `y`.
    fn toggle_visor(&mut self) -> Task<Message> {
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

        // Reverse out of a not-yet-finished animation instead of jumping to
        // the settled position, so a rapid double-toggle doesn't glitch.
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
            // Lets the user start typing immediately after summoning the visor.
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

            let title = button(text(tab.title()).line_height(TAB_TITLE_LINE_HEIGHT))
                .padding([6, 10])
                .style(move |theme, status| tab_title_style(theme, status, is_active))
                .on_press(Message::SelectTab(index));

            let close = button(text("x").size(12).line_height(TAB_CLOSE_LINE_HEIGHT))
                .padding([6, 8])
                .style(move |theme, status| tab_close_style(theme, status, is_active))
                .on_press(Message::CloseTab(index));

            // The frame is the only thing that paints this tab's background -
            // title and close stay fully transparent so there's one seamless surface.
            let frame = container(row![title, close].spacing(0).align_y(Center))
                .style(move |theme| tab_frame_style(theme, is_active));

            // Middle-click closes it too, same as the "x".
            mouse_area(frame)
                .on_middle_press(Message::CloseTab(index))
                .into()
        });

        let new_tab_button = button(text("+").line_height(TAB_TITLE_LINE_HEIGHT))
            .padding([6, 10])
            .style(new_tab_style)
            .on_press(Message::NewTab);

        let tabs_row = row(tab_chips.chain(std::iter::once(new_tab_button.into())))
            .spacing(0)
            .align_y(Center);

        // No shared background container behind the row - on a transparent
        // window every extra layer compounds opacity, so the row is painted
        // once, in pieces. `filler` covers the leftover space past the last
        // chip, matching `title`'s padding and line height so its height lines
        // up without relying on flex cross-axis sizing.
        let filler = container(text("").line_height(TAB_TITLE_LINE_HEIGHT))
            .padding([6, 10])
            .width(Fill)
            .style(tab_bar_style);

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
            // Keyed by the tab's stable id, not its Vec index, so switching
            // tabs replaces the editor widget instead of reusing stale state.
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

        // Wired up with `on_press` too, so a click works the same as Enter/Space.
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

        // Covers the window to block clicks reaching what's underneath; no
        // `on_press`, so clicking it can't lose unsaved work by accident.
        let scrim = mouse_area(container(text("")).width(Fill).height(Fill).style(modal_scrim_style));

        stack![content, scrim, center(dialog)].into()
    }

    pub fn theme(&self) -> Theme {
        if self.redraw_nudge_frames > 0 {
            nudge_background(&self.theme, self.redraw_nudge_frames)
        } else {
            self.theme.clone()
        }
    }

    /// Scales the window's base `background_color` by `background_alpha` -
    /// needed for the desktop to actually show through a transparent window.
    pub fn style(&self, theme: &Theme) -> iced::theme::Style {
        let mut style = iced::theme::default(theme);
        if self.background_alpha < 1.0 {
            style.background_color = style.background_color.scale_alpha(self.background_alpha);
            // macOS composites the wgpu surface as premultiplied alpha, but
            // the clear color reaches it straight (see `premultiply`). Not on
            // tiny-skia: it premultiplies internally and would double-darken.
            if cfg!(all(target_os = "macos", feature = "wgpu")) {
                style.background_color = premultiply(style.background_color);
            }
        }
        style
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            // Filters the unified keyboard-event stream down to presses only.
            keyboard::listen().filter_map(|event| match event {
                keyboard::Event::KeyPressed {
                    key,
                    modifiers,
                    physical_key,
                    ..
                } => Some(Message::KeyPressed(key, modifiers, physical_key)),
                _ => None,
            }),
            iced::window::close_requests().map(Message::WindowCloseRequested),
            hotkey::subscription(),
            iced::window::resize_events().map(|_| Message::WindowResized),
        ];

        // Gated timers below: each only ticks while its condition holds, so
        // there's no idle cost once things settle.
        if self.animation.is_some() {
            subscriptions
                .push(iced::time::every(VISOR_ANIM_TICK).map(|_| Message::AnimationTick));
        }

        // Real presented frames, not a wall-clock tick - the point is to outlast
        // the platform's buffer rotation, which is counted in frames.
        if self.redraw_nudge_frames > 0 {
            subscriptions.push(iced::window::frames().map(|_| Message::RedrawNudgeFrame));
        }

        if self.shadow_refresh_frames > 0 {
            subscriptions.push(iced::window::frames().map(|_| Message::ShadowRefreshFrame));
        }

        if self.tabs.iter().any(|tab| tab.editor.has_pending_highlighting()) {
            subscriptions.push(
                iced::time::every(Duration::from_millis(50)).map(|_| Message::PollHighlighting),
            );
        }

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

/// App-level command names a `keybinds.toml` override may target.
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

/// Resolves `keybinds.toml`'s overrides into a lookup keyed by physical
/// key + modifiers, matching what `handle_hotkey` checks incoming presses against.
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

/// Mirror of `build_app_overrides` for editor-level commands - built here so
/// `iced_text_editor` doesn't need to depend on `xizor_config`/`global_hotkey`.
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
    // Tier 1: user override, matched by physical key (layout-independent).
    if let key::Physical::Code(code) = physical_key {
        if let Some(message) = overrides.get(&(modifiers, code)) {
            return Some(message.clone());
        }
    }

    // Tier 2: hardcoded shortcuts.
    //
    // Ctrl+Tab is the same on every OS, so this bypasses the `command()`
    // gate below (which resolves to Cmd on macOS).
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

/// Line heights for the tab bar's text, absolute so the strip's height - and
/// with it every horizontal quad edge in it - lands on a whole pixel. iced's
/// default `LineHeight::Relative(1.3)` would put the strip at 6 + 20.8 + 6 =
/// 32.8px, and a quad edge mid-pixel gets antialiased into a visible seam on a
/// transparent window (see AGENTS.md). Only holds at integer scale factors;
/// nothing chosen in logical pixels survives a 1.25x or 1.5x display.
const TAB_TITLE_LINE_HEIGHT: Pixels = Pixels(20.0); // 16px text, 1.3x rounded down
const TAB_CLOSE_LINE_HEIGHT: Pixels = Pixels(16.0); // 12px text, 1.3x rounded up

/// How far toward black to shade a tab-bar surface, as an absolute drop in
/// brightness rather than a percentage - a percentage step is too small to see
/// against very dark themes.
const INACTIVE_TAB_DARKEN: f32 = 0.035; // ~9/255 per channel
const TAB_ROW_DARKEN: f32 = 0.09; // ~23/255 per channel

/// How opaque a `darkening_wash` is ever allowed to get. Themes whose
/// background is near-black hit this before reaching the full step above -
/// darkening a transparent window costs opacity, and a solid tab row is a
/// worse trade than a shallower one.
const WASH_ALPHA_CEILING: f32 = 0.8;

/// A translucent black overlay that darkens whatever it covers by roughly
/// `amount`, rather than a pre-darkened copy of the background - on a
/// transparent window a full-alpha copy stacks with the window background and
/// leaves a visible opacity step (see AGENTS.md's hairline-seam gotcha).
/// Solid, it matches the subtractive darkening it replaced on average, shading
/// channels proportionally instead of uniformly.
fn darkening_wash(theme: &Theme, amount: f32) -> Color {
    let base = theme.extended_palette().background.base.color;
    let luminance = (base.r + base.g + base.b) / 3.0;
    // A background not much brighter than `amount` can't give up that much
    // light without a solid wash, which would turn the tab row into an opaque
    // bar on a transparent window. Those themes get a shallower step instead.
    let alpha = if luminance > 0.0 {
        (amount / luminance).clamp(0.0, WASH_ALPHA_CEILING)
    } else {
        0.0
    };
    Color { a: alpha, ..Color::BLACK }
}

/// A tab's text color - shared by the title button and the close button so
/// they always agree exactly, rather than each computing it separately and
/// risking drift.
fn tab_text_color(theme: &Theme, is_active: bool) -> Color {
    let text = theme.extended_palette().background.base.text;
    if is_active { text } else { text.scale_alpha(0.7) }
}

/// A tab's title button: always transparent - the enclosing `tab_frame_style`
/// container is what paints the tab's background, so title and close never
/// have to agree on a box size to look like one continuous surface.
fn tab_title_style(theme: &Theme, _status: button::Status, is_active: bool) -> button::Style {
    let text_color = tab_text_color(theme, is_active);
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
    let text_color = tab_text_color(theme, is_active);
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
/// tab's background. The active tab paints nothing: the window background
/// already *is* the editor's background, so matching it seamlessly means
/// adding no layer at all. Inactive tabs get a darkening wash instead of a
/// border.
fn tab_frame_style(theme: &Theme, is_active: bool) -> container::Style {
    if is_active {
        container::Style::default()
    } else {
        container::Style::default().background(darkening_wash(theme, INACTIVE_TAB_DARKEN))
    }
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

/// The tab row's own background - a wash darker than even an inactive tab, so
/// empty space past the last tab reads as a frame, not a gap.
fn tab_bar_style(theme: &Theme) -> container::Style {
    container::Style::default().background(darkening_wash(theme, TAB_ROW_DARKEN))
}

/// One of the unsaved-changes modal's three choices - a colored border marks
/// whichever one keyboard nav currently has focused.
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

/// The full-window backdrop behind the modal dialog, dark and translucent to
/// show the rest of the app is blocked.
fn modal_scrim_style(_theme: &Theme) -> container::Style {
    container::Style::default().background(Color::from_rgba(0.0, 0.0, 0.0, 0.5))
}

/// Nudges the theme's background alpha by an invisible amount, just enough to
/// force a full repaint on tab switch - see AGENTS.md's tiny-skia stale-content
/// bug. Scaling the nudge by `frames_left` makes every frame of the countdown a
/// *different* color, so each one repaints rather than only the first.
fn nudge_background(theme: &Theme, frames_left: u8) -> Theme {
    let mut palette = theme.palette();
    palette.background.a -= 0.001 * f32::from(frames_left);
    Theme::custom("xizor (redraw nudge)".to_string(), palette)
}

/// Premultiplies a color's RGB by its alpha, in linear space - the space the
/// clear color is handed to wgpu in (`Color::into_linear`).
///
/// **The solid-white-window fix.** The macOS window server composites the
/// surface as *premultiplied* alpha - `src + (1 - a) * desktop` - but iced's
/// clear color is written straight, so a straight white background saturates
/// to solid white at any alpha, while a near-black one (rgb ~ 0) happens to
/// look right; only light themes ever looked broken. Quads and glyphs are
/// unaffected - iced's shaders premultiply before writing (see AGENTS.md).
fn premultiply(color: Color) -> Color {
    let [r, g, b, a] = color.into_linear();
    Color::from_linear_rgba(r * a, g * a, b * a, a)
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
            Theme::Light
        }
    }
}

/// Where syntax-highlighting wasm grammars (`<extension>.wasm`) are looked for.
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

/// Startup diagnostic: lists the `.wasm` grammar files found in each search directory.
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

/// Re-reads a restored tab's real file fresh from disk, never a cached copy.
async fn reload_from_disk(path: PathBuf) -> Result<Arc<String>, std::io::ErrorKind> {
    tokio::fs::read_to_string(&path)
        .await
        .map(Arc::new)
        .map_err(|err| err.kind())
}

/// Shows the native Open File dialog and reads the chosen file's contents.
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
    let path = match existing_path {
        Some(existing) if !force_dialog => existing,
        existing_path => {
            let mut dialog = rfd::AsyncFileDialog::new();
            if let Some(dir) = existing_path.as_deref().and_then(Path::parent) {
                dialog = dialog.set_directory(dir);
            }
            dialog
                .save_file()
                .await
                .map(|handle| handle.path().to_owned())
                .ok_or(SaveError::DialogClosed)?
        }
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

    /// A minimal `TextEditorWidget` for tests, with no real rendering.
    struct StubEditor;

    impl TextEditorWidget for StubEditor {
        fn view(&self) -> Element<'_, EditorMessage> {
            iced::widget::text("").into()
        }
        fn update(&mut self, _message: EditorMessage) -> bool {
            true // reports every message as an edit
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

    /// Builds an `XizorApp` with `tab_count` untitled tabs, skipping the real I/O `new()` does.
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
            redraw_nudge_frames: 0,
            shadow_refresh_frames: 0,
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
        // Ctrl+N normally binds NewTab (tier 2) - override it to OpenFile.
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
        // A second OpenFile while one's in flight is a no-op.
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
        // A Save on a never-saved tab shows a file-picker just like Open File does.
        let mut app = test_app(1);
        assert!(app.tabs[0].document.path.is_none());
        app.file_dialog_active = true;

        let _ = app.save_active_tab(false);

        assert!(app.file_dialog_active);
    }

    /// Composites `wash` over `base` the way the renderer does, source-over.
    fn composite(base: Color, wash: Color) -> Color {
        Color {
            r: wash.r * wash.a + base.r * (1.0 - wash.a),
            g: wash.g * wash.a + base.g * (1.0 - wash.a),
            b: wash.b * wash.a + base.b * (1.0 - wash.a),
            a: wash.a + base.a * (1.0 - wash.a),
        }
    }

    fn luminance(color: Color) -> f32 {
        (color.r + color.g + color.b) / 3.0
    }

    #[test]
    fn darkening_wash_darkens_by_the_requested_amount_on_every_theme_bright_enough_to() {
        let mut checked = 0;
        for theme in Theme::ALL {
            let base = theme.extended_palette().background.base.color;
            // Near-black themes can't give up this much light without a solid
            // wash; they're capped by design, not mispredicted.
            if TAB_ROW_DARKEN / luminance(base) > WASH_ALPHA_CEILING {
                continue;
            }
            let washed = composite(base, darkening_wash(theme, TAB_ROW_DARKEN));
            let wanted = luminance(base) - TAB_ROW_DARKEN;
            assert!(
                (luminance(washed) - wanted).abs() < 0.001,
                "{theme}: washed to {}, wanted {wanted}",
                luminance(washed)
            );
            checked += 1;
        }
        assert!(checked > 0, "no theme was actually checked");
    }

    #[test]
    fn darkening_wash_never_goes_solid() {
        // The whole point: the wash adds a sliver of opacity where painting a
        // pre-darkened copy of the background would add a full second layer.
        for theme in Theme::ALL {
            let wash = darkening_wash(theme, TAB_ROW_DARKEN);
            assert!(
                wash.a <= WASH_ALPHA_CEILING,
                "{theme}: wash alpha {} exceeds the ceiling",
                wash.a
            );
        }
    }

    #[test]
    fn darkening_wash_paints_nothing_on_a_background_it_cannot_darken() {
        let theme = Theme::custom(
            "black".to_string(),
            iced::theme::Palette {
                background: Color::BLACK,
                ..Theme::Dark.palette()
            },
        );
        assert_eq!(darkening_wash(&theme, TAB_ROW_DARKEN).a, 0.0);
    }

    #[test]
    fn premultiply_scales_linear_rgb_by_alpha() {
        // Linear white is 1.0 per channel, so premultiplied it *is* the alpha.
        let [r, g, b, a] = premultiply(Color { a: 0.25, ..Color::WHITE }).into_linear();
        for (channel, value) in [("r", r), ("g", g), ("b", b)] {
            assert!((value - 0.25).abs() < 1e-5, "{channel} = {value}, wanted 0.25");
        }
        assert!((a - 0.25).abs() < 1e-6, "alpha must survive untouched, got {a}");
    }

    #[test]
    fn premultiply_leaves_black_black() {
        // Why dark themes never showed the bug: rgb ~ 0 premultiplies to itself.
        let color = premultiply(Color { a: 0.25, ..Color::BLACK });
        assert_eq!(color.r, 0.0);
        assert_eq!(color.g, 0.0);
        assert_eq!(color.b, 0.0);
        assert!((color.a - 0.25).abs() < 1e-6);
    }

    #[test]
    fn every_frame_of_the_nudge_countdown_gets_its_own_background_color() {
        // The compositor only bypasses its (text-blind) per-widget damage check
        // when the frame's background color differs from the last frame's, so
        // the countdown has to move it every frame, not just on the first.
        let mut app = test_app(2);
        app.background_alpha = 0.7;

        let mut seen = Vec::new();
        for frames in 0..=REDRAW_NUDGE_FRAMES {
            app.redraw_nudge_frames = frames;
            seen.push(app.style(&app.theme()).background_color);
        }

        for (index, color) in seen.iter().enumerate() {
            for other in &seen[index + 1..] {
                assert_ne!(color, other, "two frames share a background color: {seen:?}");
            }
        }
    }

    #[test]
    fn switching_tabs_arms_the_full_nudge_countdown() {
        let mut app = test_app(2);
        let _ = app.switch_active(1);
        assert_eq!(app.redraw_nudge_frames, REDRAW_NUDGE_FRAMES);
    }

    #[test]
    fn creating_a_tab_at_the_already_active_index_still_arms_the_nudge() {
        // `switch_active` treats an unmoved index as a no-op, but the first tab
        // at startup - and the stand-in for a closed last tab - lands on one
        // with a brand-new editor under it that has to be painted.
        let mut app = test_app(0);
        let _ = app.new_tab();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active, 0);
        assert_eq!(app.redraw_nudge_frames, REDRAW_NUDGE_FRAMES);
    }

    #[test]
    fn closing_the_last_tab_arms_the_nudge_for_its_replacement() {
        let mut app = test_app(1);
        app.redraw_nudge_frames = 0;
        let _ = app.close_tab(0);
        assert_eq!(app.tabs.len(), 1); // replaced, never left empty
        assert_eq!(app.redraw_nudge_frames, REDRAW_NUDGE_FRAMES);
    }

    #[test]
    fn the_nudge_countdown_stops_at_zero() {
        let mut app = test_app(2);
        app.redraw_nudge_frames = 1;
        let _ = app.update(Message::RedrawNudgeFrame);
        assert_eq!(app.redraw_nudge_frames, 0);
        let _ = app.update(Message::RedrawNudgeFrame);
        assert_eq!(app.redraw_nudge_frames, 0);
    }

    #[test]
    fn the_active_tab_paints_no_background_of_its_own() {
        // It has to read as one surface with the editor below it, and the
        // window background already paints that surface.
        let theme = Theme::Dark;
        assert!(tab_frame_style(&theme, true).background.is_none());
        assert!(tab_frame_style(&theme, false).background.is_some());
    }
}
