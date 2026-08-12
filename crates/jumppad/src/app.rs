use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use editor_core::{
    DiskStamp, EditorFactory, EditorMessage, FLOATING_SURFACE_DARKEN, SavedSelection,
    SelectionKind, Tab, darkening_wash,
};
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use iced::advanced::widget::{Id, operate, operation};
use iced::keyboard::key;
use iced::widget::{
    button, center, column, container, keyed_column, mouse_area, row, scrollable, stack, text,
    text_input,
};
use iced::{
    Center, Color, Element, Fill, Pixels, Point, Right, Subscription, Task, Theme, Top, keyboard,
};

use jumppad_actions::{Action, Context};

use crate::docwatch;
use crate::find::FindState;
use crate::hotkey::{self, Hotkey};
use crate::reload;
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

/// How many presented frames to wait before resetting the Windows redirection
/// surface (see `windows.rs`). Same reasoning as `SHADOW_REFRESH_FRAMES`:
/// winit already does this at window-creation time and it doesn't take, so
/// the whole point is to run it once a real swapchain frame has reached the
/// screen. Costs nothing on other platforms - it's only ever armed on Windows.
const SURFACE_RESET_FRAMES: u8 = 3;

/// Hands keyboard focus to the editor widget, targeted by its stable id.
/// Not `focusable::focus_next`: that op skips the editor whenever some
/// widget is still focused when it runs, which is exactly the state after a
/// keyboard-driven tab switch - the outgoing editor never saw the unfocusing
/// click a mouse switch produces, leaving the incoming editor unfocused
/// (typing dropped, caret and selection invisible).
fn focus_editor() -> Task<Message> {
    operate(operation::focusable::focus(Id::new(editor_core::EDITOR_WIDGET_ID)))
}

/// The find palette's query field, targeted by id for the same reason
/// `focus_editor` is - `focus_next` depends on what happens to be focused.
const FIND_INPUT_ID: &str = "find-input";

/// Focuses the query field and selects whatever is already in it, so
/// reopening the palette and typing replaces the old query instead of
/// appending to it (`the` + `the` = `thethe`).
fn focus_find() -> Task<Message> {
    Task::batch([
        operate(operation::focusable::focus(Id::new(FIND_INPUT_ID))),
        operate(operation::text_input::select_all(Id::new(FIND_INPUT_ID))),
    ])
}

pub struct JumpPadApp {
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
    /// Counts presented frames down to the one-shot Windows redirection-surface
    /// reset (see `windows.rs`), armed once when the window first appears.
    surface_reset_frames: u8,
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
    keybind_overrides: Arc<KeyOverrides>,
    /// The modal dialog currently being shown, if any. One field rather than
    /// one per dialog: five places gate on "is a modal up", and they must
    /// never disagree about the answer.
    modal: Option<Modal>,
    /// Tab ids that asked to close while a prompt was already showing
    close_queue: Vec<u64>,
    /// Tab ids whose save-conflict prompt is waiting for the modal to free up
    conflict_queue: Vec<u64>,
    file_dialog_active: bool,
    /// Whether files are currently being dragged over the window - drives the
    /// drop overlay in `view`.
    files_hovered: bool,
    /// Each tab's find palette, keyed by `Tab::id` rather than living on
    /// `Tab` - see `find.rs`. An entry is dropped when its tab closes.
    find: HashMap<u64, FindState>,
    /// Live keyboard modifier state - iced's mouse events carry no
    /// modifiers, so shift+click handling reads it from here.
    modifiers: keyboard::Modifiers,
    /// The config in effect - the baseline `apply_config` diffs a reloaded
    /// file against.
    config: jumppad_config::Config,
    /// Same, for `apply_keybinds`.
    keybinds: jumppad_config::KeybindsConfig,
    /// Editor settings shared with every open tab's `TextArea` - the handle
    /// a config reload writes through.
    editor_config: Arc<jumppad_textarea::SharedEditorConfig>,
    /// Whether the window was created transparent. Fixed at startup, so a
    /// reloaded `alpha.background` can't cross it - see `apply_config`.
    window_transparent: bool,
    config_watch: reload::ConfigWatch,
    document_watch: docwatch::DocumentWatch,
}

/// The modal dialogs, both three-button and both keyboard-navigable the
/// same way - see `Modal::focus_mut` and `Modal::resolve`.
enum Modal {
    Close(PendingClose),
    SaveConflict(PendingConflict),
}

impl Modal {
    /// The dialog's focused-choice index, for the shared arrow/Tab handling.
    fn focus_mut(&mut self) -> &mut usize {
        match self {
            Modal::Close(pending) => &mut pending.focused,
            Modal::SaveConflict(pending) => &mut pending.focused,
        }
    }

    /// The message Enter/Space produces for whichever choice is focused.
    fn resolve(&self) -> Message {
        match self {
            Modal::Close(pending) => {
                let decision = match pending.focused {
                    0 => CloseDecision::Save,
                    1 => CloseDecision::DontSave,
                    _ => CloseDecision::Cancel,
                };
                Message::CloseConfirmed(pending.tab_id, decision)
            }
            Modal::SaveConflict(pending) => {
                let decision = match pending.focused {
                    0 => ConflictDecision::Overwrite,
                    1 => ConflictDecision::DiscardAndReload,
                    _ => ConflictDecision::Cancel,
                };
                Message::ConflictResolved(pending.tab_id, decision)
            }
        }
    }

    /// The message Escape produces - always the dialog's cancel.
    fn cancel(&self) -> Message {
        match self {
            Modal::Close(pending) => {
                Message::CloseConfirmed(pending.tab_id, CloseDecision::Cancel)
            }
            Modal::SaveConflict(pending) => {
                Message::ConflictResolved(pending.tab_id, ConflictDecision::Cancel)
            }
        }
    }
}

/// State for the unsaved-changes modal - `focused` indexes the three
/// choices left-to-right (0=Save, 1=Don't Save, 2=Cancel).
struct PendingClose {
    tab_id: u64,
    title: String,
    focused: usize,
}

/// State for the save-conflict modal - `focused` indexes the three choices
/// left-to-right (0=Overwrite, 1=Discard & Reload, 2=Cancel).
struct PendingConflict {
    tab_id: u64,
    title: String,
    focused: usize,
}

#[derive(Debug, Clone)]
pub enum Message {
    NewTab,
    OpenFile,
    FileOpened(Result<(PathBuf, Arc<String>), OpenError>),
    /// Files entered (`true`) or left (`false`) the window mid-drag.
    FilesHovered(bool),
    /// One file was dropped onto the window. A multi-file drop arrives as one
    /// of these per file.
    FileDropped(PathBuf),
    /// A dropped file finished reading. Separate from `FileOpened` so the drop
    /// path never touches `file_dialog_active` - a drop landing while a save
    /// dialog is open must not clear that flag.
    DroppedFileRead(Result<(PathBuf, Arc<String>), OpenError>),
    /// Files named on the command line, opened in the order given.
    OpenPaths(Vec<PathBuf>),
    SaveFile,
    SaveFileAs,
    /// A save finished, carrying the path written and the stamp of what was
    /// written - see `save_to` on why the stamp is taken inside the task.
    FileSaved(u64, Result<(PathBuf, Option<DiskStamp>), SaveError>),
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
    /// The save-conflict prompt for tab `.0` came back with the user's choice.
    ConflictResolved(u64, ConflictDecision),
    /// The user asked for the on-disk version of tab `.0`, from the conflict
    /// dialog or the banner - here are its contents.
    ConflictReloaded(u64, PathBuf, Option<DiskStamp>, Result<Arc<String>, std::io::ErrorKind>),
    /// "Reload" on the changed-on-disk bar - the dialog's Discard & Reload,
    /// reachable without a save first.
    ReloadFromDisk(u64),
    /// "Keep mine" on the changed-on-disk bar: acknowledge the change without
    /// touching the buffer.
    AcknowledgeExternalChange(u64),
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
    /// One presented frame elapsed - counts down `surface_reset_frames`,
    /// resetting the Windows redirection surface when it hits zero.
    SurfaceResetFrame,
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
    /// Show the find palette for the active tab, or refocus it if already open.
    OpenFind,
    /// The find query changed - re-searches and jumps to the first match.
    FindQueryChanged(String),
    /// Select the next match, wrapping past the last.
    FindNext,
    /// Select the previous match, wrapping past the first.
    FindPrevious,
    /// Hide the find palette, keeping its query for next time.
    CloseFind,
    /// A raw key press, resolved into a command by `handle_hotkey`. Kept as
    /// its own variant since `Subscription::filter_map`'s closure can't
    /// capture `self.keybind_overrides`.
    KeyPressed(keyboard::Key, keyboard::Modifiers, key::Physical),
    /// The keyboard modifier state changed - keeps `JumpPadApp::modifiers`
    /// current between key presses.
    ModifiersChanged(keyboard::Modifiers),
    /// The window regained focus - checks the config files for changes made
    /// while it was away.
    WindowFocused,
    /// The OS file watcher saw activity on a config file.
    ConfigFileEvent(reload::WatchedFile),
    /// Periodic while a config-reload burst is pending - applies the
    /// debounced reload once the files stop changing.
    ConfigSettleTick,
    /// The OS file watcher saw activity in a directory holding an open file.
    DocumentFileEvent,
    /// Periodic while a document-change burst is pending.
    DocumentSettleTick,
    /// A tab's file finished re-reading after an external change. Carries the
    /// path it was read from - a Save As can retarget the tab mid-read - and
    /// the stamp the read was taken against.
    DocumentReloaded(
        u64,
        PathBuf,
        Option<DiskStamp>,
        Result<Arc<String>, std::io::ErrorKind>,
    ),
}

/// The three choices offered by the unsaved-changes prompt (see
/// `request_close` and `confirm_close`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    Save,
    DontSave,
    Cancel,
}

/// What a save expects to find on disk, and so whether it checks at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveExpectation {
    /// Write regardless - a Save As target, whose dialog asks its own
    /// overwrite question, or `save_conflict_resolution = "overwrite"`.
    Unchecked,
    /// The stamp the tab last saw. `None` means it saw no file at all.
    Seen(Option<DiskStamp>),
}

/// The three choices offered when a save would overwrite a file that changed
/// on disk. No Compare: JumpPad has no diff view (see AGENTS.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictDecision {
    /// Save anyway, skipping the check.
    Overwrite,
    /// Throw the buffer away and take what's on disk.
    DiscardAndReload,
    /// Leave both sides alone; the tab stays dirty and flagged.
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
    /// The file changed on disk since the tab last saw it - nothing written.
    /// No path: the tab it belongs to is what the prompt names.
    Conflict,
}

impl JumpPadApp {
    /// Takes the already-loaded config rather than loading it itself: `run()`
    /// (in `lib.rs`) loads it before the iced runtime starts, which is
    /// before this constructor ever runs.
    pub fn new(config: jumppad_config::Config, paths: Vec<PathBuf>) -> (Self, Task<Message>) {
        // Deferred to a message rather than opened here, so the paths land
        // after `next_id` has been settled by whatever the session restored.
        let argv_task = if paths.is_empty() {
            Task::none()
        } else {
            Task::done(Message::OpenPaths(paths))
        };
        let search_dirs = default_search_dirs();
        log_wasm_files_found(&search_dirs);
        // No push-based wake-up needed (unlike egui's `ctx.request_repaint()`) -
        // the highlighting-poll subscription below re-checks periodically instead.
        let keybinds = jumppad_config::load_keybinds();
        let keybind_overrides = Arc::new(build_key_overrides(&keybinds));
        warn_unrecognized_overrides(&keybinds.overrides);

        let editor_config = jumppad_textarea::SharedEditorConfig::new(
            config.alpha.background,
            build_key_resolver(keybind_overrides.clone()),
        );
        editor_config.set_foreground_alpha(config.alpha.foreground);
        editor_config.set_scroll_sensitivity(config.scroll.sensitivity);
        editor_config.set_comment_styles(build_comment_styles(&config));

        let registry = syntax_registry::SyntaxRegistry::new(
            search_dirs,
            config.extension_to_grammar(),
            || {},
        );
        // Which `TextEditorWidget` implementation new tabs are created with.
        let editor_factory: EditorFactory = Box::new(jumppad_textarea::TextArea::factory(
            registry,
            editor_config.clone(),
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
            surface_reset_frames: 0,
            session_dir,
            pending_close_after_save: Vec::new(),
            window: None,
            hotkey,
            visor_visible: false,
            animation: None,
            visor_enabled,
            previous_active_id: None,
            keybind_overrides,
            modal: None,
            close_queue: Vec::new(),
            conflict_queue: Vec::new(),
            file_dialog_active: false,
            files_hovered: false,
            find: HashMap::new(),
            modifiers: keyboard::Modifiers::default(),
            // The same expression `run()` derives the window's transparent
            // flag from - this field is what remembers the outcome.
            window_transparent: config.alpha.background < 1.0,
            editor_config,
            config_watch: reload::ConfigWatch::new(),
            document_watch: docwatch::DocumentWatch::new(),
            config,
            keybinds,
        };

        let window_task = iced::window::latest().map(Message::WindowReady);

        let Some(manifest) = manifest.filter(|manifest| !manifest.tabs.is_empty()) else {
            let task = app.new_tab();
            return (
                app,
                Task::batch([task, window_task, focus_editor(), argv_task]),
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
                        let mut tab = Tab::restored(
                            entry.id,
                            entry.path.clone(),
                            &content,
                            true,
                            &app.editor_factory,
                        );
                        // Whatever is on disk now is what this draft's next
                        // save is measured against - a file that moved while
                        // JumpPad was closed isn't a conflict to report at
                        // startup.
                        tab.restamp();
                        app.tabs.push(tab);
                    }
                    Err(_) if entry.path.is_some() => {
                        // Draft unreadable - fall back to a clean re-read of the real file.
                        let id = entry.id;
                        let path = entry
                            .path
                            .clone()
                            .expect("checked above: entry.path is Some");
                        let mut tab = Tab::restored(
                            id,
                            Some(path.clone()),
                            "",
                            false,
                            &app.editor_factory,
                        );
                        tab.restamp();
                        app.tabs.push(tab);
                        reload_tasks.push(Task::perform(reload_from_disk(path), move |result| {
                            Message::SessionFileLoaded(id, result)
                        }));
                    }
                    Err(_) => {
                        // No draft to recover - drop this entry.
                    }
                }
            } else if let Some(path) = &entry.path {
                let mut tab = Tab::restored(
                    entry.id,
                    Some(path.clone()),
                    "",
                    false,
                    &app.editor_factory,
                );
                // A file that isn't there is the ordinary state of a tab named
                // on the command line and never saved - restore it as the empty
                // buffer it was, rather than reading it and reporting an error.
                // Stamped before the read is even queued, not just when it
                // lands: a focus sweep arriving first would otherwise see a
                // clean, stamp-less tab and reload the file over the empty
                // buffer this tab starts with.
                tab.restamp();
                if path.exists() {
                    let id = entry.id;
                    reload_tasks.push(Task::perform(reload_from_disk(path.clone()), move |result| {
                        Message::SessionFileLoaded(id, result)
                    }));
                }
                app.tabs.push(tab);
            } else {
                app.tabs.push(Tab::untitled(entry.id, &app.editor_factory));
            }
        }

        if app.tabs.is_empty() {
            let task = app.new_tab();
            return (
                app,
                Task::batch([task, window_task, focus_editor(), argv_task]),
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
            focus_editor()
        } else {
            Task::none()
        };
        let task = Task::batch(
            reload_tasks
                .into_iter()
                .chain([switch_task, focus_task, window_task, argv_task]),
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
        focus_editor()
    }

    /// Opens each path named on the command line, in order, leaving the last
    /// one active.
    ///
    /// Reads synchronously, the way restored drafts already do: these are the
    /// files the user is waiting on, so blocking beats a frame of latency, and
    /// concurrent reads wouldn't preserve the order they were named in.
    fn open_paths(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        let mut tasks = Vec::new();
        let mut problems = Vec::new();
        for path in paths {
            if let Some(index) = self.tab_index_for(&path) {
                tasks.push(self.switch_active(index));
                continue;
            }
            if path.is_dir() {
                problems.push(format!("{}: is a folder", path.display()));
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(contents) => tasks.push(self.open_loaded_file(path, &contents)),
                // Naming a file that isn't there yet is how you start one -
                // an empty tab, saved to that path on the first save.
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    tasks.push(self.open_loaded_file(path, ""))
                }
                Err(err) => problems.push(format!("{}: {}", path.display(), err.kind())),
            }
        }
        // One row, so several bad paths have to share it.
        if !problems.is_empty() {
            self.error = Some(format!("Couldn't open {}", problems.join("; ")));
        }
        Task::batch(tasks)
    }

    /// The tab with this id, but only while it still holds `path` - an async
    /// read that landed after a Save As retargeted the tab belongs to a file
    /// the tab no longer shows.
    fn tab_index_holding(&self, id: u64, path: &Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.id == id && tab.document.path.as_deref() == Some(path))
    }

    /// The tab already showing `path`, if one is open.
    fn tab_index_for(&self, path: &Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.document.path.as_deref() == Some(path))
    }

    /// Opens an already-read file, loading it into the active tab when that tab
    /// is an untouched scratch tab rather than leaving a stray "Untitled"
    /// behind. Shared by dropped files and files named on the command line.
    fn open_loaded_file(&mut self, path: PathBuf, contents: &str) -> Task<Message> {
        let scratch = self.tabs.get(self.active).is_some_and(|tab| {
            tab.document.path.is_none() && !tab.dirty && tab.editor.text().is_empty()
        });
        if !scratch {
            let id = self.next_id;
            self.next_id += 1;
            let mut tab = Tab::from_file(id, path, contents, &self.editor_factory);
            tab.restamp();
            self.tabs.push(tab);
            return self.switch_active(self.tabs.len() - 1);
        }

        // Rebuilt rather than `set_text`: the factory takes the extension, and
        // that's what picks the grammar - an editor made for an untitled buffer
        // would stay unhighlighted. The id carries over, since the session
        // manifest and `find` are keyed by it.
        let id = self.tabs[self.active].id;
        self.tabs[self.active] = Tab::from_file(id, path, contents, &self.editor_factory);
        self.tabs[self.active].restamp();
        // The index didn't move, so `switch_active` would call it a no-op -
        // the same situation `new_tab` handles above.
        self.redraw_nudge_frames = REDRAW_NUDGE_FRAMES;
        self.arm_shadow_refresh();
        self.sync_session_metadata();
        self.refresh_find();
        focus_editor()
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
        if self.modal.is_some() {
            if !self.close_queue.contains(&id) {
                self.close_queue.push(id);
            }
            return Task::none();
        }
        self.modal = Some(Modal::Close(PendingClose {
            tab_id: id,
            title: tab.document.display_name(),
            focused: 0,
        }));
        // Blur the editor so modal-navigation keystrokes don't also type
        // into the hidden document.
        operate(operation::focusable::unfocus())
    }

    /// Shows whatever dialog was waiting behind the one just dismissed -
    /// close prompts first, since they're the ones blocking a quit. With
    /// nothing queued the modal is gone, so keyboard focus goes back to the
    /// editor.
    fn show_next_modal(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        // Keeps draining until something actually puts a dialog up: a queued
        // tab can have been closed, or gone clean since - and a clean one
        // `request_close` just closes, which must not strand what's behind it.
        while self.modal.is_none() {
            if !self.close_queue.is_empty() {
                let next_id = self.close_queue.remove(0);
                if let Some(index) = self.tabs.iter().position(|tab| tab.id == next_id) {
                    tasks.push(self.request_close(index));
                }
                continue;
            }
            if !self.conflict_queue.is_empty() {
                let next_id = self.conflict_queue.remove(0);
                if self.tabs.iter().any(|tab| tab.id == next_id) {
                    tasks.push(self.open_conflict_prompt(next_id));
                }
                continue;
            }
            break;
        }
        if self.modal.is_none() {
            // Nothing left to show, so the keyboard goes back to the editor.
            tasks.push(focus_editor());
        }
        Task::batch(tasks)
    }

    /// Puts the save-conflict dialog up for `id`, or queues it behind a
    /// dialog that's already showing.
    fn open_conflict_prompt(&mut self, id: u64) -> Task<Message> {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return Task::none();
        };
        if self.modal.is_some() {
            if !self.conflict_queue.contains(&id) {
                self.conflict_queue.push(id);
            }
            return Task::none();
        }
        self.modal = Some(Modal::SaveConflict(PendingConflict {
            tab_id: id,
            title: tab.document.display_name(),
            // Cancel, not Overwrite: both of the other choices destroy
            // someone's work, and a reflexive Enter shouldn't pick either.
            // (The close prompt can default to its first choice because
            // "Save" is the safe one there.)
            focused: 2,
        }));
        // Blur the editor so modal-navigation keystrokes don't also type
        // into the hidden document.
        operate(operation::focusable::unfocus())
    }

    /// Re-runs a save that lost the conflict check, with no expectation this
    /// time - the user has said to overwrite.
    fn save_tab_forcing_overwrite(&mut self, id: u64) -> Task<Message> {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return Task::none();
        };
        let Some(path) = tab.document.path.clone() else {
            return Task::none();
        };
        let text = tab.editor.text();
        // No expectation this time - the user has said to overwrite.
        Task::perform(
            save_to(Some(path), text, false, SaveExpectation::Unchecked),
            move |result| Message::FileSaved(id, result),
        )
    }

    /// Throws a tab's unsaved buffer away and takes what's on disk.
    fn reload_from_conflict(&mut self, id: u64) -> Task<Message> {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return Task::none();
        };
        let Some(path) = tab.document.path.clone() else {
            return Task::none();
        };
        let stamp = DiskStamp::of(&path);
        Task::perform(reload_from_disk(path.clone()), move |result| {
            Message::ConflictReloaded(id, path.clone(), stamp, result)
        })
    }

    fn close_tab(&mut self, index: usize) -> Task<Message> {
        if index >= self.tabs.len() {
            return Task::none();
        }
        let closed = self.tabs.remove(index);
        self.find.remove(&closed.id);
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

    /// Whether the active tab's find palette is currently showing.
    fn find_is_open(&self) -> bool {
        self.tabs
            .get(self.active)
            .and_then(|tab| self.find.get(&tab.id))
            .is_some_and(|state| state.open)
    }

    /// Re-points the counter at whichever match the cursor now touches,
    /// without re-searching - for cursor moves that changed no text.
    fn sync_find_counter(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(state) = self.find.get_mut(&tab.id) else {
            return;
        };
        if !state.open {
            return;
        }
        let cursor = tab.editor.cursor_position();
        if let Some(touched) = state.index_at(cursor) {
            state.current = Some(touched);
            tab.editor.set_find_matches(state.matches.clone(), state.current);
        }
    }

    /// Re-runs the active tab's search against its current text and pushes
    /// the result to its editor for coloring. Called whenever either side
    /// can have moved: the query, the document, or which tab is active.
    fn refresh_find(&mut self) {
        self.refresh_find_for(self.active);
    }

    /// Same, for a tab that isn't the active one - a background tab whose
    /// buffer was replaced by an external reload still holds ranges into the
    /// text that just went away.
    fn refresh_find_for(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(state) = self.find.get_mut(&tab.id) else {
            return;
        };
        if state.open {
            state.search(&tab.editor.text());
            tab.editor.set_find_matches(state.matches.clone(), state.current);
        } else {
            tab.editor.set_find_matches(Vec::new(), None);
        }
    }

    /// Selects the active tab's current match in the editor, and repaints
    /// the match coloring so the newly-current one stands out.
    fn select_current_match(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(state) = self.find.get(&tab.id) else {
            return;
        };
        // Tint only while the palette is showing. With it closed - a bare
        // find-again - the editor holds focus, so the ordinary selection
        // marks the match on its own.
        if state.open {
            tab.editor.set_find_matches(state.matches.clone(), state.current);
        } else {
            tab.editor.set_find_matches(Vec::new(), None);
        }
        let Some(found) = state.current_match() else {
            return;
        };
        // Cursor at the end of the match so typing continues past it, the
        // same shape a drag-selection leaves. `move_to` underneath marks the
        // cursor moved, which scrolls an off-screen match into view.
        tab.editor.restore_selection(
            SavedSelection {
                anchor: (found.line, found.start),
                kind: SelectionKind::Range,
            },
            (found.line, found.end),
        );
    }

    fn switch_active(&mut self, index: usize) -> Task<Message> {
        if index >= self.tabs.len() || index == self.active {
            return Task::none();
        }
        if let Some(previous) = self.tabs.get_mut(self.active) {
            previous.last_cursor = previous.editor.cursor_position();
            previous.last_selection = previous.editor.selection();
            self.previous_active_id = Some(previous.id);
        }
        self.active = index;
        self.redraw_nudge_frames = REDRAW_NUDGE_FRAMES;
        if let Some(tab) = self.tabs.get_mut(index) {
            let (line, column) = tab.last_cursor;
            match tab.last_selection {
                Some(selection) => tab.editor.restore_selection(selection, tab.last_cursor),
                None => tab.editor.move_cursor_to(line, column),
            }
        }
        self.sync_session_metadata();
        self.arm_shadow_refresh();
        // The incoming tab's document may have changed since its palette
        // last searched, and its editor widget state started fresh.
        self.refresh_find();
        focus_editor()
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

    /// Drops the Windows 11 system backdrop from behind a translucent window
    /// (see `windows.rs`). Runs once, as soon as the window exists - winit has
    /// already set `DWMSBT_AUTO` by then, so this is strictly an override.
    /// Skipped on a solid window, where the backdrop is hidden anyway and
    /// turning it off would be a gratuitous difference from every other app.
    #[cfg(target_os = "windows")]
    fn disable_system_backdrop(&self) -> Task<Message> {
        match self.window {
            Some(id) if self.background_alpha < 1.0 => iced::window::run(id, |window| {
                crate::windows::disable_system_backdrop(window);
            })
            .discard(),
            _ => Task::none(),
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn disable_system_backdrop(&self) -> Task<Message> {
        Task::none()
    }

    /// Arms the one-shot redirection-surface reset (see `windows.rs`). Only
    /// on a translucent Windows window: elsewhere there is nothing to fix, and
    /// on a solid window the surface's alpha is never read.
    fn arm_surface_reset(&mut self) {
        if cfg!(target_os = "windows") && self.background_alpha < 1.0 {
            self.surface_reset_frames = SURFACE_RESET_FRAMES;
        }
    }

    #[cfg(target_os = "windows")]
    fn reset_redirection_surface(&self) -> Task<Message> {
        match self.window {
            Some(id) => iced::window::run(id, |window| {
                crate::windows::reset_redirection_surface(window);
            })
            .discard(),
            None => Task::none(),
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn reset_redirection_surface(&self) -> Task<Message> {
        Task::none()
    }

    /// Rewrites the session manifest, pruning orphaned draft files.
    fn sync_session_metadata(&self) {
        let manifest = session::build_manifest(&self.tabs, self.active);
        session::write_manifest_sync(&self.session_dir, &manifest);
    }

    /// Reloads whichever config files settled out of a change burst. A file
    /// that no longer parses keeps the current in-memory settings and says
    /// so in the error banner - a save mid-edit must not reset anything.
    fn reload_settled_configs(&mut self) {
        for file in self.config_watch.settled(Instant::now()) {
            let result = match file {
                reload::WatchedFile::Config => {
                    jumppad_config::try_load().map(|config| self.apply_config(config))
                }
                reload::WatchedFile::Keybinds => {
                    jumppad_config::try_load_keybinds()
                        .map(|keybinds| self.apply_keybinds(keybinds))
                }
            };
            if let Err(err) = result {
                eprintln!("jumppad: couldn't reload {}: {err}", file.name());
                self.error = Some(format!(
                    "{}: {err} - keeping the previous settings",
                    file.name()
                ));
            }
        }
    }

    /// Every open file-backed tab's path, sorted and deduped - the watcher's
    /// identity, so an unstable order would restart it on every tab switch.
    fn watched_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .tabs
            .iter()
            .filter_map(|tab| tab.document.path.clone())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    /// Re-reads whatever the sweep decided had changed underneath a clean tab.
    fn sweep_documents(&mut self) -> Task<Message> {
        let reads = self.resolve_disk_changes().into_iter().map(|(id, path, stamp)| {
            Task::perform(reload_from_disk(path.clone()), move |result| {
                Message::DocumentReloaded(id, path.clone(), Some(stamp), result)
            })
        });
        Task::batch(reads)
    }

    /// Stats every file-backed tab and applies the external-change rules:
    /// a clean tab reloads silently, a dirty one is flagged and left alone,
    /// and a file that disappeared leaves its tab open and dirty so the next
    /// save recreates it. Returns the tabs needing a re-read, as
    /// `(id, path, stamp)`.
    ///
    /// Nothing here matches event paths - the sweep re-derives what moved
    /// from `stat`, so a spurious poke costs one no-op pass.
    fn resolve_disk_changes(&mut self) -> Vec<(u64, PathBuf, DiskStamp)> {
        let mut reads = Vec::new();
        let mut deleted_any = false;
        // Every tab whose path matches, not just the first: `tab_index_for`
        // stops the same file being opened twice, but Save As can still
        // leave two tabs pointing at one path.
        for tab in &mut self.tabs {
            let Some(path) = tab.document.path.clone() else {
                continue;
            };
            let current = DiskStamp::of(&path);
            if current == tab.disk {
                // Nothing moved. This is also what makes JumpPad's own save
                // a no-op here: the save task stamped what it wrote.
                continue;
            }
            let Some(current) = current else {
                // Deleted or renamed away. Keep the tab and its content -
                // the next save recreates the file - and no prompt, the way
                // VS Code leaves the editor open.
                tab.disk = None;
                tab.dirty = true;
                tab.draft_generation += 1;
                deleted_any = true;
                continue;
            };
            if tab.dirty {
                // Unsaved edits win: the buffer is untouched and the
                // conflict surfaces at save time. `tab.disk` deliberately
                // keeps its old value - that's what the save-time check
                // compares against.
                tab.externally_changed = true;
                continue;
            }
            // Clean: reload silently. Stamped on arrival, not here, so a
            // change landing during the read isn't recorded as seen.
            reads.push((tab.id, path, current));
        }
        if deleted_any {
            // Newly dirty tabs need draft files, so the manifest has to say so.
            self.sync_session_metadata();
        }
        reads
    }

    /// Applies a freshly reloaded `config.toml`, diffing against the one in
    /// effect. The single wiring point for live config: a new reloadable
    /// setting gets its arm here, and a setting that can only apply at
    /// startup gets a `restart_required` line instead.
    fn apply_config(&mut self, new: jumppad_config::Config) {
        let current = &self.config;
        let mut repaint = false;

        if new.theme != current.theme {
            self.theme = resolve_theme(&new.theme);
            repaint = true;
        }

        if new.alpha.foreground != current.alpha.foreground {
            self.editor_config.set_foreground_alpha(new.alpha.foreground);
            repaint = true;
        }

        if new.alpha.background != current.alpha.background {
            if self.window_transparent {
                self.background_alpha = new.alpha.background.clamp(0.0, 1.0);
                self.editor_config.set_background_alpha(new.alpha.background);
                repaint = true;
            } else if new.alpha.background < 1.0 {
                // Applying anyway would darken the window without revealing
                // the desktop: an opaque surface presents `rgb * a` as-is.
                restart_required("[alpha] background on a window that started opaque");
            }
        }

        // No repaint: nothing on screen changes until the next wheel event,
        // and the widget reads the new value on the `view` that one causes.
        if new.scroll.sensitivity != current.scroll.sensitivity {
            self.editor_config
                .set_scroll_sensitivity(new.scroll.sensitivity);
        }

        // One [[languages]] edit can feed two consumers, so diff the derived
        // views: comment styles apply live (no repaint - nothing on screen
        // changes until the next toggle), grammar mappings can't.
        if new.comment_styles_by_extension() != current.comment_styles_by_extension() {
            self.editor_config.set_comment_styles(build_comment_styles(&new));
        }

        if new.window != current.window {
            restart_required("[window] decorations");
        }
        if new.visor.enabled != current.visor.enabled {
            restart_required("[visor] enabled");
        }
        if new.extension_to_grammar() != current.extension_to_grammar() {
            restart_required("[[languages]] extension-to-syntax mappings");
        }

        if repaint {
            // tiny-skia's damage tracking can otherwise skip presenting a
            // change that moved no widget - an alpha-only reload, say.
            self.redraw_nudge_frames = REDRAW_NUDGE_FRAMES;
        }
        self.config = new;
    }

    /// Mirror of `apply_config` for `keybinds.toml`. The override tables are
    /// rebuilt wholesale - they're small - so added, changed, and removed
    /// binds all land in one pass.
    fn apply_keybinds(&mut self, new: jumppad_config::KeybindsConfig) {
        self.keybind_overrides = Arc::new(build_key_overrides(&new));
        self.editor_config
            .set_resolver(build_key_resolver(self.keybind_overrides.clone()));
        warn_unrecognized_overrides(&new.overrides);

        // Re-registered only on an actual change: dropping the old
        // registration releases the chord to other apps, however briefly.
        if self.visor_enabled && new.toggle != self.keybinds.toggle {
            self.hotkey = None;
            self.hotkey = Hotkey::register(new.toggle);
        }
        self.keybinds = new;
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
        let expected = self.save_expectation(tab, shows_dialog);
        Task::perform(
            save_to(existing_path, text, force_dialog, expected),
            move |result| Message::FileSaved(id, result),
        )
    }

    /// What an about-to-run save should expect to find on disk. Only an
    /// in-place write of a known file is checked: a Save As target is the
    /// user picking a file in a dialog that asks its own overwrite question.
    ///
    /// Read from `self.config` here rather than applied through
    /// `apply_config`, so a reloaded `config.toml` takes effect on the next
    /// save for free - the one live setting with no `apply_config` arm.
    fn save_expectation(&self, tab: &Tab, shows_dialog: bool) -> SaveExpectation {
        if shows_dialog || !self.config.files.save_conflict_resolution.asks() {
            SaveExpectation::Unchecked
        } else {
            SaveExpectation::Seen(tab.disk)
        }
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
                let mut tab = Tab::from_file(id, path, &contents, &self.editor_factory);
                tab.restamp();
                self.tabs.push(tab);
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
            Message::FilesHovered(hovered) => {
                self.files_hovered = hovered;
                Task::none()
            }
            Message::FileDropped(path) => {
                // A completed drop emits no `FilesHoveredLeft` on Windows or
                // macOS, so this is what actually dismisses the overlay.
                self.files_hovered = false;
                // Window events reach the app past the modal's scrim, which
                // only blocks clicks - so the prompt has to turn a drop away
                // itself, the way it does keystrokes.
                if self.modal.is_some() {
                    return Task::none();
                }
                // Already open - focus that tab rather than opening a second
                // copy of the same file.
                if let Some(index) = self.tab_index_for(&path) {
                    return self.switch_active(index);
                }
                if path.is_dir() {
                    self.error = Some(format!("Can't open {}: it's a folder", path.display()));
                    return Task::none();
                }
                Task::perform(read_path(path), Message::DroppedFileRead)
            }
            Message::DroppedFileRead(Ok((path, contents))) => {
                self.open_loaded_file(path, &contents)
            }
            Message::DroppedFileRead(Err(OpenError::Io { path, kind })) => {
                self.error = Some(format!("Couldn't open {}: {kind}", path.display()));
                Task::none()
            }
            // No dialog is involved in a drop.
            Message::DroppedFileRead(Err(OpenError::DialogClosed)) => Task::none(),
            Message::OpenPaths(paths) => self.open_paths(paths),
            Message::SaveFile => self.save_active_tab(false),
            Message::SaveFileAs => self.save_active_tab(true),
            Message::FileSaved(id, Ok((path, stamp))) => {
                self.file_dialog_active = false;
                self.config_watch.note_saved(&path, Instant::now());
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.document.path = Some(path);
                    tab.dirty = false;
                    tab.disk = stamp;
                    tab.externally_changed = false;
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
            Message::FileSaved(id, Err(SaveError::Conflict)) => {
                // `file_dialog_active` is deliberately left alone: only a
                // dialog-less save is ever checked (see `save_expectation`),
                // so clearing it here could dismiss another tab's open dialog.
                //
                // A conflict aborts the close, the same way VS Code leaves
                // the editor open when its save doesn't go through.
                self.pending_close_after_save.retain(|&pending_id| pending_id != id);
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    // The sweep may not have run yet - the save is how this
                    // one found out.
                    tab.externally_changed = true;
                }
                self.open_conflict_prompt(id)
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
                // Whichever modal is up intercepts all keystrokes: cycle the
                // focused choice mod 3, then resolve it against that dialog.
                if let Some(modal) = &mut self.modal {
                    let focused = modal.focus_mut();
                    match key {
                        keyboard::Key::Named(key::Named::ArrowLeft) => {
                            *focused = (*focused + 2) % 3;
                            return Task::none();
                        }
                        keyboard::Key::Named(key::Named::ArrowRight) => {
                            *focused = (*focused + 1) % 3;
                            return Task::none();
                        }
                        keyboard::Key::Named(key::Named::Tab) => {
                            *focused = if modifiers.shift() {
                                (*focused + 2) % 3
                            } else {
                                (*focused + 1) % 3
                            };
                            return Task::none();
                        }
                        keyboard::Key::Named(key::Named::Enter)
                        | keyboard::Key::Named(key::Named::Space) => {
                            let resolved = modal.resolve();
                            return self.update(resolved);
                        }
                        keyboard::Key::Named(key::Named::Escape) => {
                            let cancel = modal.cancel();
                            return self.update(cancel);
                        }
                        _ => return Task::none(),
                    }
                }
                match handle_hotkey(key, modifiers, physical_key, &self.keybind_overrides) {
                    Some(resolved) => self.update(resolved),
                    None => Task::none(),
                }
            }
            Message::OpenFind => {
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let origin = tab.editor.cursor_position();
                let state = self.find.entry(tab.id).or_default();
                // Reopening re-anchors to wherever the cursor is now, so the
                // next search runs from there rather than from wherever the
                // palette was last used.
                state.origin = origin;
                state.open = true;
                self.refresh_find();
                self.select_current_match();
                // Already-open is not a no-op: Cmd+F should pull focus back
                // to the field from wherever it went.
                focus_find()
            }
            Message::FindQueryChanged(query) => {
                // On macOS, holding Cmd doesn't suppress character
                // production, and `text_input`'s insert branch has no
                // modifier guard - so Cmd+G types a "g" into the query on
                // its way to being a shortcut. Drop a single character that
                // appeared while `command()` was held. Mirrors the same
                // workaround `jumppad_textarea::key_binding` needs.
                //
                // Scoped to one-character growth so a Cmd+V paste, which
                // legitimately arrives with `command()` held, still lands.
                let leaked_shortcut_character = self.modifiers.command()
                    && self
                        .active_find()
                        .is_some_and(|state| query.len() == state.query.len() + 1);
                if leaked_shortcut_character {
                    return Task::none();
                }
                if let Some(tab) = self.tabs.get(self.active) {
                    let state = self.find.entry(tab.id).or_default();
                    state.query = query;
                    self.refresh_find();
                    self.select_current_match();
                }
                Task::none()
            }
            Message::FindNext | Message::FindPrevious => {
                let delta = if matches!(message, Message::FindNext) { 1 } else { -1 };
                let Some(tab) = self.tabs.get(self.active) else {
                    return Task::none();
                };
                let (tab_id, text, cursor) =
                    (tab.id, tab.editor.text(), tab.editor.cursor_position());
                let Some(state) = self.find.get_mut(&tab_id) else {
                    return Task::none();
                };
                if state.query.is_empty() {
                    return Task::none();
                }
                if !state.open {
                    // Find-again with the palette closed (Cmd+G): the
                    // document may have changed since it last searched, and
                    // stepping should continue from the cursor rather than
                    // from wherever the palette was left.
                    state.matches = editor_core::find_matches(&text, &state.query);
                    state.origin = cursor;
                    state.current = state.index_at(cursor);
                }
                state.step(delta);
                self.select_current_match();
                // Focus is left alone: in the field so Enter keeps stepping,
                // or in the editor for a palette-closed find-again.
                Task::none()
            }
            Message::CloseFind => {
                // A modal owns Escape while it is up.
                if self.modal.is_some() || !self.find_is_open() {
                    return Task::none();
                }
                if let Some(tab) = self.tabs.get(self.active) {
                    if let Some(state) = self.find.get_mut(&tab.id) {
                        // Closed, not cleared - the query is there next time.
                        state.open = false;
                    }
                }
                self.refresh_find();
                // Non-optional: without it the editor stays unfocused and
                // typing goes nowhere (see AGENTS.md).
                focus_editor()
            }
            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            }
            Message::WindowFocused => {
                self.config_watch.check(Instant::now());
                // A focus gain is already a settled moment - the safety net
                // for changes the watcher never delivered, no debounce needed.
                self.sweep_documents()
            }
            Message::ConfigFileEvent(file) => {
                self.config_watch.note_event(file, Instant::now());
                Task::none()
            }
            Message::ConfigSettleTick => {
                self.reload_settled_configs();
                Task::none()
            }
            Message::DocumentFileEvent => {
                self.document_watch.note_event(Instant::now());
                Task::none()
            }
            Message::DocumentSettleTick => {
                if self.document_watch.settled(Instant::now()) {
                    self.sweep_documents()
                } else {
                    Task::none()
                }
            }
            Message::DocumentReloaded(id, path, stamp, result) => {
                // Re-validated against the tab as it stands *now*: the read
                // was async, and the tab may have been closed, saved
                // elsewhere, or typed into while it was in flight.
                let Some(index) = self.tab_index_holding(id, &path) else {
                    return Task::none();
                };
                let tab = &mut self.tabs[index];
                if tab.dirty {
                    // The user typed while the read was in flight. Applying
                    // it would destroy those edits, which is the one thing
                    // this feature must never do.
                    tab.externally_changed = true;
                    return Task::none();
                }
                let contents = match result {
                    Ok(contents) => contents,
                    // Vanished between the sweep and the read - the delete
                    // case, not an error worth a banner.
                    Err(std::io::ErrorKind::NotFound) => {
                        tab.disk = None;
                        tab.dirty = true;
                        tab.draft_generation += 1;
                        self.sync_session_metadata();
                        return Task::none();
                    }
                    // Anything else (a permission flip, a file replaced with
                    // something that isn't UTF-8) is a read that failed, not
                    // a file that went away: leave the tab exactly as it is
                    // and let the next event try again.
                    Err(_) => return Task::none(),
                };
                tab.editor.reload_text(&contents);
                tab.disk = stamp;
                // The document moved under *this* tab's match list, which is
                // not necessarily the active one's.
                self.refresh_find_for(index);
                Task::none()
            }
            Message::DismissError => {
                self.error = None;
                Task::none()
            }
            Message::Editor(index, editor_message) => {
                // Belt and suspenders: the editor is blurred when the modal
                // opens, but that doesn't take effect until the next render.
                if self.modal.is_some() {
                    return Task::none();
                }
                let mut edited = false;
                if let Some(tab) = self.tabs.get_mut(index) {
                    let editor_message = editor_message.with_shift_click(self.modifiers.shift());
                    if tab.editor.update(editor_message) {
                        edited = true;
                        let just_became_dirty = !tab.dirty;
                        tab.dirty = true;
                        tab.draft_generation += 1;
                        // Only sync on the clean->dirty transition, not every keystroke.
                        if just_became_dirty {
                            self.sync_session_metadata();
                        }
                    }
                }
                if edited {
                    // The document moved under the match list.
                    self.refresh_find();
                } else {
                    // No edit, but the cursor may have moved (a click), and
                    // the counter reports whichever match it now touches.
                    self.sync_find_counter(index);
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
                    // The boot-time version of an external reload: the tab and
                    // its file agree again, so this is where the stamp is taken.
                    tab.restamp();
                }
                Task::none()
            }
            Message::SessionFileLoaded(_, Err(kind)) => {
                self.error = Some(format!("Couldn't reload a restored tab: {kind}"));
                Task::none()
            }
            Message::CloseConfirmed(id, decision) => {
                self.modal = None;
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
                let next_task = self.show_next_modal();
                Task::batch([decision_task, next_task])
            }
            Message::ConflictResolved(id, decision) => {
                self.modal = None;
                let decision_task = match decision {
                    // The tab stays dirty and flagged; nothing on disk moved.
                    ConflictDecision::Cancel => Task::none(),
                    // Re-run the save with no expectation, so the check the
                    // first attempt failed is skipped this time.
                    ConflictDecision::Overwrite => self.save_tab_forcing_overwrite(id),
                    ConflictDecision::DiscardAndReload => self.reload_from_conflict(id),
                };
                let next_task = self.show_next_modal();
                Task::batch([decision_task, next_task])
            }
            Message::ConflictReloaded(id, path, stamp, Ok(contents)) => {
                let Some(index) = self.tab_index_holding(id, &path) else {
                    return Task::none();
                };
                let tab = &mut self.tabs[index];
                // Asked for explicitly, so this one *does* replace a dirty
                // buffer - the whole point of "discard mine."
                tab.editor.reload_text(&contents);
                tab.dirty = false;
                tab.externally_changed = false;
                tab.disk = stamp;
                self.refresh_find_for(index);
                // Now clean - prune the draft file it no longer needs.
                self.sync_session_metadata();
                Task::none()
            }
            Message::ConflictReloaded(id, path, _stamp, Err(kind)) => {
                // Named by the path actually read, not by whatever the tab
                // shows now - a Save As can have moved it in between.
                if self.tab_index_holding(id, &path).is_some() {
                    self.error = Some(format!("Couldn't reload {}: {kind}", path.display()));
                }
                Task::none()
            }
            Message::ReloadFromDisk(id) => {
                // The bar sits under the modal's scrim, which only swallows
                // clicks its own widget sees - so, like `Editor` and
                // `FileDropped`, this has to turn itself away.
                if self.modal.is_some() {
                    return Task::none();
                }
                self.reload_from_conflict(id)
            }
            Message::AcknowledgeExternalChange(id) => {
                if self.modal.is_some() {
                    return Task::none();
                }
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.externally_changed = false;
                    // Re-stamping is the point: it records "I have seen this
                    // version", so the next save goes through unprompted.
                    tab.restamp();
                }
                Task::none()
            }
            Message::WindowReady(id) => {
                self.window = id;
                // The redirection-surface reset can't run yet - it has to
                // outlast the first presented frames (see `windows.rs`), so
                // it's armed here and fires from the frame countdown.
                self.arm_surface_reset();
                Task::batch([self.disable_system_backdrop(), self.snap_to_monitor()])
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
            Message::SurfaceResetFrame => {
                self.surface_reset_frames = self.surface_reset_frames.saturating_sub(1);
                if self.surface_reset_frames == 0 {
                    self.reset_redirection_surface()
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
            eprintln!("jumppad: couldn't determine the primary monitor's bounds");
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
            eprintln!("jumppad: couldn't determine the primary monitor's bounds");
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

    /// The active tab's find state, if it has any.
    fn active_find(&self) -> Option<&FindState> {
        self.find.get(&self.tabs.get(self.active)?.id)
    }

    /// The bar shown over the active tab when its file changed on disk while
    /// it had unsaved edits - VS Code shows one for the same reason, since
    /// the conflict otherwise stays invisible until the next save.
    fn changed_on_disk_bar(&self, tab_id: u64) -> Element<'_, Message> {
        let action = |label: &'static str, message: Message| {
            button(text(label).size(12).line_height(FIND_TEXT_LINE_HEIGHT))
                .padding([4, 8])
                .style(find_button_style)
                .on_press(message)
        };

        container(
            row![
                text("This file has changed on disk.")
                    .size(12)
                    .line_height(FIND_TEXT_LINE_HEIGHT),
                action("Reload", Message::ReloadFromDisk(tab_id)),
                action("Keep mine", Message::AcknowledgeExternalChange(tab_id)),
            ]
            .spacing(8)
            .align_y(Center),
        )
        .width(Fill)
        .padding(6)
        .style(find_palette_style)
        .into()
    }

    /// The find palette: query field, match counter, previous/next, close.
    fn find_palette(&self, state: &FindState) -> Element<'_, Message> {
        let query = text_input("Find", &state.query)
            .id(Id::new(FIND_INPUT_ID))
            .on_input(Message::FindQueryChanged)
            .on_submit(Message::FindNext)
            .padding([4, 8])
            .size(14)
            .width(Pixels(180.0))
            .style(find_input_style);

        let counter: Element<'_, Message> = match state.counter() {
            Some(label) => text(label).size(12).line_height(FIND_TEXT_LINE_HEIGHT).into(),
            None => text("").into(),
        };

        let step = |label: &'static str, message: Message| {
            button(text(label).size(12).line_height(FIND_TEXT_LINE_HEIGHT))
                .padding([4, 6])
                .style(find_button_style)
                .on_press(message)
        };

        container(
            row![
                query,
                counter,
                step("\u{2191}", Message::FindPrevious),
                step("\u{2193}", Message::FindNext),
                step("\u{2715}", Message::CloseFind),
            ]
            .spacing(6)
            .align_y(Center),
        )
        .padding(6)
        .style(find_palette_style)
        .into()
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

        // Composed before the find palette so the palette floats above the
        // bar rather than under it - the bar's controls sit at its left end,
        // where the top-right palette doesn't reach.
        let editor = match self.tabs.get(self.active).filter(|tab| tab.externally_changed) {
            Some(tab) => stack![
                editor,
                container(self.changed_on_disk_bar(tab.id))
                    .width(Fill)
                    .height(Fill)
                    .align_y(Top)
            ]
            .into(),
            None => editor,
        };

        // Floated over the editor rather than the whole window, so it never
        // covers the tab bar. `stack!` is the same overlay the modal uses.
        let editor = match self.active_find().filter(|state| state.open) {
            Some(state) => stack![
                editor,
                container(self.find_palette(state))
                    .width(Fill)
                    .height(Fill)
                    .align_x(Right)
                    .align_y(Top)
                    .padding(8)
            ]
            .into(),
            None => editor,
        };

        // Same overlay treatment, and for the same reason: the tab bar stays
        // visible and clickable underneath a drag. A plain container captures
        // no events, so the drop itself still lands.
        let editor = if self.files_hovered {
            stack![
                editor,
                container(center(text("Drop to open")))
                    .width(Fill)
                    .height(Fill)
                    .style(drop_overlay_style)
            ]
            .into()
        } else {
            editor
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

        let Some(modal) = &self.modal else {
            return content.into();
        };

        let dialog = match modal {
            Modal::Close(pending) => {
                // Wired up with `on_press` too, so a click works the same as
                // Enter/Space.
                let choice = |label: &'static str, index: usize, decision: CloseDecision| {
                    modal_choice(label, pending.focused == index)
                        .on_press(Message::CloseConfirmed(pending.tab_id, decision))
                };
                modal_dialog(
                    format!(
                        "Do you want to save the changes you made to {}?",
                        pending.title
                    ),
                    row![
                        choice("Save", 0, CloseDecision::Save),
                        choice("Don't Save", 1, CloseDecision::DontSave),
                        choice("Cancel", 2, CloseDecision::Cancel),
                    ],
                )
            }
            Modal::SaveConflict(pending) => {
                let choice = |label: &'static str, index: usize, decision: ConflictDecision| {
                    modal_choice(label, pending.focused == index)
                        .on_press(Message::ConflictResolved(pending.tab_id, decision))
                };
                modal_dialog(
                    format!("{} has changed on disk since you opened it.", pending.title),
                    row![
                        choice("Overwrite", 0, ConflictDecision::Overwrite),
                        choice("Discard & Reload", 1, ConflictDecision::DiscardAndReload),
                        choice("Cancel", 2, ConflictDecision::Cancel),
                    ],
                )
            }
        };

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
            if CLEAR_COLOR_NEEDS_PREMULTIPLY {
                style.background_color = premultiply(style.background_color);
            }
        }
        style
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            // Filters the unified keyboard-event stream down to presses and
            // modifier changes.
            keyboard::listen().filter_map(|event| match event {
                keyboard::Event::KeyPressed {
                    key,
                    modifiers,
                    physical_key,
                    ..
                } => Some(Message::KeyPressed(key, modifiers, physical_key)),
                keyboard::Event::ModifiersChanged(modifiers) => {
                    Some(Message::ModifiersChanged(modifiers))
                }
                _ => None,
            }),
            // Escape needs its own listener: `keyboard::listen()` only
            // yields events with `Status::Ignored`, and a focused
            // `text_input` swallows Escape with `capture_event()` (it
            // unfocuses itself). Without this the find palette needed two
            // presses to close - one eaten by the field, one seen here.
            iced::event::listen_with(|event, status, _window| {
                let iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key, modifiers, ..
                }) = event
                else {
                    return None;
                };
                match key.as_ref() {
                    keyboard::Key::Named(key::Named::Escape) => Some(Message::CloseFind),
                    // Find-again, but only for a press some widget swallowed.
                    // An uncaptured one already reached `handle_hotkey`, and
                    // acting on it twice would skip a match.
                    keyboard::Key::Character("g")
                        if modifiers.command()
                            && status == iced::event::Status::Captured =>
                    {
                        Some(if modifiers.shift() {
                            Message::FindPrevious
                        } else {
                            Message::FindNext
                        })
                    }
                    _ => None,
                }
            }),
            // Files dragged in from Finder/Explorer. iced surfaces these
            // itself; see AGENTS.md for the Wayland gap underneath.
            iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Window(iced::window::Event::FileHovered(_)) => {
                    Some(Message::FilesHovered(true))
                }
                iced::Event::Window(iced::window::Event::FilesHoveredLeft) => {
                    Some(Message::FilesHovered(false))
                }
                iced::Event::Window(iced::window::Event::FileDropped(path)) => {
                    Some(Message::FileDropped(path))
                }
                iced::Event::Window(iced::window::Event::Focused) => {
                    Some(Message::WindowFocused)
                }
                _ => None,
            }),
            iced::window::close_requests().map(Message::WindowCloseRequested),
            hotkey::subscription(),
            reload::subscription(),
            docwatch::subscription(self.watched_paths()),
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

        if self.surface_reset_frames > 0 {
            subscriptions.push(iced::window::frames().map(|_| Message::SurfaceResetFrame));
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

        if self.config_watch.pending() {
            subscriptions.push(
                iced::time::every(reload::SETTLE_TICK).map(|_| Message::ConfigSettleTick),
            );
        }

        if self.document_watch.pending() {
            subscriptions.push(
                iced::time::every(docwatch::SETTLE_TICK).map(|_| Message::DocumentSettleTick),
            );
        }

        Subscription::batch(subscriptions)
    }
}

/// App-level command names a `keybinds.toml` override may target.
/// A user's `keybinds.toml` remaps, resolved to physical key + modifiers.
///
/// Physical rather than logical so a remap lands on the same *place* on every
/// layout, which is what lets a German-layout user reach a chord their layout
/// cannot type directly.
pub type KeyOverrides = HashMap<(keyboard::Modifiers, key::Code), Action>;

/// Resolves `keybinds.toml`'s overrides into a lookup keyed by physical key +
/// modifiers. One table for both layers now that both speak `Action` - it
/// used to be two, `build_app_overrides` and `build_editor_overrides`, each
/// carrying its own copy of the command-name list.
fn build_key_overrides(keybinds: &jumppad_config::KeybindsConfig) -> KeyOverrides {
    let mut map = HashMap::new();
    for (name, resolved) in keybinds.resolved_overrides() {
        if let Some(action) = Action::from_name(&name) {
            map.insert((resolved.modifiers, resolved.code), action);
        }
    }
    map
}

/// How the shell performs an [`Action`], or `None` for one it doesn't own -
/// every `Action::Editor`, which the text widget handles instead.
///
/// The other half of `jumppad_textarea::binding_for`; between them they must
/// cover every action, which `every_action_is_wired_to_something` checks.
fn message_for(action: Action) -> Option<Message> {
    match action {
        Action::NewTab => Some(Message::NewTab),
        Action::OpenFile => Some(Message::OpenFile),
        Action::SaveFile => Some(Message::SaveFile),
        Action::SaveFileAs => Some(Message::SaveFileAs),
        Action::CloseActiveTab => Some(Message::CloseActiveTab),
        Action::SelectPreviousTab => Some(Message::SelectPreviousTab),
        Action::SelectNextTab => Some(Message::SelectNextTab),
        Action::SelectPreviousActiveTab => Some(Message::SelectPreviousActiveTab),
        Action::Find => Some(Message::OpenFind),
        Action::FindNext => Some(Message::FindNext),
        Action::FindPrevious => Some(Message::FindPrevious),
        _ => None,
    }
}

/// The action a press asks for: a user override first, then the shipped
/// default chords. Shared by the shell and the editor widget, so the two can
/// no longer disagree about which wins.
fn resolve_action(
    key: &keyboard::Key,
    physical_key: key::Physical,
    modifiers: keyboard::Modifiers,
    context: Context,
    overrides: &KeyOverrides,
) -> Option<Action> {
    if let key::Physical::Code(code) = physical_key {
        if let Some(&action) = overrides.get(&(modifiers, code)) {
            if action.context() == Context::Always || action.context() == context {
                return Some(action);
            }
        }
    }
    jumppad_keybinds::action_for(key, physical_key, modifiers, context)
}

/// The resolver handed to every `TextArea`, closing over the overrides of the
/// moment. Rebuilt and re-injected on a `keybinds.toml` reload.
fn build_key_resolver(overrides: Arc<KeyOverrides>) -> Arc<jumppad_textarea::KeyResolver> {
    Arc::new(move |press: &jumppad_textarea::KeyPress| {
        resolve_action(
            &press.key,
            press.physical_key,
            press.modifiers,
            Context::EditorFocused,
            &overrides,
        )
    })
}

/// Logs (doesn't fail) any `keybinds.toml` override whose command name
/// isn't recognized by either layer - a cheap typo-catcher, not a
/// validation framework.
fn warn_unrecognized_overrides(overrides: &HashMap<String, global_hotkey::hotkey::HotKey>) {
    for name in overrides.keys() {
        if Action::from_name(name).is_none() {
            eprintln!(
                "jumppad_config: keybinds.toml overrides an unrecognized command {name:?}, ignoring"
            );
        }
    }
}

fn handle_hotkey(
    key: keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical_key: key::Physical,
    overrides: &KeyOverrides,
) -> Option<Message> {
    // `Context::Always`: the shell's own shortcuts, which do not require the
    // editor to hold focus. An editor action resolved here returns `None`
    // from `message_for` and falls through to the widget, as it always has.
    let action = resolve_action(
        &key,
        physical_key,
        modifiers,
        Context::Always,
        overrides,
    )?;

    message_for(action)
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

/// One modal choice button, shared by both dialogs - they differ only in
/// their labels and the message each choice sends. The caller adds that with
/// `.on_press`, so a click resolves the dialog the same way Enter does.
fn modal_choice(label: &'static str, is_focused: bool) -> button::Button<'static, Message> {
    button(text(label))
        .padding([6, 14])
        .style(move |theme, status| modal_button_style(theme, status, is_focused))
}

/// A modal's box: one line of prompt over a row of choices.
fn modal_dialog<'a>(
    prompt: String,
    choices: iced::widget::Row<'a, Message>,
) -> container::Container<'a, Message> {
    container(
        column![text(prompt), choices.spacing(10)]
            .spacing(16)
            .padding(20),
    )
    .style(modal_dialog_style)
}

/// One of a modal's three choices - a colored border marks whichever one
/// keyboard nav currently has focused.
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
/// Absolute like the tab bar's, so the palette's edges land on whole
/// pixels instead of being antialiased into seams (see AGENTS.md).
const FIND_TEXT_LINE_HEIGHT: Pixels = Pixels(16.0);

fn find_palette_style(theme: &Theme) -> container::Style {
    container::Style::default().background(darkening_wash(theme, FLOATING_SURFACE_DARKEN))
}

/// How far the drag-and-drop overlay dims the document underneath. Lighter
/// than a floating surface - it covers the whole editor, and the text below it
/// should still read as text.
const DROP_OVERLAY_DARKEN: f32 = 0.08;

/// The overlay shown while files are dragged over the window. A wash, not a
/// pre-darkened background copy, so a transparent window stays transparent
/// through it (see AGENTS.md); the accent border is what carries the cue on
/// the macOS software build, which is always opaque.
fn drop_overlay_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style::default()
        .background(darkening_wash(theme, DROP_OVERLAY_DARKEN))
        .border(iced::Border {
            color: palette.primary.base.color,
            width: 2.0,
            radius: 6.0.into(),
        })
}

fn find_input_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let palette = theme.extended_palette();
    let default = text_input::default(theme, status);
    text_input::Style {
        // The palette behind it is already a distinct surface; a second
        // filled quad on top would just compound opacity (see AGENTS.md).
        background: Color::TRANSPARENT.into(),
        border: iced::Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..default
    }
}

fn find_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let text_color = theme.extended_palette().background.base.text;
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
    Theme::custom("jumppad (redraw nudge)".to_string(), palette)
}

/// Whether this build has to premultiply the window's clear color before
/// handing it to iced - see `premultiply` for what goes wrong without it.
///
/// Two conditions have to hold. The backend must be `wgpu`, since only its
/// clear-color path writes straight alpha (`tiny-skia` premultiplies
/// internally, so feeding it a premultiplied color would double-darken).
/// And the platform compositor must read the presented surface as
/// premultiplied - confirmed on the macOS window server and on Windows' DWM,
/// each by the same symptom: light themes going opaque while dark themes
/// looked fine.
///
/// Linux is the one holdout. Wayland and compositing X11 are premultiplied
/// too, so it very likely belongs here, but nobody has reproduced the
/// symptom there and a wrong guess costs opacity on a window that currently
/// looks right. The tell to watch for is a *light* theme, not a dark one.
const CLEAR_COLOR_NEEDS_PREMULTIPLY: bool = cfg!(all(
    any(target_os = "macos", target_os = "windows"),
    feature = "wgpu"
));

/// Premultiplies a color's RGB by its alpha, on the sRGB-encoded channel
/// values - the space desktop compositors composite in.
///
/// **The solid-white-window fix.** The compositor composites the surface
/// as *premultiplied* alpha - `src + (1 - a) * desktop` - but iced's clear
/// color is written straight, so a straight white background saturates to
/// solid white at any alpha, while a near-black one (rgb ~ 0) happens to
/// look right; only light themes ever look broken. Quads and glyphs are
/// unaffected - iced's shaders premultiply before writing (see AGENTS.md).
///
/// That asymmetry is the whole diagnostic. `src_rgb` saturates the channel
/// on its own once `rgb` is near 1, so alpha stops mattering entirely and a
/// light theme reads as an opaque window at *any* configured alpha - even
/// 0.1. A dark theme at the same alpha looks nearly right. "Light themes are
/// opaque, dark themes are fine" means this bug; "everything is uniformly
/// too dark" means the opposite mistake, premultiplying where it isn't
/// wanted.
///
/// Not in linear space: the composite runs on encoded values, and
/// `encode(linear * a) > encode(linear) * a`, so premultiplying before the
/// sRGB encode over-brightens - white at alpha 0.1 came out ~3.5x too bright.
/// This also lands the wgpu build on exactly the bytes `tiny-skia` presents:
/// `iced_wgpu`'s clear color round-trips through `Color::into_linear()` and
/// back out through the sRGB surface's encode, so the encoded value written
/// is the one passed in here.
fn premultiply(color: Color) -> Color {
    Color {
        r: color.r * color.a,
        g: color.g * color.a,
        b: color.b * color.a,
        a: color.a,
    }
}

/// Mirror of `build_editor_overrides` for comment styles - built here so
/// `jumppad_textarea` doesn't need to depend on `jumppad_config`.
fn build_comment_styles(
    config: &jumppad_config::Config,
) -> HashMap<String, jumppad_textarea::CommentStyle> {
    config
        .comment_styles_by_extension()
        .into_iter()
        .map(|(extension, style)| {
            let style = match style {
                jumppad_config::CommentSyntax::Single(prefix) => {
                    jumppad_textarea::CommentStyle::Single(prefix)
                }
                jumppad_config::CommentSyntax::Multi { left, right } => {
                    jumppad_textarea::CommentStyle::Multi { left, right }
                }
            };
            (extension, style)
        })
        .collect()
}

/// A reloaded setting that only applies at startup. Logged, not shown in
/// the banner: the change is valid, it just waits for the next start.
fn restart_required(what: &str) {
    eprintln!("jumppad: {what} changed - takes effect on restart");
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
                "jumppad: unknown theme {name:?}, using default. Valid options: {valid}"
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
                    eprintln!("jumppad: {}: exists but no .wasm files found", dir.display());
                } else {
                    eprintln!(
                        "jumppad: {}: found {} .wasm file(s): {}",
                        dir.display(),
                        wasm_files.len(),
                        wasm_files.join(", ")
                    );
                }
            }
            Err(err) => {
                eprintln!("jumppad: {}: couldn't read directory: {err}", dir.display());
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

/// Reads a path that's already known - a dropped file, and the tail of the
/// Open dialog once it has produced one.
async fn read_path(path: PathBuf) -> Result<(PathBuf, Arc<String>), OpenError> {
    let contents = tokio::fs::read_to_string(&path)
        .await
        .map(Arc::new)
        .map_err(|err| OpenError::Io {
            path: path.clone(),
            kind: err.kind(),
        })?;
    Ok((path, contents))
}

/// Shows the native Open File dialog and reads the chosen file's contents.
async fn open_and_read() -> Result<(PathBuf, Arc<String>), OpenError> {
    let handle = rfd::AsyncFileDialog::new()
        .pick_file()
        .await
        .ok_or(OpenError::DialogClosed)?;
    read_path(handle.path().to_owned()).await
}

/// Whether writing to `path` now would clobber a change made since the tab
/// last looked.
///
/// A missing file is never a conflict: there is nothing to clobber, and the
/// write recreates it - the delete rule. A file that *appeared* where the
/// tab saw none is, though, which is why `Seen` carries an `Option` rather
/// than folding "saw nothing" into `Unchecked`.
fn conflicts(path: &Path, expected: SaveExpectation) -> bool {
    match expected {
        SaveExpectation::Unchecked => false,
        SaveExpectation::Seen(seen) => {
            DiskStamp::of(path).is_some_and(|current| Some(current) != seen)
        }
    }
}

/// Writes `text` to the tab's file, and stamps what it wrote. Stamping here
/// rather than back in `update` closes the window where the app would record
/// a file that had already been modified again externally - which is what
/// keeps a save from reading as an external change and reloading itself.
///
/// `expected` is the stamp the tab believes the file has; a mismatch is a
/// conflict and nothing is written. There is an unavoidable stat-then-write
/// window between the check and the write - VS Code has the same one.
async fn save_to(
    existing_path: Option<PathBuf>,
    text: String,
    force_dialog: bool,
    expected: SaveExpectation,
) -> Result<(PathBuf, Option<DiskStamp>), SaveError> {
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

    if conflicts(&path, expected) {
        return Err(SaveError::Conflict);
    }

    tokio::fs::write(&path, text)
        .await
        .map_err(|err| SaveError::Io {
            path: path.clone(),
            kind: err.kind(),
        })?;
    let stamp = DiskStamp::of(&path);
    Ok((path, stamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::WASH_ALPHA_CEILING;
    use editor_core::{SavedSelection, SelectionKind, TextEditorWidget};
    use iced::keyboard::key::Named;
    use iced::keyboard::{Key, Modifiers};

    /// A minimal `TextEditorWidget` for tests, with no real rendering - it
    /// holds its text so file-backed flows can assert on what landed in the
    /// buffer.
    struct StubEditor(String);

    impl TextEditorWidget for StubEditor {
        fn view(&self) -> Element<'_, EditorMessage> {
            iced::widget::text("").into()
        }
        fn update(&mut self, _message: EditorMessage) -> bool {
            true // reports every message as an edit
        }
        fn text(&self) -> String {
            self.0.clone()
        }
        fn set_text(&mut self, text: &str) {
            self.0 = text.to_string();
        }
        fn reload_text(&mut self, text: &str) {
            self.0 = text.to_string();
        }
        fn poll_highlighting(&mut self) {}
        fn cursor_position(&self) -> (usize, usize) {
            (0, 0)
        }
        fn move_cursor_to(&mut self, _line: usize, _column: usize) {}
        fn selection(&self) -> Option<SavedSelection> {
            None
        }
        fn restore_selection(&mut self, _selection: SavedSelection, _cursor: (usize, usize)) {}
        fn set_find_matches(
            &mut self,
            _matches: Vec<editor_core::FindMatch>,
            _current: Option<usize>,
        ) {
        }
        fn has_pending_highlighting(&self) -> bool {
            false
        }
    }

    /// The close prompt currently showing, for tests that assert on which
    /// tab it's for and where its focus sits.
    fn close_prompt(app: &JumpPadApp) -> Option<&PendingClose> {
        match &app.modal {
            Some(Modal::Close(pending)) => Some(pending),
            _ => None,
        }
    }

    /// Mirror of `close_prompt` for the save-conflict dialog.
    fn conflict_prompt(app: &JumpPadApp) -> Option<&PendingConflict> {
        match &app.modal {
            Some(Modal::SaveConflict(pending)) => Some(pending),
            _ => None,
        }
    }

    fn stub_factory() -> EditorFactory {
        Box::new(|text, _extension| {
            Box::new(StubEditor(text.to_string())) as Box<dyn TextEditorWidget>
        })
    }

    /// Records every message it's handed, for asserting on what the app
    /// actually dispatches to an editor.
    struct RecordingEditor(std::rc::Rc<std::cell::RefCell<Vec<EditorMessage>>>);

    impl TextEditorWidget for RecordingEditor {
        fn view(&self) -> Element<'_, EditorMessage> {
            iced::widget::text("").into()
        }
        fn update(&mut self, message: EditorMessage) -> bool {
            self.0.borrow_mut().push(message);
            false
        }
        fn text(&self) -> String {
            String::new()
        }
        fn set_text(&mut self, _text: &str) {}
        fn reload_text(&mut self, _text: &str) {}
        fn poll_highlighting(&mut self) {}
        fn cursor_position(&self) -> (usize, usize) {
            (0, 0)
        }
        fn move_cursor_to(&mut self, _line: usize, _column: usize) {}
        fn selection(&self) -> Option<SavedSelection> {
            None
        }
        fn restore_selection(&mut self, _selection: SavedSelection, _cursor: (usize, usize)) {}
        fn set_find_matches(
            &mut self,
            _matches: Vec<editor_core::FindMatch>,
            _current: Option<usize>,
        ) {
        }
        fn has_pending_highlighting(&self) -> bool {
            false
        }
    }

    /// What `switch_active` restored into an editor - see `SelectionSpyEditor`.
    #[derive(Debug, PartialEq)]
    enum Restore {
        Cursor(usize, usize),
        Selection {
            selection: SavedSelection,
            cursor: (usize, usize),
        },
    }

    /// Reports a canned cursor/selection and records what the app restores,
    /// for asserting on `switch_active`'s save/restore orchestration.
    struct SelectionSpyEditor {
        selection: Option<SavedSelection>,
        cursor: (usize, usize),
        restored: std::rc::Rc<std::cell::RefCell<Vec<Restore>>>,
    }

    impl TextEditorWidget for SelectionSpyEditor {
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
        fn reload_text(&mut self, _text: &str) {}
        fn poll_highlighting(&mut self) {}
        fn cursor_position(&self) -> (usize, usize) {
            self.cursor
        }
        fn move_cursor_to(&mut self, line: usize, column: usize) {
            self.restored.borrow_mut().push(Restore::Cursor(line, column));
        }
        fn selection(&self) -> Option<SavedSelection> {
            self.selection
        }
        fn restore_selection(&mut self, selection: SavedSelection, cursor: (usize, usize)) {
            self.restored.borrow_mut().push(Restore::Selection { selection, cursor });
        }
        fn set_find_matches(
            &mut self,
            _matches: Vec<editor_core::FindMatch>,
            _current: Option<usize>,
        ) {
        }
        fn has_pending_highlighting(&self) -> bool {
            false
        }
    }

    /// Builds an `JumpPadApp` with `tab_count` untitled tabs, skipping the real I/O `new()` does.
    fn test_app(tab_count: u64) -> JumpPadApp {
        let factory = stub_factory();
        let tabs = (0..tab_count).map(|id| Tab::untitled(id, &factory)).collect();
        JumpPadApp {
            tabs,
            active: 0,
            next_id: tab_count,
            error: None,
            editor_factory: factory,
            theme: Theme::ALL[0].clone(),
            background_alpha: 1.0,
            redraw_nudge_frames: 0,
            shadow_refresh_frames: 0,
            surface_reset_frames: 0,
            session_dir: PathBuf::from("/tmp"),
            pending_close_after_save: Vec::new(),
            window: None,
            hotkey: None,
            visor_visible: false,
            animation: None,
            visor_enabled: false,
            previous_active_id: None,
            keybind_overrides: Arc::new(HashMap::new()),
            modal: None,
            close_queue: Vec::new(),
            conflict_queue: Vec::new(),
            file_dialog_active: false,
            files_hovered: false,
            find: HashMap::new(),
            modifiers: Modifiers::default(),
            config: jumppad_config::Config::default(),
            keybinds: jumppad_config::KeybindsConfig::default(),
            editor_config: jumppad_textarea::SharedEditorConfig::new(1.0, Arc::new(|_: &jumppad_textarea::KeyPress| None)),
            window_transparent: false,
            config_watch: reload::ConfigWatch::new(),
            document_watch: docwatch::DocumentWatch::new(),
        }
    }

    /// End-to-end scenario against real `TextArea`s (not stubs): two
    /// tabs make selections in turn, and each keeps its own through switches.
    #[test]
    fn each_tab_keeps_its_own_selection_through_switches() {
        use iced::widget::text_editor::{Action, Motion};

        let factory: EditorFactory = Box::new(|text, _extension| {
            let registry = syntax_registry::SyntaxRegistry::new(Vec::new(), HashMap::new(), || {});
            Box::new(jumppad_textarea::TextArea::new(
                text,
                &registry,
                None,
                jumppad_textarea::SharedEditorConfig::new(1.0, Arc::new(|_: &jumppad_textarea::KeyPress| None)),
            ))
        });
        let mut app = test_app(0);
        app.tabs = vec![
            Tab::restored(0, None, "alpha beta", false, &factory),
            Tab::restored(1, None, "gamma delta", false, &factory),
        ];
        app.next_id = 2;

        // Tab 0: a double-click-style word selection (Click then SelectWord,
        // the sequence the widget publishes). Regression case: a word
        // selection's anchor sits at the cursor, not across the range.
        let _ = app.update(Message::Editor(
            0,
            EditorMessage::Action(Action::Click(iced::Point::new(10.0, 4.0))),
        ));
        let _ = app.update(Message::Editor(0, EditorMessage::Action(Action::SelectWord)));
        let word = app.tabs[0].editor.selection();
        assert!(
            matches!(word, Some(SavedSelection { kind: SelectionKind::Word, .. })),
            "expected a word selection, got {word:?}"
        );

        // Tab 1: a keyboard range selection over "ga".
        let _ = app.update(Message::SelectTab(1));
        for _ in 0..2 {
            let _ = app.update(Message::Editor(1, EditorMessage::Action(Action::Select(Motion::Right))));
        }

        // Both tabs hold their own selection, verified across two round trips.
        let _ = app.update(Message::SelectTab(0));
        assert_eq!(app.tabs[0].editor.selection(), word);

        let _ = app.update(Message::SelectTab(1));
        assert_eq!(
            app.tabs[1].editor.selection(),
            Some(SavedSelection { anchor: (0, 0), kind: SelectionKind::Range })
        );
        assert_eq!(app.tabs[1].editor.cursor_position(), (0, 2));
    }

    /// Same scenario as above, but with the message sequence a real mouse
    /// selection produces (Click then Drags), exercising pixel hit-testing.
    #[test]
    fn mouse_made_selections_stay_independent_per_tab() {
        use iced::Point;
        use iced::widget::text_editor::Action;

        let factory: EditorFactory = Box::new(|text, _extension| {
            let registry = syntax_registry::SyntaxRegistry::new(Vec::new(), HashMap::new(), || {});
            Box::new(jumppad_textarea::TextArea::new(
                text,
                &registry,
                None,
                jumppad_textarea::SharedEditorConfig::new(1.0, Arc::new(|_: &jumppad_textarea::KeyPress| None)),
            ))
        });
        let mut app = test_app(0);
        app.tabs = vec![
            Tab::restored(0, None, "alpha beta", false, &factory),
            Tab::restored(1, None, "gamma delta", false, &factory),
        ];
        app.next_id = 2;

        let _ = app.update(Message::Editor(0, EditorMessage::Action(Action::Click(Point::ORIGIN))));
        let _ = app.update(Message::Editor(0, EditorMessage::Action(Action::Drag(Point::new(500.0, 4.0)))));
        let selection_0 = app.tabs[0].editor.selection();
        assert!(selection_0.is_some());
        let cursor_0 = app.tabs[0].editor.cursor_position();

        let _ = app.update(Message::SelectTab(1));
        let _ = app.update(Message::Editor(1, EditorMessage::Action(Action::Click(Point::ORIGIN))));
        let _ = app.update(Message::Editor(1, EditorMessage::Action(Action::Drag(Point::new(500.0, 4.0)))));
        assert!(app.tabs[1].editor.selection().is_some());

        let _ = app.update(Message::SelectTab(0));
        assert_eq!(app.tabs[0].editor.selection(), selection_0);
        assert_eq!(app.tabs[0].editor.cursor_position(), cursor_0);
    }

    /// An app whose tabs hold real `TextArea`s seeded with `texts`,
    /// for find flows that need actual text and cursor behavior.
    fn app_with_text(texts: &[&str]) -> JumpPadApp {
        let factory: EditorFactory = Box::new(|text, _extension| {
            let registry = syntax_registry::SyntaxRegistry::new(Vec::new(), HashMap::new(), || {});
            Box::new(jumppad_textarea::TextArea::new(
                text,
                &registry,
                None,
                jumppad_textarea::SharedEditorConfig::new(1.0, Arc::new(|_: &jumppad_textarea::KeyPress| None)),
            ))
        });
        let mut app = test_app(0);
        app.tabs = texts
            .iter()
            .enumerate()
            .map(|(index, body)| Tab::restored(index as u64, None, body, false, &factory))
            .collect();
        app.next_id = texts.len() as u64;
        app.editor_factory = factory;
        app
    }

    #[test]
    fn opening_find_searches_and_selects_the_first_match() {
        let mut app = app_with_text(&["one two\nthree two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));

        let state = app.active_find().expect("palette open");
        assert!(state.open);
        assert_eq!(state.matches.len(), 2);
        assert_eq!(state.counter().as_deref(), Some("1 of 2"));
        // The first match is selected in the document, not just counted.
        assert_eq!(app.tabs[0].editor.cursor_position(), (0, 7));
    }

    #[test]
    fn find_next_and_previous_step_and_wrap() {
        let mut app = app_with_text(&["one two\nthree two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));

        let _ = app.update(Message::FindNext);
        assert_eq!(app.active_find().unwrap().counter().as_deref(), Some("2 of 2"));
        assert_eq!(app.tabs[0].editor.cursor_position(), (1, 9));

        let _ = app.update(Message::FindNext);
        assert_eq!(
            app.active_find().unwrap().counter().as_deref(),
            Some("1 of 2"),
            "next past the last wraps"
        );

        let _ = app.update(Message::FindPrevious);
        assert_eq!(
            app.active_find().unwrap().counter().as_deref(),
            Some("2 of 2"),
            "previous before the first wraps"
        );
    }

    #[test]
    fn escape_closes_the_palette_but_keeps_the_query() {
        let mut app = app_with_text(&["one two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));
        assert!(app.find_is_open());

        // Escape arrives as `CloseFind` from the dedicated event listener -
        // `keyboard::listen()` never sees it, since a focused `text_input`
        // captures Escape.
        let _ = app.update(Message::CloseFind);
        assert!(!app.find_is_open(), "escape closes the palette");
        assert_eq!(
            app.active_find().map(|state| state.query.as_str()),
            Some("two"),
            "the query survives for next time"
        );

        // Reopening brings the same query straight back.
        let _ = app.update(Message::OpenFind);
        assert!(app.find_is_open());
        assert_eq!(app.active_find().unwrap().matches.len(), 1);
    }

    #[test]
    fn escape_leaves_the_find_palette_alone_while_the_modal_is_up() {
        // Escape now reaches the app from a listener that fires regardless
        // of which widget has focus, so the modal has to keep first claim on
        // it - cancelling the prompt, not quietly closing find behind it.
        let mut app = app_with_text(&["one two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));
        app.tabs[0].dirty = true;
        let _ = app.request_close(0);
        assert!(app.modal.is_some());

        let _ = app.update(Message::CloseFind);
        assert!(app.find_is_open(), "the modal owns Escape while it is up");
        assert!(app.modal.is_some());
    }

    #[test]
    fn each_tab_keeps_its_own_find_query() {
        let mut app = app_with_text(&["alpha beta", "gamma delta gamma"]);

        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("beta".into()));

        let _ = app.update(Message::SelectTab(1));
        // Tab 1 has no palette of its own yet.
        assert!(!app.find_is_open());
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("gamma".into()));
        assert_eq!(app.active_find().unwrap().counter().as_deref(), Some("1 of 2"));

        // Back to tab 0: its own query and its own match count.
        let _ = app.update(Message::SelectTab(0));
        let state = app.active_find().expect("tab 0 palette");
        assert_eq!(state.query, "beta");
        assert_eq!(state.counter().as_deref(), Some("1 of 1"));

        let _ = app.update(Message::SelectTab(1));
        assert_eq!(app.active_find().unwrap().query, "gamma");
    }

    #[test]
    fn closing_a_tab_forgets_its_find_state() {
        let mut app = app_with_text(&["one two", "other"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));
        let closed_id = app.tabs[0].id;
        assert!(app.find.contains_key(&closed_id));

        let _ = app.close_tab(0);
        assert!(!app.find.contains_key(&closed_id));
    }

    #[test]
    fn editing_the_document_refreshes_the_match_list() {
        use iced::widget::text_editor;

        let mut app = app_with_text(&["two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));
        assert_eq!(app.active_find().unwrap().matches.len(), 1);

        // Typing a second occurrence into the document updates the count.
        app.tabs[0].editor.move_cursor_to(0, 3);
        for character in " two".chars() {
            let _ = app.update(Message::Editor(
                0,
                EditorMessage::Action(text_editor::Action::Edit(text_editor::Edit::Insert(
                    character,
                ))),
            ));
        }
        assert_eq!(app.active_find().unwrap().matches.len(), 2);
    }

    #[test]
    fn find_on_a_missing_match_reports_no_results() {
        let mut app = app_with_text(&["one two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("zebra".into()));
        let state = app.active_find().unwrap();
        assert!(state.matches.is_empty());
        assert_eq!(state.counter().as_deref(), Some("No results"));

        // Stepping with nothing to step through must not panic.
        let _ = app.update(Message::FindNext);
        assert_eq!(app.active_find().unwrap().current, None);
    }

    #[test]
    fn find_matches_case_insensitively_through_the_app() {
        let mut app = app_with_text(&["Two two TWO"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));
        assert_eq!(app.active_find().unwrap().counter().as_deref(), Some("1 of 3"));
    }

    #[test]
    fn command_g_finds_again_with_the_palette_closed() {
        let mut app = app_with_text(&["two one two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));
        let _ = app.update(Message::CloseFind);
        assert!(!app.find_is_open());
        assert_eq!(app.tabs[0].editor.cursor_position(), (0, 3), "on the first match");

        // Cmd+G steps without reopening the palette.
        let _ = app.update(Message::FindNext);
        assert!(!app.find_is_open(), "find-again must not reopen the palette");
        assert_eq!(app.tabs[0].editor.cursor_position(), (0, 11), "second match");

        // And wraps back around.
        let _ = app.update(Message::FindNext);
        assert_eq!(app.tabs[0].editor.cursor_position(), (0, 3));
    }

    #[test]
    fn find_again_picks_up_document_edits_made_while_closed() {
        use iced::widget::text_editor;

        let mut app = app_with_text(&["two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));
        let _ = app.update(Message::CloseFind);
        assert_eq!(app.active_find().unwrap().matches.len(), 1);

        // Append a second occurrence with the palette shut - the stale match
        // list must be re-searched before stepping.
        app.tabs[0].editor.move_cursor_to(0, 3);
        for character in " two".chars() {
            let _ = app.update(Message::Editor(
                0,
                EditorMessage::Action(text_editor::Action::Edit(text_editor::Edit::Insert(
                    character,
                ))),
            ));
        }
        // Step from the top so the target is unambiguous - starting at the
        // cursor's post-edit spot (the end of the last match) would wrap.
        app.tabs[0].editor.move_cursor_to(0, 0);
        let _ = app.update(Message::FindNext);
        assert_eq!(
            app.active_find().unwrap().matches.len(),
            2,
            "the stale match list was re-searched"
        );
        assert_eq!(app.tabs[0].editor.cursor_position(), (0, 7), "the new match");
    }

    #[test]
    fn find_again_without_a_query_does_nothing() {
        let mut app = app_with_text(&["one two"]);
        // No palette ever opened for this tab.
        let _ = app.update(Message::FindNext);
        assert_eq!(app.tabs[0].editor.cursor_position(), (0, 0));
        assert!(app.active_find().is_none());
    }

    #[test]
    fn find_again_steps_while_the_palette_is_open() {
        let mut app = app_with_text(&["two one two three two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));
        assert_eq!(app.active_find().unwrap().counter().as_deref(), Some("1 of 3"));

        // Cmd+G with the palette still up steps without closing it.
        let _ = app.update(Message::FindNext);
        assert!(app.find_is_open());
        assert_eq!(app.active_find().unwrap().counter().as_deref(), Some("2 of 3"));

        // Cmd+Shift+G goes back.
        let _ = app.update(Message::FindPrevious);
        assert_eq!(app.active_find().unwrap().counter().as_deref(), Some("1 of 3"));
        let _ = app.update(Message::FindPrevious);
        assert_eq!(
            app.active_find().unwrap().counter().as_deref(),
            Some("3 of 3"),
            "previous past the first wraps"
        );
    }

    #[test]
    fn a_shortcut_character_leaked_while_command_is_held_is_not_typed_into_the_query() {
        // macOS only in practice: Cmd doesn't suppress character production
        // and `text_input` inserts it, so Cmd+G would append "g" to the
        // query on its way to being a shortcut.
        let mut app = app_with_text(&["one two"]);
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));

        let _ = app.update(Message::ModifiersChanged(Modifiers::COMMAND));
        let _ = app.update(Message::FindQueryChanged("twog".into()));
        assert_eq!(
            app.active_find().unwrap().query,
            "two",
            "the leaked shortcut character must not reach the query"
        );

        // A multi-character change with command held is a paste, not a leak.
        let _ = app.update(Message::FindQueryChanged("twobeta".into()));
        assert_eq!(app.active_find().unwrap().query, "twobeta", "paste still lands");

        // And ordinary typing is untouched once command is released.
        let _ = app.update(Message::ModifiersChanged(Modifiers::empty()));
        let _ = app.update(Message::FindQueryChanged("twobetax".into()));
        assert_eq!(app.active_find().unwrap().query, "twobetax");
    }

    #[test]
    fn command_shift_g_resolves_to_find_previous() {
        let overrides = HashMap::new();
        let resolved = handle_hotkey(
            Key::Character("g".into()),
            Modifiers::CTRL | Modifiers::SHIFT,
            key::Physical::Code(key::Code::KeyG),
            &overrides,
        );
        assert!(matches!(resolved, Some(Message::FindPrevious)));
    }

    #[test]
    fn command_g_resolves_to_find_next() {
        let overrides = HashMap::new();
        let resolved = handle_hotkey(
            Key::Character("g".into()),
            Modifiers::CTRL,
            key::Physical::Code(key::Code::KeyG),
            &overrides,
        );
        assert!(matches!(resolved, Some(Message::FindNext)));
    }

    #[test]
    fn command_f_resolves_to_open_find() {
        // `Modifiers::CTRL` stands in for `command()`, which resolves per-OS
        // at compile time - the same convention the undo/redo tests use.
        let overrides = HashMap::new();
        let resolved = handle_hotkey(
            Key::Character("f".into()),
            Modifiers::CTRL,
            key::Physical::Code(key::Code::KeyF),
            &overrides,
        );
        assert!(matches!(resolved, Some(Message::OpenFind)));
    }

    #[test]
    fn shift_click_reaches_the_editor_as_a_selection_extending_drag() {
        use iced::widget::text_editor;

        let mut app = test_app(1);
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        app.tabs[0].editor = Box::new(RecordingEditor(log.clone()));
        let click = EditorMessage::Action(text_editor::Action::Click(iced::Point::ORIGIN));

        let _ = app.update(Message::ModifiersChanged(Modifiers::SHIFT));
        let _ = app.update(Message::Editor(0, click.clone()));
        // Releasing shift restores the plain click.
        let _ = app.update(Message::ModifiersChanged(Modifiers::default()));
        let _ = app.update(Message::Editor(0, click));

        assert!(matches!(
            log.borrow().as_slice(),
            [
                EditorMessage::Action(text_editor::Action::Drag(_)),
                EditorMessage::Action(text_editor::Action::Click(_)),
            ]
        ));
    }

    #[test]
    fn switching_tabs_saves_and_restores_the_selection() {
        let mut app = test_app(2);
        let restored = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let saved = SavedSelection { anchor: (0, 2), kind: SelectionKind::Range };
        app.tabs[0].editor = Box::new(SelectionSpyEditor {
            selection: Some(saved),
            cursor: (1, 4),
            restored: restored.clone(),
        });

        let _ = app.switch_active(1);
        assert_eq!(app.tabs[0].last_selection, Some(saved));
        assert_eq!(app.tabs[0].last_cursor, (1, 4));

        let _ = app.switch_active(0);
        assert_eq!(
            restored.borrow().as_slice(),
            [Restore::Selection { selection: saved, cursor: (1, 4) }]
        );
    }

    #[test]
    fn switching_to_a_tab_without_a_selection_restores_only_the_cursor() {
        let mut app = test_app(2);
        let restored = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        app.tabs[0].editor = Box::new(SelectionSpyEditor {
            selection: None,
            cursor: (3, 1),
            restored: restored.clone(),
        });

        let _ = app.switch_active(1);
        assert_eq!(app.tabs[0].last_selection, None);

        let _ = app.switch_active(0);
        assert_eq!(restored.borrow().as_slice(), [Restore::Cursor(3, 1)]);
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

    fn no_overrides() -> KeyOverrides {
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
    fn every_action_is_wired_to_exactly_one_layer() {
        // The composition root is the only place that can see both mappers,
        // so this is the only place the question can be asked. An action
        // added to the registry and wired to nothing fails here rather than
        // going quiet at runtime.
        for action in jumppad_actions::Action::ALL {
            let shell = message_for(*action).is_some();
            let editor = jumppad_textarea::binding_for(*action).is_some();
            assert!(
                shell != editor,
                "{action} is handled by {}",
                match (shell, editor) {
                    (true, true) => "both layers",
                    _ => "neither layer",
                }
            );
        }
    }

    #[test]
    fn a_category_matches_the_layer_that_performs_it() {
        use jumppad_actions::Category;

        for action in jumppad_actions::Action::ALL {
            let expected = match action.category() {
                Category::App => message_for(*action).is_some(),
                Category::Editor => {
                    jumppad_textarea::binding_for(*action).is_some()
                }
            };
            assert!(expected, "{action} is filed under the wrong category");
        }
    }

    #[test]
    fn override_wins_over_a_conflicting_hardcoded_default() {
        // Ctrl+N normally binds NewTab (tier 2) - override it to OpenFile.
        let mut overrides = HashMap::new();
        overrides.insert((Modifiers::CTRL, key::Code::KeyN), Action::OpenFile);
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
    fn build_key_overrides_ignores_unrecognized_command_name() {
        let mut keybinds = jumppad_config::KeybindsConfig::default();
        keybinds.overrides.insert(
            "frobnicate".to_string(),
            global_hotkey::hotkey::HotKey::new(
                Some(global_hotkey::hotkey::Modifiers::CONTROL),
                global_hotkey::hotkey::Code::KeyN,
            ),
        );
        assert!(build_key_overrides(&keybinds).is_empty());
    }

    #[test]
    fn build_key_overrides_resolves_the_line_command_names() {
        // The one check that the new snake_case names survive the
        // `global_hotkey` -> iced conversion, arrow codes included.
        let keybinds: jumppad_config::KeybindsConfig = toml::from_str(
            r#"
            toggle = "control+Backquote"

            [overrides]
            delete_line = "control+alt+k"
            move_line_up = "control+alt+ArrowUp"
            copy_line_down = "control+shift+alt+ArrowDown"
            "#,
        )
        .unwrap();
        let overrides = build_key_overrides(&keybinds);
        assert_eq!(
            overrides.get(&(Modifiers::CTRL | Modifiers::ALT, key::Code::KeyK)),
            Some(&Action::DeleteLine)
        );
        assert_eq!(
            overrides.get(&(Modifiers::CTRL | Modifiers::ALT, key::Code::ArrowUp)),
            Some(&Action::MoveLineUp)
        );
        assert_eq!(
            overrides.get(&(
                Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT,
                key::Code::ArrowDown
            )),
            Some(&Action::CopyLineDown)
        );
    }

    #[test]
    fn apply_keybinds_swaps_both_override_tables_live() {
        let mut app = test_app(1);
        let keybinds: jumppad_config::KeybindsConfig = toml::from_str(
            r#"
            toggle = "control+Backquote"

            [overrides]
            new_tab = "control+alt+n"
            undo = "control+alt+z"
            "#,
        )
        .unwrap();
        app.apply_keybinds(keybinds);

        // App level: the very next key press resolves through the new table.
        assert!(matches!(
            handle_hotkey(
                Key::Character("n".into()),
                Modifiers::CTRL | Modifiers::ALT,
                key::Physical::Code(key::Code::KeyN),
                &app.keybind_overrides
            ),
            Some(Message::NewTab)
        ));
        // Editor level: the resolver every open tab reads was replaced.
        let resolved = |app: &JumpPadApp, modifiers, code, key| {
            (app.editor_config.resolver())(&jumppad_textarea::KeyPress {
                key,
                modified_key: Key::Character("z".into()),
                physical_key: key::Physical::Code(code),
                modifiers,
                text: None,
                status: jumppad_textarea::text_editor::Status::Focused {
                    is_hovered: false,
                },
            })
        };
        assert_eq!(
            resolved(
                &app,
                Modifiers::CTRL | Modifiers::ALT,
                key::Code::KeyZ,
                Key::Character("z".into())
            ),
            Some(Action::Undo)
        );

        // A revert to defaults removes the binds just as live.
        app.apply_keybinds(jumppad_config::KeybindsConfig::default());
        assert!(app.keybind_overrides.is_empty());
        assert_eq!(
            resolved(
                &app,
                Modifiers::CTRL | Modifiers::ALT,
                key::Code::KeyZ,
                Key::Character("z".into())
            ),
            None
        );
    }

    #[test]
    fn apply_config_swaps_the_theme_and_arms_a_repaint() {
        let mut app = test_app(1);
        let config = jumppad_config::Config {
            theme: "Dracula".to_string(),
            ..Default::default()
        };
        app.apply_config(config);
        assert_eq!(app.theme.to_string(), "Dracula");
        assert_eq!(app.redraw_nudge_frames, REDRAW_NUDGE_FRAMES);
        assert_eq!(app.config.theme, "Dracula");
    }

    #[test]
    fn apply_config_background_alpha_needs_a_window_born_transparent() {
        let mut app = test_app(1);
        let config = jumppad_config::Config {
            alpha: jumppad_config::AlphaConfig { background: 0.5, foreground: 1.0 },
            ..Default::default()
        };

        // Booted opaque: the surface can't turn translucent, so nothing moves.
        app.apply_config(config.clone());
        assert_eq!(app.background_alpha, 1.0);
        assert_eq!(app.editor_config.background_alpha(), 1.0);

        // Booted transparent: the window style and the editors both follow.
        app.window_transparent = true;
        app.config = jumppad_config::Config::default();
        app.apply_config(config);
        assert_eq!(app.background_alpha, 0.5);
        assert_eq!(app.editor_config.background_alpha(), 0.5);
    }

    fn language(
        name: &str,
        syntax: Option<&str>,
        extensions: &[&str],
        comment: Option<jumppad_config::CommentSyntax>,
    ) -> jumppad_config::LanguageConfig {
        jumppad_config::LanguageConfig {
            name: name.to_string(),
            syntax: syntax.map(str::to_string),
            extensions: extensions.iter().map(|ext| ext.to_string()).collect(),
            comment,
        }
    }

    #[test]
    fn apply_config_reaches_the_shared_comment_styles() {
        let mut app = test_app(1);
        let config = jumppad_config::Config {
            languages: vec![
                language(
                    "Zig",
                    Some("zig"),
                    &["zig"],
                    Some(jumppad_config::CommentSyntax::Single("// ".to_string())),
                ),
                language(
                    "HTML",
                    None,
                    &["html"],
                    Some(jumppad_config::CommentSyntax::Multi {
                        left: "<!--".to_string(),
                        right: "-->".to_string(),
                    }),
                ),
            ],
            ..Default::default()
        };
        app.apply_config(config);
        assert_eq!(
            app.editor_config.comment_styles().get("zig"),
            Some(&jumppad_textarea::CommentStyle::Single("// ".to_string()))
        );
        assert_eq!(
            app.editor_config.comment_styles().get("html"),
            Some(&jumppad_textarea::CommentStyle::Multi {
                left: "<!--".to_string(),
                right: "-->".to_string(),
            })
        );
        assert_eq!(app.redraw_nudge_frames, 0, "nothing visual changed");
    }

    #[test]
    fn a_name_only_change_applies_nothing() {
        let mut app = test_app(1);
        let before = app.editor_config.comment_styles();
        let mut config = jumppad_config::Config::default();
        config.languages[0].name = "Renamed".to_string();

        app.apply_config(config);
        // Neither derived view moved, so the setter never ran.
        assert!(Arc::ptr_eq(&before, &app.editor_config.comment_styles()));
    }

    #[test]
    fn restart_required_settings_mutate_no_live_state() {
        let mut app = test_app(1);
        let theme_before = app.theme.to_string();
        let mut config = jumppad_config::Config {
            window: jumppad_config::WindowConfig { decorations: false },
            visor: jumppad_config::VisorConfig { enabled: true },
            ..Default::default()
        };
        // A new grammar mapping without a comment style: restart-required,
        // nothing live to apply.
        config.languages.push(language("Zig", Some("zig"), &["zig"], None));

        app.apply_config(config.clone());
        assert_eq!(app.theme.to_string(), theme_before);
        assert_eq!(app.background_alpha, 1.0);
        assert_eq!(app.redraw_nudge_frames, 0, "nothing visual changed");
        assert!(!app.visor_enabled);
        // The new values still become the diff baseline, so the
        // restart-required log fires once per transition rather than on
        // every later unrelated reload.
        assert_eq!(app.config, config);
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
        assert_eq!(close_prompt(&app).expect("a close prompt").tab_id, tab0_id);
        assert!(app.close_queue.is_empty());

        let _ = app.request_close(1);
        assert_eq!(close_prompt(&app).expect("a close prompt").tab_id, tab0_id); // unchanged
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
        assert_eq!(close_prompt(&app).expect("a close prompt").tab_id, tab1_id);
    }

    #[test]
    fn key_pressed_cycles_focused_choice_both_directions_with_wraparound() {
        let mut app = test_app(1);
        app.tabs[0].dirty = true;
        let _ = app.request_close(0);
        assert_eq!(close_prompt(&app).expect("a close prompt").focused, 0);

        let _ = app.update(key_press(Named::ArrowRight, Modifiers::empty(), key::Code::ArrowRight));
        assert_eq!(close_prompt(&app).expect("a close prompt").focused, 1);

        let _ = app.update(key_press(Named::Tab, Modifiers::empty(), key::Code::Tab));
        assert_eq!(close_prompt(&app).expect("a close prompt").focused, 2);

        // Wraps back around to 0.
        let _ = app.update(key_press(Named::ArrowRight, Modifiers::empty(), key::Code::ArrowRight));
        assert_eq!(close_prompt(&app).expect("a close prompt").focused, 0);

        // Shift+Tab goes backward, wrapping to the last choice.
        let _ = app.update(key_press(Named::Tab, Modifiers::SHIFT, key::Code::Tab));
        assert_eq!(close_prompt(&app).expect("a close prompt").focused, 2);

        let _ = app.update(key_press(Named::ArrowLeft, Modifiers::empty(), key::Code::ArrowLeft));
        assert_eq!(close_prompt(&app).expect("a close prompt").focused, 1);
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

        assert!(app.modal.is_none());
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

        assert!(app.modal.is_none());
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
        assert!(app.modal.is_some());
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

    /// A fresh scratch directory per test, under the OS temp dir - the same
    /// shape `session.rs`'s tests use.
    fn argv_scratch_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("jumppad-argv-test-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_named_file_loads_into_the_startup_scratch_tab() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "hello").expect("write");

        let mut app = test_app(1);
        let id = app.tabs[0].id;
        let _ = app.update(Message::OpenPaths(vec![file.clone()]));

        assert_eq!(app.tabs.len(), 1, "no stray Untitled left behind");
        assert_eq!(app.tabs[0].id, id);
        assert_eq!(app.tabs[0].document.path.as_deref(), Some(file.as_path()));
    }

    #[test]
    fn named_files_open_in_order_with_the_last_active() {
        let dir = argv_scratch_dir();
        let first = dir.join("a.txt");
        let second = dir.join("b.txt");
        std::fs::write(&first, "a").expect("write");
        std::fs::write(&second, "b").expect("write");

        let mut app = test_app(1);
        let _ = app.update(Message::OpenPaths(vec![first.clone(), second.clone()]));

        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[0].document.path.as_deref(), Some(first.as_path()));
        assert_eq!(app.tabs[1].document.path.as_deref(), Some(second.as_path()));
        assert_eq!(app.active, 1, "the last one named is the one you land on");
    }

    #[test]
    fn naming_a_file_that_does_not_exist_opens_an_empty_buffer_for_it() {
        let dir = argv_scratch_dir();
        let file = dir.join("brand-new.md");

        let mut app = test_app(1);
        let _ = app.update(Message::OpenPaths(vec![file.clone()]));

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].document.path.as_deref(),
            Some(file.as_path()),
            "bound to the path, so the first save creates it"
        );
        assert!(app.tabs[0].editor.text().is_empty());
        assert!(!app.tabs[0].dirty, "nothing typed yet");
        assert!(app.error.is_none(), "a file you're about to create isn't an error");
    }

    #[test]
    fn naming_a_directory_surfaces_an_error_and_opens_nothing() {
        let dir = argv_scratch_dir();

        let mut app = test_app(1);
        let _ = app.update(Message::OpenPaths(vec![dir.clone()]));

        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].document.path.is_none());
        let error = app.error.expect("an error row");
        assert!(error.contains("is a folder"), "got {error:?}");
    }

    #[test]
    fn naming_an_already_open_file_focuses_its_tab() {
        let dir = argv_scratch_dir();
        let file = dir.join("restored.txt");
        std::fs::write(&file, "body").expect("write");

        let mut app = test_app(2);
        app.tabs[1].document.path = Some(file.clone());
        let _ = app.update(Message::OpenPaths(vec![file]));

        assert_eq!(app.tabs.len(), 2, "no duplicate of a restored tab");
        assert_eq!(app.active, 1);
    }

    #[test]
    fn naming_no_files_changes_nothing() {
        let mut app = test_app(1);
        let _ = app.update(Message::OpenPaths(Vec::new()));

        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].document.path.is_none());
        assert!(app.error.is_none());
    }

    /// An app whose one tab is backed by `path`, holding whatever is on disk
    /// there and stamped against it - the state every external-change test
    /// starts from. Its session dir is the scratch dir, so manifest writes
    /// stay out of the real one.
    fn app_watching(dir: &Path, path: &Path) -> JumpPadApp {
        let mut app = test_app(0);
        app.session_dir = dir.to_path_buf();
        let contents = std::fs::read_to_string(path).unwrap_or_default();
        let mut tab = Tab::from_file(0, path.to_path_buf(), &contents, &app.editor_factory);
        tab.restamp();
        app.tabs = vec![tab];
        app.next_id = 1;
        app
    }

    fn reload_message(id: u64, path: &Path, contents: &str) -> Message {
        Message::DocumentReloaded(
            id,
            path.to_path_buf(),
            DiskStamp::of(path),
            Ok(Arc::new(contents.to_string())),
        )
    }

    #[test]
    fn a_clean_tab_reloads_silently_when_its_file_changes() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "before").expect("write");
        let mut app = app_watching(&dir, &file);
        let stamped_at_open = app.tabs[0].disk;

        std::fs::write(&file, "after the change").expect("rewrite");
        let reads = app.resolve_disk_changes();

        assert_eq!(reads.len(), 1, "the clean tab is queued for a re-read");
        assert!(!app.tabs[0].dirty, "a reload is not an edit");
        assert!(!app.tabs[0].externally_changed, "nothing to warn about");
        assert_eq!(
            app.tabs[0].disk, stamped_at_open,
            "stamped when the read lands, not when it's queued"
        );

        let _ = app.update(reload_message(0, &file, "after the change"));
        assert_eq!(app.tabs[0].editor.text(), "after the change");
        assert!(!app.tabs[0].dirty);
        assert_eq!(app.tabs[0].disk, DiskStamp::of(&file));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dirty_tab_keeps_its_buffer_and_is_flagged_instead() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "before").expect("write");
        let mut app = app_watching(&dir, &file);
        app.tabs[0].dirty = true;
        app.tabs[0].editor.set_text("my unsaved edits");
        let stamp_before = app.tabs[0].disk;

        std::fs::write(&file, "someone else's version").expect("rewrite");
        let reads = app.resolve_disk_changes();

        assert!(reads.is_empty(), "unsaved edits are never overwritten");
        assert_eq!(app.tabs[0].editor.text(), "my unsaved edits");
        assert!(app.tabs[0].externally_changed);
        assert_eq!(
            app.tabs[0].disk, stamp_before,
            "the old stamp is what the save-time check compares against"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deleted_file_leaves_the_tab_open_and_dirty() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "still wanted").expect("write");
        let mut app = app_watching(&dir, &file);

        std::fs::remove_file(&file).expect("delete");
        let reads = app.resolve_disk_changes();

        assert!(reads.is_empty(), "there's nothing to read");
        assert_eq!(app.tabs.len(), 1, "the tab stays open");
        assert_eq!(app.tabs[0].editor.text(), "still wanted", "content preserved");
        assert!(app.tabs[0].dirty, "so the next save recreates the file");
        assert_eq!(app.tabs[0].disk, None);
        assert_eq!(
            app.tabs[0].document.path.as_deref(),
            Some(file.as_path()),
            "still bound to the path it will be written back to"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn our_own_save_does_not_read_as_an_external_change() {
        // The reload-loop guard: the save task stamps what it wrote, so the
        // watcher event it causes compares equal and sweeps to nothing.
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "original").expect("write");
        let mut app = app_watching(&dir, &file);
        app.tabs[0].dirty = true;

        std::fs::write(&file, "saved by jumppad").expect("the save");
        let _ = app.update(Message::FileSaved(0, Ok((file.clone(), DiskStamp::of(&file)))));

        let reads = app.resolve_disk_changes();
        assert!(reads.is_empty(), "our own write must not trigger a reload");
        assert!(!app.tabs[0].dirty);
        assert!(!app.tabs[0].externally_changed);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reload_arriving_after_the_user_typed_is_dropped_and_flags_instead() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "before").expect("write");
        let mut app = app_watching(&dir, &file);
        std::fs::write(&file, "from another program").expect("rewrite");
        assert_eq!(app.resolve_disk_changes().len(), 1);

        // The user types while the read is in flight.
        let _ = app.update(Message::Editor(0, EditorMessage::Undo));
        assert!(app.tabs[0].dirty);

        let _ = app.update(reload_message(0, &file, "from another program"));

        assert_eq!(
            app.tabs[0].editor.text(),
            "before",
            "the in-flight reload must not clobber what was just typed"
        );
        assert!(app.tabs[0].externally_changed);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reload_for_a_path_the_tab_no_longer_holds_is_dropped() {
        // Save As can retarget a tab while a read is in flight.
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "before").expect("write");
        let mut app = app_watching(&dir, &file);
        app.tabs[0].document.path = Some(dir.join("saved-as.txt"));

        let _ = app.update(reload_message(0, &file, "content of the old path"));

        assert_eq!(app.tabs[0].editor.text(), "before");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_tabs_on_one_path_both_react() {
        // `tab_index_for` stops the same file opening twice, but Save As can
        // still leave two tabs pointing at one path - the sweep visits every
        // tab, not the first match.
        let dir = argv_scratch_dir();
        let file = dir.join("shared.txt");
        std::fs::write(&file, "before").expect("write");
        let mut app = app_watching(&dir, &file);
        let mut second = Tab::from_file(1, file.clone(), "before", &app.editor_factory);
        second.restamp();
        second.dirty = true;
        app.tabs.push(second);
        app.next_id = 2;

        std::fs::write(&file, "changed by another program").expect("rewrite");
        let reads = app.resolve_disk_changes();

        assert_eq!(reads.len(), 1, "only the clean tab re-reads");
        assert_eq!(reads[0].0, 0);
        assert!(app.tabs[1].externally_changed, "the dirty one is flagged");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_over_a_changed_file_reports_a_conflict() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "as opened").expect("write");
        let opened = SaveExpectation::Seen(DiskStamp::of(&file));

        assert!(!conflicts(&file, opened), "nothing moved");
        assert!(
            !conflicts(&file, SaveExpectation::Unchecked),
            "a Save As target has nothing to compare against"
        );

        std::fs::write(&file, "changed by another program").expect("rewrite");
        assert!(conflicts(&file, opened), "the file moved under the tab");
        assert!(
            !conflicts(&file, SaveExpectation::Unchecked),
            "and Unchecked still writes over it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_appeared_where_the_tab_saw_none_is_a_conflict() {
        // `jumppad newnote.md` on a file that doesn't exist yet, and then
        // something else creates it. The buffer has never seen those bytes,
        // so a save would clobber them.
        let dir = argv_scratch_dir();
        let file = dir.join("newnote.md");
        let saw_nothing = SaveExpectation::Seen(None);
        assert!(!conflicts(&file, saw_nothing), "nothing there yet");

        std::fs::write(&file, "created by another program").expect("write");
        assert!(conflicts(&file, saw_nothing));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deleted_file_is_not_a_conflict_to_save_over() {
        // The delete rule: there is nothing to clobber, so the save just
        // recreates the file - no prompt, whatever the tab last saw.
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "as opened").expect("write");
        let opened = SaveExpectation::Seen(DiskStamp::of(&file));

        std::fs::remove_file(&file).expect("delete");
        assert!(!conflicts(&file, opened));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A dirty tab whose file changed underneath it, with the conflict
    /// already surfaced - the state both dialog and banner start from.
    fn app_in_conflict(dir: &Path, file: &Path) -> JumpPadApp {
        std::fs::write(file, "as opened").expect("write");
        let mut app = app_watching(dir, file);
        app.tabs[0].dirty = true;
        app.tabs[0].editor.set_text("my unsaved edits");
        std::fs::write(file, "changed by another program").expect("rewrite");
        app.resolve_disk_changes();
        assert!(app.tabs[0].externally_changed);
        app
    }

    #[test]
    fn a_conflicted_save_prompts_instead_of_writing() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        app.tabs[0].externally_changed = false; // the sweep hasn't run yet

        let _ = app.update(Message::FileSaved(
            0,
            Err(SaveError::Conflict),
        ));

        assert_eq!(conflict_prompt(&app).expect("a conflict prompt").tab_id, 0);
        assert!(app.tabs[0].externally_changed, "the save is how it found out");
        assert!(!app.file_dialog_active);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "changed by another program",
            "nothing was written"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwrite_writes_and_clears_the_flag() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        let _ = app.update(Message::FileSaved(
            0,
            Err(SaveError::Conflict),
        ));

        let _ = app.update(Message::ConflictResolved(0, ConflictDecision::Overwrite));
        assert!(app.modal.is_none(), "the prompt is dismissed");

        // The re-run save lands the way any other successful save does.
        std::fs::write(&file, "my unsaved edits").expect("the overwrite");
        let _ = app.update(Message::FileSaved(0, Ok((file.clone(), DiskStamp::of(&file)))));

        assert!(!app.tabs[0].dirty);
        assert!(!app.tabs[0].externally_changed);
        assert_eq!(app.tabs[0].disk, DiskStamp::of(&file));
        assert!(
            app.resolve_disk_changes().is_empty(),
            "and the overwrite doesn't read back as an external change"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discard_and_reload_replaces_the_buffer_and_goes_clean() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);

        let _ = app.update(Message::ConflictResolved(0, ConflictDecision::DiscardAndReload));
        let _ = app.update(Message::ConflictReloaded(
            0,
            file.clone(),
            DiskStamp::of(&file),
            Ok(Arc::new("changed by another program".to_string())),
        ));

        assert_eq!(app.tabs[0].editor.text(), "changed by another program");
        assert!(!app.tabs[0].dirty, "the buffer matches disk again");
        assert!(!app.tabs[0].externally_changed);
        assert_eq!(app.tabs[0].disk, DiskStamp::of(&file));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_a_conflict_changes_nothing() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        let _ = app.update(Message::FileSaved(
            0,
            Err(SaveError::Conflict),
        ));

        let _ = app.update(Message::ConflictResolved(0, ConflictDecision::Cancel));

        assert!(app.modal.is_none());
        assert!(app.tabs[0].dirty, "still unsaved");
        assert!(app.tabs[0].externally_changed, "still flagged");
        assert_eq!(app.tabs[0].editor.text(), "my unsaved edits");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "changed by another program"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keep_mine_restamps_so_the_next_save_does_not_prompt() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);

        let _ = app.update(Message::AcknowledgeExternalChange(0));

        assert!(!app.tabs[0].externally_changed);
        assert!(app.tabs[0].dirty, "the edits are still unsaved");
        assert_eq!(app.tabs[0].editor.text(), "my unsaved edits");
        assert!(
            !conflicts(&file, SaveExpectation::Seen(app.tabs[0].disk)),
            "acknowledging records the version seen, so the next save goes through"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conflict_during_a_close_prompt_save_aborts_the_close() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        // The "Save" branch of the unsaved-changes prompt.
        let _ = app.request_close(0);
        let _ = app.update(Message::CloseConfirmed(0, CloseDecision::Save));
        assert_eq!(app.pending_close_after_save, vec![0]);

        let _ = app.update(Message::FileSaved(
            0,
            Err(SaveError::Conflict),
        ));

        assert_eq!(app.tabs.len(), 1, "the tab stays open");
        assert!(
            app.pending_close_after_save.is_empty(),
            "a failed save cancels the close it was for"
        );
        assert!(conflict_prompt(&app).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conflict_waits_behind_the_close_prompt() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        let mut other = Tab::untitled(1, &app.editor_factory);
        other.dirty = true;
        app.tabs.push(other);
        app.next_id = 2;

        let _ = app.request_close(1);
        assert!(close_prompt(&app).is_some());

        // The conflict arrives while that prompt is up - queued, not stacked.
        let _ = app.update(Message::FileSaved(
            0,
            Err(SaveError::Conflict),
        ));
        assert!(close_prompt(&app).is_some(), "the close prompt keeps the screen");
        assert_eq!(app.conflict_queue, vec![0]);

        let _ = app.update(Message::CloseConfirmed(1, CloseDecision::Cancel));
        assert_eq!(conflict_prompt(&app).expect("queued conflict").tab_id, 0);
        assert!(app.conflict_queue.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_conflict_prompt_navigates_like_the_close_prompt() {
        // Both dialogs share one keyboard path - three choices, cycled mod 3,
        // Enter resolving whichever is focused and Escape always cancelling.
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        let _ = app.update(Message::FileSaved(
            0,
            Err(SaveError::Conflict),
        ));
        assert_eq!(
            conflict_prompt(&app).unwrap().focused,
            2,
            "opens on Cancel - the other two both destroy someone's work"
        );

        let _ = app.update(key_press(Named::ArrowRight, Modifiers::empty(), key::Code::ArrowRight));
        assert_eq!(conflict_prompt(&app).unwrap().focused, 0, "wraps forward");
        let _ = app.update(key_press(Named::Tab, Modifiers::SHIFT, key::Code::Tab));
        assert_eq!(conflict_prompt(&app).unwrap().focused, 2, "and backward");

        // Focus "Discard & Reload" and take it.
        let _ = app.update(key_press(Named::ArrowLeft, Modifiers::empty(), key::Code::ArrowLeft));
        let _ = app.update(key_press(Named::Enter, Modifiers::empty(), key::Code::Enter));
        assert!(app.modal.is_none());
        let _ = app.update(Message::ConflictReloaded(
            0,
            file.clone(),
            DiskStamp::of(&file),
            Ok(Arc::new("changed by another program".to_string())),
        ));
        assert_eq!(app.tabs[0].editor.text(), "changed by another program");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn escape_cancels_the_conflict_prompt_whatever_is_focused() {
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        let _ = app.update(Message::FileSaved(
            0,
            Err(SaveError::Conflict),
        ));
        // Focused on "Overwrite" - Escape must still cancel.
        let _ = app.update(key_press(Named::Escape, Modifiers::empty(), key::Code::Escape));

        assert!(app.modal.is_none());
        assert!(app.tabs[0].externally_changed);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "changed by another program"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_changed_on_disk_bar_is_inert_while_a_modal_is_up() {
        // The scrim only swallows clicks the widget tree routes through it,
        // and the bar sits underneath - so its two messages have to turn
        // themselves away, the way `Editor` and `FileDropped` do.
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        let mut other = Tab::untitled(1, &app.editor_factory);
        other.dirty = true;
        app.tabs.push(other);
        let _ = app.request_close(1);
        let stamp_before = app.tabs[0].disk;

        let _ = app.update(Message::AcknowledgeExternalChange(0));
        assert!(app.tabs[0].externally_changed, "still flagged");
        assert_eq!(app.tabs[0].disk, stamp_before, "and not re-stamped");

        let _ = app.update(Message::ReloadFromDisk(0));
        assert_eq!(app.tabs[0].editor.text(), "my unsaved edits");
        assert!(close_prompt(&app).is_some(), "the close prompt is still up");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_queued_prompt_that_went_clean_does_not_strand_the_rest() {
        // `request_close` closes a clean tab outright instead of prompting,
        // so draining has to keep going rather than assume a dialog opened.
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        let mut queued = Tab::untitled(1, &app.editor_factory);
        queued.dirty = true;
        let mut showing = Tab::untitled(2, &app.editor_factory);
        showing.dirty = true;
        app.tabs.push(queued);
        app.tabs.push(showing);
        app.next_id = 3;

        let _ = app.request_close(2); // this one gets the prompt
        let _ = app.request_close(1); // queued behind it
        let _ = app.update(Message::FileSaved(0, Err(SaveError::Conflict))); // queued too
        assert_eq!(app.close_queue, vec![1]);
        assert_eq!(app.conflict_queue, vec![0]);

        // Tab 1 goes clean while it waits - closing it must not swallow the
        // conflict waiting behind it.
        app.tabs[1].dirty = false;
        let _ = app.update(Message::CloseConfirmed(2, CloseDecision::Cancel));

        assert!(app.close_queue.is_empty());
        assert!(app.conflict_queue.is_empty());
        assert!(!app.tabs.iter().any(|tab| tab.id == 1), "the clean tab closed");
        assert_eq!(
            conflict_prompt(&app).expect("the queued conflict still shows").tab_id,
            0
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_file_leaves_its_tab_alone() {
        // Only NotFound means "deleted". A read that failed for any other
        // reason must not dirty a tab the user never touched.
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "before").expect("write");
        let mut app = app_watching(&dir, &file);
        let stamp = app.tabs[0].disk;

        let _ = app.update(Message::DocumentReloaded(
            0,
            file.clone(),
            DiskStamp::of(&file),
            Err(std::io::ErrorKind::InvalidData),
        ));

        assert!(!app.tabs[0].dirty, "a failed read is not an edit");
        assert_eq!(app.tabs[0].disk, stamp);
        assert_eq!(app.tabs[0].editor.text(), "before");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reload_refreshes_the_find_state_of_the_tab_that_moved() {
        // The reloaded tab's match list holds ranges into text that just
        // went away - and it isn't necessarily the active tab.
        let mut app = app_with_text(&["active tab", "background tab"]);
        let dir = argv_scratch_dir();
        let file = dir.join("background.txt");
        std::fs::write(&file, "one two two").expect("write");
        app.session_dir = dir.clone();
        app.tabs[1].document.path = Some(file.clone());
        app.tabs[1].editor.set_text("one two two");
        app.tabs[1].restamp();

        // A palette open on the background tab, matching twice.
        let _ = app.update(Message::SelectTab(1));
        let _ = app.update(Message::OpenFind);
        let _ = app.update(Message::FindQueryChanged("two".into()));
        let _ = app.update(Message::SelectTab(0));
        assert_eq!(app.find[&app.tabs[1].id].matches.len(), 2);

        let _ = app.update(reload_message(app.tabs[1].id, &file, "one"));

        assert_eq!(
            app.find[&app.tabs[1].id].matches.len(),
            0,
            "the background tab's matches were re-searched against the new text"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restored_tab_is_stamped_before_its_re_read_is_queued() {
        // Otherwise a focus sweep arriving first sees a clean, stamp-less
        // tab and reloads the file over the empty buffer it starts with -
        // one Ctrl+Z away from blanking the document.
        let dir = argv_scratch_dir();
        let file = dir.join("restored.txt");
        std::fs::write(&file, "on disk").expect("write");

        let mut app = test_app(0);
        app.session_dir = dir.clone();
        let mut tab = Tab::restored(0, Some(file.clone()), "", false, &app.editor_factory);
        tab.restamp();
        app.tabs = vec![tab];

        assert!(
            app.resolve_disk_changes().is_empty(),
            "a sweep before the re-read lands must find nothing to do"
        );
        assert_eq!(app.tabs[0].editor.text(), "", "the buffer is left for the re-read");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flagged_tab_says_so_in_its_title() {
        let factory = stub_factory();
        let mut tab = Tab::from_file(0, PathBuf::from("/tmp/notes.txt"), "", &factory);
        assert_eq!(tab.title(), "notes.txt");
        tab.dirty = true;
        assert_eq!(tab.title(), "notes.txt \u{2022}");
        tab.externally_changed = true;
        assert_eq!(tab.title(), "notes.txt \u{2022} \u{26a0}");
    }

    #[test]
    fn the_overwrite_setting_skips_the_conflict_check() {
        // Read from `self.config` at save time, so a reloaded config.toml
        // takes effect without an `apply_config` arm.
        let dir = argv_scratch_dir();
        let file = dir.join("notes.txt");
        let mut app = app_in_conflict(&dir, &file);
        let tab = &app.tabs[0];
        assert!(app.config.files.save_conflict_resolution.asks());
        assert_eq!(
            app.save_expectation(tab, false),
            SaveExpectation::Seen(tab.disk),
            "the default setting checks against what the tab last saw"
        );
        assert_eq!(
            app.save_expectation(tab, true),
            SaveExpectation::Unchecked,
            "a Save As dialog asks its own overwrite question"
        );

        app.config.files.save_conflict_resolution =
            jumppad_config::SaveConflictResolution::Overwrite;
        assert_eq!(
            app.save_expectation(&app.tabs[0], false),
            SaveExpectation::Unchecked,
            "saves always win under the overwrite setting"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_watched_path_list_is_sorted_deduped_and_file_backed_only() {
        // It's the watcher subscription's identity: an unstable order would
        // tear the watcher down and rebuild it on every unrelated change.
        let mut app = test_app(3);
        app.tabs[0].document.path = Some(PathBuf::from("/tmp/zebra.txt"));
        app.tabs[1].document.path = Some(PathBuf::from("/tmp/apple.txt"));
        app.tabs[2].document.path = Some(PathBuf::from("/tmp/zebra.txt"));

        assert_eq!(
            app.watched_paths(),
            vec![PathBuf::from("/tmp/apple.txt"), PathBuf::from("/tmp/zebra.txt")]
        );

        app.tabs[0].document.path = None;
        app.tabs[2].document.path = None;
        assert_eq!(app.watched_paths(), vec![PathBuf::from("/tmp/apple.txt")]);
    }

    #[test]
    fn an_untitled_tab_is_never_swept() {
        let mut app = test_app(1);
        assert!(app.resolve_disk_changes().is_empty());
        assert!(!app.tabs[0].dirty);
        assert!(app.watched_paths().is_empty());
    }

    #[test]
    fn hovering_files_over_the_window_toggles_the_drop_overlay() {
        let mut app = test_app(1);
        let _ = app.update(Message::FilesHovered(true));
        assert!(app.files_hovered);
        let _ = app.update(Message::FilesHovered(false));
        assert!(!app.files_hovered);
    }

    #[test]
    fn dropping_a_file_clears_the_hover_overlay() {
        // Windows and macOS send no `FilesHoveredLeft` after a completed drop,
        // so the drop itself has to dismiss the overlay.
        let mut app = test_app(1);
        app.files_hovered = true;
        let _ = app.update(Message::FileDropped(PathBuf::from("/tmp/dropped.txt")));
        assert!(!app.files_hovered);
    }

    #[test]
    fn dropping_onto_an_empty_scratch_tab_loads_into_it() {
        let mut app = test_app(1);
        let id = app.tabs[0].id;
        let _ = app.update(Message::DroppedFileRead(Ok((
            PathBuf::from("/tmp/dropped.txt"),
            Arc::new("body".to_string()),
        ))));

        assert_eq!(app.tabs.len(), 1, "no stray Untitled left behind");
        assert_eq!(app.tabs[0].id, id, "the tab keeps its id");
        assert_eq!(
            app.tabs[0].document.path.as_deref(),
            Some(Path::new("/tmp/dropped.txt"))
        );
    }

    #[test]
    fn dropping_onto_a_file_backed_tab_opens_a_new_one() {
        let mut app = test_app(1);
        app.tabs[0].document.path = Some(PathBuf::from("/tmp/already-open.txt"));
        let _ = app.update(Message::DroppedFileRead(Ok((
            PathBuf::from("/tmp/dropped.txt"),
            Arc::new("body".to_string()),
        ))));

        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, 1);
        assert_eq!(
            app.tabs[1].document.path.as_deref(),
            Some(Path::new("/tmp/dropped.txt"))
        );
    }

    #[test]
    fn dropping_an_already_open_file_focuses_its_tab() {
        let mut app = test_app(2);
        app.tabs[1].document.path = Some(PathBuf::from("/tmp/already-open.txt"));
        let _ = app.update(Message::FileDropped(PathBuf::from("/tmp/already-open.txt")));

        assert_eq!(app.tabs.len(), 2, "no duplicate tab");
        assert_eq!(app.active, 1);
    }

    #[test]
    fn dropping_a_file_leaves_an_in_flight_dialog_alone() {
        let mut app = test_app(1);
        app.file_dialog_active = true;
        let _ = app.update(Message::FileDropped(PathBuf::from("/tmp/dropped.txt")));
        assert!(app.file_dialog_active);

        let _ = app.update(Message::DroppedFileRead(Ok((
            PathBuf::from("/tmp/dropped.txt"),
            Arc::new("body".to_string()),
        ))));
        assert!(app.file_dialog_active);
    }

    #[test]
    fn the_unsaved_changes_prompt_turns_drops_away() {
        let mut app = test_app(1);
        app.modal = Some(Modal::Close(PendingClose {
            tab_id: app.tabs[0].id,
            title: "Untitled".into(),
            focused: 0,
        }));
        app.files_hovered = true;
        let _ = app.update(Message::FileDropped(PathBuf::from("/tmp/dropped.txt")));

        assert!(!app.files_hovered, "the overlay still clears");
        assert!(app.tabs[0].document.path.is_none(), "nothing was opened");
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn an_unreadable_dropped_file_surfaces_an_error() {
        let mut app = test_app(1);
        let _ = app.update(Message::DroppedFileRead(Err(OpenError::Io {
            path: PathBuf::from("/tmp/dropped.bin"),
            kind: std::io::ErrorKind::InvalidData,
        })));

        assert!(app.error.is_some());
        assert_eq!(app.tabs.len(), 1);
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
        let _ = app.update(Message::FileSaved(id, Ok((PathBuf::from("/tmp/x"), None))));
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

    // The scrollbar thumb and the find palette used to be required to share
    // one material (an invariant test lived here). The thumb now washes
    // toward white on dark themes and toward black on light ones, while the
    // find palette keeps washing toward black always - an intentional
    // divergence, so that invariant no longer holds.

    #[test]
    fn scrollbar_thumb_wash_lightens_dark_themes_and_darkens_light_ones() {
        let mut checked_dark = 0;
        let mut checked_light = 0;
        for theme in Theme::ALL {
            let base = theme.extended_palette().background.base.color;
            let washed = jumppad_textarea::scrollbar_thumb_style(theme);
            let composited_luminance = luminance(composite(base, washed));
            if theme.extended_palette().is_dark {
                assert!(
                    composited_luminance >= luminance(base),
                    "{theme}: dark theme's thumb wash didn't lighten it"
                );
                checked_dark += 1;
            } else {
                assert!(
                    composited_luminance <= luminance(base),
                    "{theme}: light theme's thumb wash didn't darken it"
                );
                checked_light += 1;
            }
        }
        assert!(
            checked_dark > 0 && checked_light > 0,
            "need themes of both kinds to prove the branch runs"
        );
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
    fn premultiply_scales_encoded_rgb_by_alpha() {
        // White premultiplied is the alpha itself, per channel - directly on
        // the encoded values, with no linear round-trip to brighten them.
        let color = premultiply(Color { a: 0.25, ..Color::WHITE });
        assert_eq!(color.r, 0.25);
        assert_eq!(color.g, 0.25);
        assert_eq!(color.b, 0.25);
        assert_eq!(color.a, 0.25, "alpha must survive untouched");
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
    fn a_light_theme_at_low_alpha_stays_see_through_once_premultiplied() {
        // The reported symptom, as arithmetic. A compositor doing
        // `src_rgb + (1 - a) * desktop` over a *straight* white background
        // computes `1.0 + anything` on every channel - saturated, opaque, and
        // completely insensitive to alpha, which is why a light theme looked
        // solid even at 0.1 while a dark one looked fine. Premultiplied, the
        // desktop keeps its full `1 - a` share of the result.
        let alpha = 0.1;
        let straight = Color { a: alpha, ..Color::WHITE };
        let premultiplied = premultiply(straight);

        let over_desktop = |src: Color, desktop: f32| src.r + (1.0 - src.a) * desktop;

        // Straight: black desktop and white desktop composite identically.
        assert_eq!(over_desktop(straight, 0.0), over_desktop(straight, 1.0) - 0.9);
        assert!(over_desktop(straight, 0.0) >= 1.0, "already saturated before the desktop is added");

        // Premultiplied: the window contributes its 0.1 and the desktop the rest.
        assert!((over_desktop(premultiplied, 0.0) - 0.1).abs() < 1e-6);
        assert!((over_desktop(premultiplied, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_translucent_window_hands_the_compositor_whatever_the_gate_asks_for() {
        // The two binaries have to agree on screen, so `style` is the single
        // place the premultiply decision is made - not a per-platform branch
        // that can drift. Asserted against the gate rather than a literal so
        // this test says the same thing on every host it runs on.
        let mut app = test_app(1);
        app.background_alpha = 0.5;
        let theme = app.theme();

        let scaled = iced::theme::default(&theme).background_color.scale_alpha(0.5);
        let expected = if CLEAR_COLOR_NEEDS_PREMULTIPLY { premultiply(scaled) } else { scaled };

        assert_eq!(app.style(&theme).background_color, expected);
    }

    #[test]
    fn premultiplying_leaves_the_configured_alpha_alone() {
        // Whatever the RGB does, the alpha channel *is* the transparency the
        // user configured - premultiplying must not eat into it.
        let mut app = test_app(1);
        app.background_alpha = 0.5;
        assert!((app.style(&app.theme()).background_color.a - 0.5).abs() < 1e-6);
    }

    #[test]
    fn an_opaque_window_is_never_premultiplied() {
        // At alpha 1.0 the window isn't transparent at all and premultiplying
        // would be a no-op anyway - but the branch is skipped outright, so a
        // solid window renders identically on both backends.
        let app = test_app(1);
        let theme = app.theme();
        assert_eq!(
            app.style(&theme).background_color,
            iced::theme::default(&theme).background_color
        );
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
