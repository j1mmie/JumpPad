use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use editor_core::{
    DiskStamp, EditorFactory, EditorMessage, FLOATING_SURFACE_DARKEN,
    SavedSelection, SelectionKind, Tab, darkening_wash,
};
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use iced::advanced::widget::{Id, operate, operation};
use iced::keyboard::key;
use iced::widget::{
    Text, button, center, column, container, keyed_column, mouse_area, row,
    scrollable, stack, text, text_input,
};
use iced::{
    Border, Center, Color, Element, Fill, Font, Padding, Pixels, Point, Right,
    Subscription, Task, Theme, Top, keyboard,
};

use jumppad_actions::{Action, Context};
use jumppad_config::Appearance;

use crate::docwatch;
use crate::find::FindState;
use crate::hotkey::{self, Hotkey};
use crate::reload;
use crate::session;
use crate::visor::{self, Animation};
use crate::window;

/// The face the icon glyphs are drawn in, selected by the family name the
/// font records. `run` registers [`ICON_FONT_BYTES`] at startup.
const ICON_FONT: Font = Font::with_name(jumppad_icons::FAMILY_NAME);

/// The font `cargo build_fonts` writes, embedded for `run` to hand to iced.
pub(crate) const ICON_FONT_BYTES: &[u8] =
    include_bytes!("../../../assets/fonts/jumppad-icons.ttf");

// The font is kept in Git LFS, and a clone made without it leaves a short
// text pointer at that path instead of the file. Nothing downstream would
// say so: iced declines a face it cannot parse, `.notdef` is empty on
// purpose, and every icon draws as nothing at all. Checking the sfnt
// version tag lets the build fail with a sentence instead.
//
// It belongs here rather than in `jumppad_icons` because `build_fonts`
// reads that manifest to write this very file, and a check there would stop
// the tool that fixes the problem from compiling.
const _: () = assert!(
    ICON_FONT_BYTES.len() > 4
        && ICON_FONT_BYTES[0] == 0x00
        && ICON_FONT_BYTES[1] == 0x01
        && ICON_FONT_BYTES[2] == 0x00
        && ICON_FONT_BYTES[3] == 0x00,
    "assets/fonts/jumppad-icons.ttf is a Git LFS pointer rather than a font - run `git lfs pull`"
);

/// How much larger an icon is drawn than the text it sits among. An icon's
/// ink fills a little over half its em box and a letter's fills about half,
/// so the two come out close at the matched size this is set to; raising it
/// brings an icon up toward the height of a capital instead. Only the size
/// scales - the line height stays the text's, so the tab strip keeps its
/// whole-pixel height whatever this is.
const ICON_SCALE: f32 = 1.0;

/// The space above and below a tab's title, and so half of what sets the
/// tab strip's height. Everything else standing in the strip matches it
/// rather than choosing its own, so the strip keeps one straight edge.
const TAB_VERTICAL_PADDING: f32 = 6.0;

const VISOR_ANIM_TICK: Duration = Duration::from_millis(16);
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(5);

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
    operate(operation::focusable::focus(Id::new(
        editor_core::EDITOR_WIDGET_ID,
    )))
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
    /// Which theme slot is showing, and what the OS last said. The OS's
    /// answer is kept even while `detection` ignores it, so switching back
    /// to `auto` resolves immediately instead of waiting for the OS to
    /// change again.
    showing: Appearance,
    os_appearance: Option<Appearance>,
    /// All four are written only by `apply_theme`, from the theme the
    /// config and `showing` resolve to.
    theme: Theme,
    ui_text: UiText,
    background_alpha: f32,
    background_blur: jumppad_config::Blur,
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
            Modal::SaveConflict(pending) => Message::ConflictResolved(
                pending.tab_id,
                ConflictDecision::Cancel,
            ),
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
    ConflictReloaded(
        u64,
        PathBuf,
        Option<DiskStamp>,
        Result<Arc<String>, std::io::ErrorKind>,
    ),
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
    /// What the OS's light/dark setting is: once at startup, and again
    /// whenever it changes. Acted on only while `[mode] detection` is
    /// `auto`, but always listened for, so turning `auto` back on doesn't
    /// wait for the OS to change again.
    SystemAppearanceReported(iced::theme::Mode),
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
    pub fn new(
        config: jumppad_config::Config,
        paths: Vec<PathBuf>,
    ) -> (Self, Task<Message>) {
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

        // Opaque and unstyled to start with: `apply_theme` below writes
        // every appearance setting, so there is one writer rather than two
        // that have to agree.
        let editor_config = jumppad_textarea::SharedEditorConfig::new(
            1.0,
            build_key_resolver(keybind_overrides.clone()),
        );
        editor_config.set_scroll_sensitivity(config.scroll.sensitivity);
        editor_config.set_drag_speed(config.scroll.drag_speed);
        editor_config.set_undo_depth(config.history.depth);
        editor_config.set_comment_styles(build_comment_styles(&config));
        editor_config.set_indentation(build_indentation(&config));
        editor_config.set_word_separators(
            jumppad_textarea::WordSeparators::new(&config.words.separators),
        );

        let registry = syntax_registry::SyntaxRegistry::new(
            search_dirs,
            config.extension_to_grammar(),
            || {},
        );
        // Which `TextEditorWidget` implementation new tabs are created with.
        let editor_factory: EditorFactory =
            Box::new(jumppad_textarea::TextArea::factory(
                registry,
                editor_config.clone(),
            ));

        let session_candidates = session::candidate_dirs();
        let session_dir = session_candidates
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("drafts"));
        let manifest = session::load_manifest(&session_candidates);

        // Nothing to ask yet: iced answers `system::theme()` as a task, so
        // the OS's setting arrives a message later. Until it does the light
        // slot stands - see the boot batch below.
        let os_appearance = None;
        let showing = config.mode.showing(os_appearance);

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
            showing,
            os_appearance,
            // Placeholders until the `apply_theme` below, which is what
            // actually resolves them.
            theme: Theme::Light,
            ui_text: ui_text(&jumppad_config::ResolvedFont::default()),
            background_alpha: 1.0,
            background_blur: jumppad_config::DEFAULT_BLUR,
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
            editor_config,
            config_watch: reload::ConfigWatch::new(),
            document_watch: docwatch::DocumentWatch::new(),
            config,
            keybinds,
        };

        // The task is dropped rather than batched: there is no window yet
        // to tell anything to, and `WindowReady` tells the one that arrives.
        let _ = app.apply_theme();
        // Both asked once, here. Later changes to either arrive through
        // `subscription` instead.
        let boot_tasks = Task::batch([
            iced::window::latest().map(Message::WindowReady),
            iced::system::theme().map(Message::SystemAppearanceReported),
        ]);

        let Some(manifest) =
            manifest.filter(|manifest| !manifest.tabs.is_empty())
        else {
            let task = app.new_tab();
            return (app, Task::batch([task, boot_tasks, argv_task]));
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
                        reload_tasks.push(Task::perform(
                            reload_from_disk(path),
                            move |result| {
                                Message::SessionFileLoaded(id, result)
                            },
                        ));
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
                    reload_tasks.push(Task::perform(
                        reload_from_disk(path.clone()),
                        move |result| Message::SessionFileLoaded(id, result),
                    ));
                }
                app.tabs.push(tab);
            } else {
                app.tabs.push(Tab::untitled(entry.id, &app.editor_factory));
            }
        }

        if app.tabs.is_empty() {
            let task = app.new_tab();
            return (app, Task::batch([task, boot_tasks, argv_task]));
        }

        app.next_id = manifest
            .tabs
            .iter()
            .map(|entry| entry.id)
            .max()
            .map(|max_id| max_id + 1)
            .unwrap_or(0);

        let desired_active = manifest.active.min(app.tabs.len() - 1);
        // No focus task alongside it: `WindowReady` focuses whatever this
        // leaves active, once the window it belongs to exists.
        let switch_task = app.switch_active(desired_active);
        let task = Task::batch(reload_tasks.into_iter().chain([
            switch_task,
            boot_tasks,
            argv_task,
        ]));
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
        // The very first tab, or the stand-in for a closed last one: the
        // index didn't move, so `switch_active` would call it a no-op, but
        // the editor sitting at it is brand new and still needs focus.
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
                Ok(contents) => {
                    tasks.push(self.open_loaded_file(path, &contents))
                }
                // Naming a file that isn't there yet is how you start one -
                // an empty tab, saved to that path on the first save.
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    tasks.push(self.open_loaded_file(path, ""))
                }
                Err(err) => {
                    problems.push(format!("{}: {}", path.display(), err.kind()))
                }
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
        self.tabs.iter().position(|tab| {
            tab.id == id && tab.document.path.as_deref() == Some(path)
        })
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
    fn open_loaded_file(
        &mut self,
        path: PathBuf,
        contents: &str,
    ) -> Task<Message> {
        let scratch = self.tabs.get(self.active).is_some_and(|tab| {
            tab.document.path.is_none()
                && !tab.dirty
                && tab.editor.text().is_empty()
        });
        if !scratch {
            let id = self.next_id;
            self.next_id += 1;
            let mut tab =
                Tab::from_file(id, path, contents, &self.editor_factory);
            tab.restamp();
            self.tabs.push(tab);
            return self.switch_active(self.tabs.len() - 1);
        }

        // Rebuilt rather than `set_text`: the factory takes the extension, and
        // that's what picks the grammar - an editor made for an untitled buffer
        // would stay unhighlighted. The id carries over, since the session
        // manifest and `find` are keyed by it.
        let id = self.tabs[self.active].id;
        self.tabs[self.active] =
            Tab::from_file(id, path, contents, &self.editor_factory);
        self.tabs[self.active].restamp();
        // The index didn't move, so `switch_active` would call it a no-op -
        // the same situation `new_tab` handles above.
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
                if let Some(index) =
                    self.tabs.iter().position(|tab| tab.id == next_id)
                {
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
            tab.editor
                .set_find_matches(state.matches.clone(), state.current);
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
            tab.editor
                .set_find_matches(state.matches.clone(), state.current);
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
            tab.editor
                .set_find_matches(state.matches.clone(), state.current);
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
        if let Some(tab) = self.tabs.get_mut(index) {
            let (line, column) = tab.last_cursor;
            match tab.last_selection {
                Some(selection) => {
                    tab.editor.restore_selection(selection, tab.last_cursor)
                }
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

    /// Pins the window's appearance to the slot the config names, or clears
    /// it while the config is following the OS - a pinned appearance is
    /// precisely what stops the OS from being heard (see
    /// `macos::pin_appearance`) - and takes the OS's own answer on the way
    /// past. A session that spent time pinned has heard nothing from the OS
    /// in the meantime, so that answer is stale exactly when `auto` comes
    /// back on.
    #[cfg(target_os = "macos")]
    fn sync_window_appearance(&self) -> Task<Message> {
        let Some(id) = self.window else {
            return Task::none();
        };
        let pinned = self.config.mode.pinned();

        iced::window::run(id, move |window| {
            crate::macos::pin_appearance(window, pinned);
            crate::macos::system_appearance()
        })
        // Travelling as the runtime's own report, so a read and a switch
        // arrive by the same road.
        .map(|appearance| match appearance {
            Some(Appearance::Light) => iced::theme::Mode::Light,
            Some(Appearance::Dark) => iced::theme::Mode::Dark,
            None => iced::theme::Mode::None,
        })
        .map(Message::SystemAppearanceReported)
    }

    /// Nothing to do elsewhere: no other platform lets a pinned appearance
    /// silence the OS, and winit reports the switch on its own.
    #[cfg(not(target_os = "macos"))]
    fn sync_window_appearance(&self) -> Task<Message> {
        Task::none()
    }

    /// Frosts the desktop showing through the window the way the theme's
    /// `background.blur` asks. Each platform is asked its own way, and each
    /// reads only the forms it can act on - Windows its two acrylics, macOS a
    /// radius - so what the other platform's forms mean here is nothing.
    ///
    /// Only a translucent window is told anything, on either. A solid one
    /// has no desktop showing through to frost, and on Windows the off case
    /// also overrides a backdrop winit already asked DWM for, which would be
    /// a gratuitous difference from every other app on a window nobody can
    /// see through (see `windows.rs`).
    #[cfg(target_os = "windows")]
    fn apply_window_blur(&self) -> Task<Message> {
        let blur = self.background_blur;
        match self.window {
            Some(id) if self.background_alpha < 1.0 => {
                iced::window::run(id, move |window| {
                    crate::windows::set_system_backdrop(window, blur);
                })
                .discard()
            }
            _ => Task::none(),
        }
    }

    /// Same gate as above, the window server's own blur behind it. Only the
    /// radius travels, and the acrylics have none: a window-server blur has
    /// no focus to lose the frost to, so the distinction they draw is one
    /// macOS has no way to be asked about.
    #[cfg(target_os = "macos")]
    fn apply_window_blur(&self) -> Task<Message> {
        let radius = self.background_blur.radius();
        match self.window {
            Some(id) if self.background_alpha < 1.0 => {
                iced::window::run(id, move |window| {
                    crate::macos::set_window_blur(window, radius);
                })
                .discard()
            }
            _ => Task::none(),
        }
    }

    /// No blur to ask for anywhere else: X11 and Wayland leave it to the
    /// compositor, whether through its own window rules or a protocol
    /// extension, and neither is reachable through what iced exposes.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn apply_window_blur(&self) -> Task<Message> {
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
    fn reload_settled_configs(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for file in self.config_watch.settled(Instant::now()) {
            let result = match file {
                reload::WatchedFile::Config => jumppad_config::try_load()
                    .map(|config| tasks.push(self.apply_config(config))),
                reload::WatchedFile::Keybinds => {
                    jumppad_config::try_load_keybinds()
                        .map(|keybinds| self.apply_keybinds(keybinds))
                }
            };
            if let Err(err) = result {
                log::warn!("couldn't reload {}: {err}", file.name());
                self.error = Some(format!(
                    "{}: {err} - keeping the previous settings",
                    file.name()
                ));
            }
        }

        Task::batch(tasks)
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
        let reads =
            self.resolve_disk_changes()
                .into_iter()
                .map(|(id, path, stamp)| {
                    Task::perform(
                        reload_from_disk(path.clone()),
                        move |result| {
                            Message::DocumentReloaded(
                                id,
                                path.clone(),
                                Some(stamp),
                                result,
                            )
                        },
                    )
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
    /// setting gets its arm here, a creation-time one is named in
    /// `window::settings` and gets a new window, and a setting that can do
    /// neither gets a `restart_required` line. Anything a theme carries
    /// belongs in `apply_theme` instead, which an OS light/dark switch
    /// calls too.
    ///
    fn apply_config(&mut self, new: jumppad_config::Config) -> Task<Message> {
        let current = &self.config;
        let appearance_changed =
            new.themes != current.themes || new.mode != current.mode;

        // No repaint: nothing on screen changes until the next wheel event
        // or selection drag, and the widget reads the new values on the
        // `view` that one causes.
        if new.scroll.sensitivity != current.scroll.sensitivity {
            self.editor_config
                .set_scroll_sensitivity(new.scroll.sensitivity);
        }
        if new.scroll.drag_speed != current.scroll.drag_speed {
            self.editor_config.set_drag_speed(new.scroll.drag_speed);
        }

        // No repaint: tabs read the new depth on their next edit.
        if new.history.depth != current.history.depth {
            self.editor_config.set_undo_depth(new.history.depth);
        }

        // One [[languages]] edit can feed two consumers, so diff the derived
        // views: comment styles apply live (no repaint - nothing on screen
        // changes until the next toggle), grammar mappings can't.
        if new.comment_styles_by_extension()
            != current.comment_styles_by_extension()
        {
            self.editor_config
                .set_comment_styles(build_comment_styles(&new));
        }

        // Unlike the two above this one does change what is on screen - the
        // width every tab is drawn at - but it still needs no repaint of its
        // own: the editor damages its own bounds on every layout, whatever
        // changed (see AGENTS.md on the repaint nudge).
        if new.indentation != current.indentation {
            self.editor_config.set_indentation(build_indentation(&new));
        }

        // No repaint: nothing on screen changes until the next word motion
        // or double click, and neither draws anything the old list did.
        if new.words != current.words {
            self.editor_config.set_word_separators(
                jumppad_textarea::WordSeparators::new(&new.words.separators),
            );
        }

        // Settings a window is handed once, at creation. Nothing can reach
        // the one on screen, so it gets replaced by one built to the new
        // config - see `window::replace`.
        let replacement = window::needs_replacing(current, &new);
        if new.visor.enabled != current.visor.enabled {
            self.visor_enabled = new.visor.enabled;
            // Registered only while the visor can be summoned, and dropped
            // otherwise: holding the chord would keep it from every other app
            // for a mode that isn't on.
            self.hotkey = None;
            if self.visor_enabled {
                self.hotkey = Hotkey::register(self.keybinds.toggle);
            }
        }
        if new.extension_to_grammar() != current.extension_to_grammar() {
            restart_required("[[languages]] extension-to-syntax mappings");
        }
        // Both of these are settled once, when iced builds its compositor:
        // the adapter it picks, and the present mode it configures the
        // surface with. Nothing reaches either afterwards.
        if new.gpu != current.gpu {
            restart_required("[gpu]");
        }

        self.config = new;

        // Last, and through the same path an OS change takes: `theme_for`
        // reads the config that was just stored.
        let applied = if appearance_changed {
            self.showing = self.config.mode.showing(self.os_appearance);
            self.apply_theme()
        } else {
            Task::none()
        };

        match self.window.filter(|_| replacement) {
            // Ends at `WindowReady`, the same arm the first window arrives
            // through, so a replacement is set up by the code that sets up
            // every window - which is why the platform work `apply_theme`
            // just asked of the outgoing window costs nothing to repeat: the
            // new one is told all of it on arrival.
            Some(previous) => {
                window::replace(previous, window::settings(&self.config))
                    .map(|id| Message::WindowReady(Some(id)))
            }
            None => applied,
        }
    }

    /// Resolves the config and the showing slot into a theme, and writes it
    /// everywhere it is read from. The only way a theme reaches the app, so
    /// a config reload and a change of the OS's setting cannot drift apart.
    fn apply_theme(&mut self) -> Task<Message> {
        let theme = self.config.theme_for(self.showing);
        let was_translucent = self.background_alpha < 1.0;
        let was_blurred = self.background_blur;

        self.theme = resolve_palette(
            &theme.palette,
            resolve_palette(self.showing.default_palette(), Theme::Light),
        );
        // No check that the window can actually be seen through, because
        // there is no way for it not to be: an alpha below 1.0 here means
        // some theme in the file named one, which is the very question
        // `window::settings` asks to decide the window's transparency - and
        // `window::needs_replacing` puts a new window on screen the moment
        // the answer changes. See AGENTS.md.
        self.background_alpha = theme.background_alpha.clamp(0.0, 1.0);
        self.editor_config
            .set_background_alpha(theme.background_alpha);
        // Stored as written: which of these forms means anything is the
        // platform's to decide, and the radius is capped in `macos.rs`, the
        // one platform with a ceiling.
        self.background_blur = theme.background_blur;
        self.editor_config
            .set_foreground_alpha(theme.foreground_alpha);
        self.editor_config.set_font(resolve_font(
            theme.editor_font.family.as_deref(),
            Font::MONOSPACE,
        ));
        self.editor_config.set_font_size(theme.editor_font.size);
        self.ui_text = ui_text(&theme.ui_font);

        let translucency = if self.background_alpha < 1.0 && !was_translucent {
            // Already tells the window about the blur, so the two branches
            // can't both fire.
            self.turn_translucent()
        } else if self.background_blur != was_blurred {
            self.apply_window_blur()
        } else {
            Task::none()
        };

        Task::batch([translucency, self.sync_window_appearance()])
    }

    /// What a window has to be told once it is actually going to be seen
    /// through. Done at `WindowReady` for a window that starts translucent,
    /// and here for one that becomes translucent later - a theme switch can
    /// cross that line without needing a new window, since the window's
    /// transparency was settled at startup for every theme in the file.
    ///
    /// Both platforms leave the reverse alone: nothing puts a window's
    /// original backdrop back, the same as when a session boots translucent
    /// and reloads to solid.
    fn turn_translucent(&mut self) -> Task<Message> {
        self.arm_surface_reset();
        self.arm_shadow_refresh();

        self.apply_window_blur()
    }

    /// Takes the OS's light/dark setting and switches themes if it moved the
    /// showing slot. Recorded whatever `detection` says, so a reload that
    /// turns `auto` back on resolves against an answer already in hand.
    fn apply_os_appearance(&mut self, os: Option<Appearance>) -> Task<Message> {
        self.os_appearance = os;

        let showing = self.config.mode.showing(self.os_appearance);
        if showing != self.showing {
            self.showing = showing;
            return self.apply_theme();
        }

        Task::none()
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
    fn save_expectation(
        &self,
        tab: &Tab,
        shows_dialog: bool,
    ) -> SaveExpectation {
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
                let mut tab =
                    Tab::from_file(id, path, &contents, &self.editor_factory);
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
                self.error =
                    Some(format!("Couldn't open {}: {kind}", path.display()));
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
                    self.error = Some(format!(
                        "Can't open {}: it's a folder",
                        path.display()
                    ));
                    return Task::none();
                }
                Task::perform(read_path(path), Message::DroppedFileRead)
            }
            Message::DroppedFileRead(Ok((path, contents))) => {
                self.open_loaded_file(path, &contents)
            }
            Message::DroppedFileRead(Err(OpenError::Io { path, kind })) => {
                self.error =
                    Some(format!("Couldn't open {}: {kind}", path.display()));
                Task::none()
            }
            // No dialog is involved in a drop.
            Message::DroppedFileRead(Err(OpenError::DialogClosed)) => {
                Task::none()
            }
            Message::OpenPaths(paths) => self.open_paths(paths),
            Message::SaveFile => self.save_active_tab(false),
            Message::SaveFileAs => self.save_active_tab(true),
            Message::FileSaved(id, Ok((path, stamp))) => {
                self.file_dialog_active = false;
                self.config_watch.note_saved(&path, Instant::now());
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
                {
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
                    if let Some(index) =
                        self.tabs.iter().position(|tab| tab.id == id)
                    {
                        return self.close_tab(index);
                    }
                }
                Task::none()
            }
            Message::FileSaved(id, Err(SaveError::DialogClosed)) => {
                // The user canceled the save dialog - leave the tab open
                // rather than closing it unsaved.
                self.file_dialog_active = false;
                self.pending_close_after_save
                    .retain(|&pending_id| pending_id != id);
                Task::none()
            }
            Message::FileSaved(id, Err(SaveError::Conflict)) => {
                // `file_dialog_active` is deliberately left alone: only a
                // dialog-less save is ever checked (see `save_expectation`),
                // so clearing it here could dismiss another tab's open dialog.
                //
                // A conflict aborts the close, the same way VS Code leaves
                // the editor open when its save doesn't go through.
                self.pending_close_after_save
                    .retain(|&pending_id| pending_id != id);
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
                {
                    // The sweep may not have run yet - the save is how this
                    // one found out.
                    tab.externally_changed = true;
                }
                self.open_conflict_prompt(id)
            }
            Message::FileSaved(id, Err(SaveError::Io { path, kind })) => {
                self.file_dialog_active = false;
                self.pending_close_after_save
                    .retain(|&pending_id| pending_id != id);
                if let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) {
                    self.error = Some(format!(
                        "Couldn't save {}: {kind}",
                        tab.document.display_name()
                    ));
                } else {
                    self.error = Some(format!(
                        "Couldn't save {}: {kind}",
                        path.display()
                    ));
                }
                Task::none()
            }
            Message::SelectTab(index) => self.switch_active(index),
            Message::CloseTab(index) => self.request_close(index),
            Message::CloseActiveTab => self.request_close(self.active),
            Message::SelectNextTab => self.cycle_tab(1),
            Message::SelectPreviousTab => self.cycle_tab(-1),
            Message::SelectPreviousActiveTab => match self.previous_active_id {
                Some(id) => match self.tabs.iter().position(|tab| tab.id == id)
                {
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
                match handle_hotkey(
                    key,
                    modifiers,
                    physical_key,
                    &self.keybind_overrides,
                ) {
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
                    && self.active_find().is_some_and(|state| {
                        query.len() == state.query.len() + 1
                    });
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
                let delta = if matches!(message, Message::FindNext) {
                    1
                } else {
                    -1
                };
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
                    state.matches =
                        editor_core::find_matches(&text, &state.query);
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
                if let Some(tab) = self.tabs.get(self.active)
                    && let Some(state) = self.find.get_mut(&tab.id)
                {
                    // Closed, not cleared - the query is there next time.
                    state.open = false;
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
            Message::SystemAppearanceReported(reported) => {
                self.apply_os_appearance(system_appearance(reported))
            }
            Message::ConfigFileEvent(file) => {
                self.config_watch.note_event(file, Instant::now());
                Task::none()
            }
            Message::ConfigSettleTick => self.reload_settled_configs(),
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
                    let editor_message =
                        editor_message.with_shift_click(self.modifiers.shift());
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
                let writes = session::stale_tabs(&self.tabs).into_iter().map(
                    |(id, generation, text)| {
                        Task::perform(
                            session::flush_draft_async(
                                dir.clone(),
                                id,
                                generation,
                                text,
                            ),
                            |(id, generation)| {
                                Message::DraftFlushed(id, generation)
                            },
                        )
                    },
                );
                Task::batch(writes)
            }
            Message::DraftFlushed(id, generation) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
                    && generation > tab.flushed_generation
                {
                    tab.flushed_generation = generation;
                }
                Task::none()
            }
            Message::WindowCloseRequested(id) => {
                session::flush_on_exit(
                    &self.session_dir,
                    &self.tabs,
                    self.active,
                );
                iced::window::close(id)
            }
            Message::SessionFileLoaded(id, Ok(contents)) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
                {
                    tab.editor.set_text(&contents);
                    // The boot-time version of an external reload: the tab and
                    // its file agree again, so this is where the stamp is taken.
                    tab.restamp();
                }
                Task::none()
            }
            Message::SessionFileLoaded(_, Err(kind)) => {
                self.error =
                    Some(format!("Couldn't reload a restored tab: {kind}"));
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
                    ConflictDecision::Overwrite => {
                        self.save_tab_forcing_overwrite(id)
                    }
                    ConflictDecision::DiscardAndReload => {
                        self.reload_from_conflict(id)
                    }
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
                    self.error = Some(format!(
                        "Couldn't reload {}: {kind}",
                        path.display()
                    ));
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
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
                {
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
                Task::batch([
                    self.apply_window_blur(),
                    self.snap_to_monitor(),
                    // Every window arrives here, the first one included, so
                    // this is the only place focus is handed to a new one.
                    self.restore_focus(),
                    // And the only place a fresh window's appearance is,
                    // since iced pins one from the theme as it opens.
                    self.sync_window_appearance(),
                ])
            }
            Message::HotkeyEvent(event) => {
                let is_our_toggle =
                    self.hotkey.as_ref().is_some_and(|hotkey| {
                        event.state() == HotKeyState::Pressed
                            && event.id() == hotkey.id()
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
                // A resize reallocates the redirection surface, and what
                // lands in the new one is not zeroed - see `windows.rs`.
                self.arm_surface_reset();
                Task::none()
            }
            Message::ShadowRefreshFrame => {
                self.shadow_refresh_frames =
                    self.shadow_refresh_frames.saturating_sub(1);
                if self.shadow_refresh_frames == 0 {
                    self.refresh_window_shadow()
                } else {
                    Task::none()
                }
            }
            Message::SurfaceResetFrame => {
                self.surface_reset_frames =
                    self.surface_reset_frames.saturating_sub(1);
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
            log::warn!("couldn't determine the primary monitor's bounds");
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
            log::warn!("couldn't determine the primary monitor's bounds");
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

    /// The widget a window should hand keyboard focus to, by id, or `None`
    /// when nothing on screen wants it.
    fn focus_target(&self) -> Option<&'static str> {
        if self.modal.is_some() {
            // A modal's choices are driven from the app's own key handling
            // and its focused choice is a field of the modal, so there is no
            // widget here to hand anything to.
            return None;
        }

        match self.active_find().filter(|find| find.open) {
            Some(_) => Some(FIND_INPUT_ID),
            None => Some(editor_core::EDITOR_WIDGET_ID),
        }
    }

    /// Puts keyboard focus where [`focus_target`](Self::focus_target) says.
    ///
    /// Focus belongs to a window's own widget tree, so a window that has
    /// just appeared has none of it, however much of the app's state carried
    /// into it - the caret and the selection do, since those live in the
    /// document rather than in the widgets drawing it.
    ///
    /// Not `focus_find` for the palette, though the id is the same: that one
    /// selects the query as well, which is right when reopening the palette
    /// and wrong here, where the next keystroke would wipe a query the user
    /// was midway through.
    fn restore_focus(&self) -> Task<Message> {
        match self.focus_target() {
            Some(id) => operate(operation::focusable::focus(Id::new(id))),
            None => Task::none(),
        }
    }

    /// The active tab's find state, if it has any.
    fn active_find(&self) -> Option<&FindState> {
        self.find.get(&self.tabs.get(self.active)?.id)
    }

    /// The bar shown over the active tab when its file changed on disk while
    /// it had unsaved edits - VS Code shows one for the same reason, since
    /// the conflict otherwise stays invisible until the next save.
    fn changed_on_disk_bar(&self, tab_id: u64) -> Element<'_, Message> {
        let ui = self.ui_text;
        let action = |label: &'static str, message: Message| {
            button(ui.control_text(label))
                .padding([4, 8])
                .style(find_button_style)
                .on_press(message)
        };

        container(
            row![
                ui.control_text("This file has changed on disk."),
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
        let ui = self.ui_text;
        let query = text_input("Find", &state.query)
            .id(Id::new(FIND_INPUT_ID))
            .on_input(Message::FindQueryChanged)
            .on_submit(Message::FindNext)
            .padding([4, 8])
            .font(ui.font)
            .size(ui.input_size())
            .width(Pixels(180.0))
            .style(find_input_style);

        let counter: Element<'_, Message> = match state.counter() {
            Some(label) => ui.control_text(label).into(),
            None => text("").into(),
        };

        let step = |content: Text<'static>, message: Message| {
            button(content)
                .padding([4, 6])
                .style(find_button_style)
                .on_press(message)
        };

        container(
            row![
                query,
                counter,
                step(ui.control_text("\u{2191}"), Message::FindPrevious),
                step(ui.control_text("\u{2193}"), Message::FindNext),
                step(
                    ui.control_icon(jumppad_icons::CLOSE),
                    Message::CloseFind,
                ),
            ]
            .spacing(6)
            .align_y(Center),
        )
        .padding(6)
        .style(find_palette_style)
        .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let ui = self.ui_text;
        let tab_chips = self.tabs.iter().enumerate().map(|(index, tab)| {
            let is_active = index == self.active;

            let title = button(ui.tab_text(tab.title()))
                .padding([TAB_VERTICAL_PADDING, 10.0])
                .style(move |theme, status| {
                    tab_title_style(theme, status, is_active)
                })
                .on_press(Message::SelectTab(index));

            let close_diameter = ui.close_button_diameter();
            let close = round_icon_button(
                ui.control_icon(jumppad_icons::CLOSE),
                close_diameter,
            )
            .style(move |theme, status| {
                tab_close_style(theme, status, is_active, close_diameter)
            })
            .on_press(Message::CloseTab(index));

            // The frame is the only thing that paints this tab's background -
            // title and close stay fully transparent so there's one seamless surface.
            let frame = container(row![title, close].align_y(Center))
                .padding(Padding::ZERO.right(8))
                .style(move |theme| tab_frame_style(theme, is_active));

            // Middle-click closes it too, same as the close button.
            mouse_area(frame)
                .on_middle_press(Message::CloseTab(index))
                .into()
        });

        let new_tab_diameter = ui.new_tab_button_diameter();
        let new_tab_button = container(
            round_icon_button(
                ui.tab_icon(jumppad_icons::ADD),
                new_tab_diameter,
            )
            .style(move |theme, status| {
                new_tab_style(theme, status, new_tab_diameter)
            })
            .on_press(Message::NewTab),
        )
        .padding(Padding::ZERO.left(6).right(6))
        .height(ui.strip_height())
        .align_y(Center)
        .style(tab_bar_style);

        let tabs_row =
            row(tab_chips.chain(std::iter::once(new_tab_button.into())))
                .spacing(0)
                .align_y(Center);

        // No shared background container behind the row - on a transparent
        // window every extra layer compounds opacity, so the row is painted
        // once, in pieces. `filler` covers the leftover space past the last
        // chip, matching `title`'s padding and line height so its height lines
        // up without relying on flex cross-axis sizing.
        let filler = container(ui.tab_text(""))
            .padding([TAB_VERTICAL_PADDING, 10.0])
            .width(Fill)
            .style(tab_bar_style);

        // The scrollbar is hidden rather than absent: the row still scrolls
        // by wheel or trackpad, but a floating bar over a strip this short
        // sits across the tab titles and makes them unreadable.
        let tab_bar: Element<'_, Message> = row![
            scrollable(tabs_row).direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::hidden(),
            )),
            filler,
        ]
        .width(Fill)
        .align_y(Center)
        .into();

        let editor: Element<'_, Message> =
            if let Some(tab) = self.tabs.get(self.active) {
                let index = self.active;
                let tab_id = tab.id;
                let view = tab
                    .editor
                    .view()
                    .map(move |message| Message::Editor(index, message));
                // Keyed by the tab's stable id, not its Vec index, so switching
                // tabs replaces the editor widget instead of reusing stale state.
                keyed_column([(tab_id, view)])
                    .width(Fill)
                    .height(Fill)
                    .into()
            } else {
                ui.body_text("No open tabs").into()
            };

        // Composed before the find palette so the palette floats above the
        // bar rather than under it - the bar's controls sit at its left end,
        // where the top-right palette doesn't reach.
        let editor = match self
            .tabs
            .get(self.active)
            .filter(|tab| tab.externally_changed)
        {
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
                container(center(ui.body_text("Drop to open")))
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
                    ui.body_text(error.clone())
                        .color(iced::Color::from_rgb8(220, 60, 60)),
                    button(ui.body_text("Dismiss"))
                        .on_press(Message::DismissError),
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
                let choice =
                    |label: &'static str,
                     index: usize,
                     decision: CloseDecision| {
                        modal_choice(ui, label, pending.focused == index)
                            .on_press(Message::CloseConfirmed(
                                pending.tab_id,
                                decision,
                            ))
                    };
                modal_dialog(
                    ui,
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
                let choice =
                    |label: &'static str,
                     index: usize,
                     decision: ConflictDecision| {
                        modal_choice(ui, label, pending.focused == index)
                            .on_press(Message::ConflictResolved(
                                pending.tab_id,
                                decision,
                            ))
                    };
                modal_dialog(
                    ui,
                    format!(
                        "{} has changed on disk since you opened it.",
                        pending.title
                    ),
                    row![
                        choice("Overwrite", 0, ConflictDecision::Overwrite),
                        choice(
                            "Discard & Reload",
                            1,
                            ConflictDecision::DiscardAndReload
                        ),
                        choice("Cancel", 2, ConflictDecision::Cancel),
                    ],
                )
            }
        };

        // Covers the window to block clicks reaching what's underneath; no
        // `on_press`, so clicking it can't lose unsaved work by accident.
        let scrim = mouse_area(
            container(text(""))
                .width(Fill)
                .height(Fill)
                .style(modal_scrim_style),
        );

        stack![content, scrim, center(dialog)].into()
    }

    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }

    /// Scales the window's base `background_color` by `background_alpha` -
    /// needed for the desktop to actually show through a transparent window.
    pub fn style(&self, theme: &Theme) -> iced::theme::Style {
        let mut style = iced::theme::default(theme);
        if self.background_alpha < 1.0 {
            style.background_color =
                style.background_color.scale_alpha(self.background_alpha);
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
                    key,
                    modifiers,
                    ..
                }) = event
                else {
                    return None;
                };
                match key.as_ref() {
                    keyboard::Key::Named(key::Named::Escape) => {
                        Some(Message::CloseFind)
                    }
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
            // Ungated on purpose: iced's runtime watches the OS setting
            // whether or not anything listens, so pinning `[mode]` saves
            // nothing by unsubscribing and would only let the answer go
            // stale under a later switch back to `auto`.
            iced::system::theme_changes()
                .map(Message::SystemAppearanceReported),
        ];

        // Gated timers below: each only ticks while its condition holds, so
        // there's no idle cost once things settle.
        if self.animation.is_some() {
            subscriptions.push(
                iced::time::every(VISOR_ANIM_TICK)
                    .map(|_| Message::AnimationTick),
            );
        }

        if self.shadow_refresh_frames > 0 {
            subscriptions.push(
                iced::window::frames().map(|_| Message::ShadowRefreshFrame),
            );
        }

        if self.surface_reset_frames > 0 {
            subscriptions.push(
                iced::window::frames().map(|_| Message::SurfaceResetFrame),
            );
        }

        if self
            .tabs
            .iter()
            .any(|tab| tab.editor.has_pending_highlighting())
        {
            subscriptions.push(
                iced::time::every(Duration::from_millis(50))
                    .map(|_| Message::PollHighlighting),
            );
        }

        if self.tabs.iter().any(|tab| {
            tab.dirty && tab.draft_generation != tab.flushed_generation
        }) {
            subscriptions.push(
                iced::time::every(AUTOSAVE_INTERVAL)
                    .map(|_| Message::AutosaveTick),
            );
        }

        if self.config_watch.pending() {
            subscriptions.push(
                iced::time::every(reload::SETTLE_TICK)
                    .map(|_| Message::ConfigSettleTick),
            );
        }

        if self.document_watch.pending() {
            subscriptions.push(
                iced::time::every(docwatch::SETTLE_TICK)
                    .map(|_| Message::DocumentSettleTick),
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
fn build_key_overrides(
    keybinds: &jumppad_config::KeybindsConfig,
) -> KeyOverrides {
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
        Action::SelectPreviousActiveTab => {
            Some(Message::SelectPreviousActiveTab)
        }
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
    if let key::Physical::Code(code) = physical_key
        && let Some(&action) = overrides.get(&(modifiers, code))
        && (action.context() == Context::Always || action.context() == context)
    {
        return Some(action);
    }
    jumppad_keybinds::action_for(key, physical_key, modifiers, context)
}

/// The resolver handed to every `TextArea`, closing over the overrides of the
/// moment. Rebuilt and re-injected on a `keybinds.toml` reload.
fn build_key_resolver(
    overrides: Arc<KeyOverrides>,
) -> Arc<jumppad_textarea::KeyResolver> {
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
fn warn_unrecognized_overrides(
    overrides: &HashMap<String, global_hotkey::hotkey::HotKey>,
) {
    for name in overrides.keys() {
        if Action::from_name(name).is_none() {
            log::warn!(
                "keybinds.toml overrides an unrecognized command {name:?}, ignoring"
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

/// The text the app draws around the editor: one font and one size, with
/// the smaller sizes derived from that size so the whole frame scales
/// together. The proportions are the ones the sizes had when they were
/// hardcoded at a 16px base.
///
/// The line heights are absolute so the tab strip's height - and with it
/// every horizontal quad edge in it - lands on a whole pixel. iced's default
/// `LineHeight::Relative(1.3)` would put the strip at 6 + 20.8 + 6 = 32.8px,
/// and a quad edge mid-pixel gets antialiased into a visible seam on a
/// transparent window (see AGENTS.md). Only holds at integer scale factors;
/// nothing chosen in logical pixels survives a 1.25x or 1.5x display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UiText {
    font: Font,
    /// Tab titles draw at this; everything smaller is a fraction of it.
    base: f32,
}

impl UiText {
    fn new(font: Font, size: f32) -> Self {
        Self {
            font,
            base: jumppad_textarea::font::clamp_size(size),
        }
    }

    /// Tab titles, the new-tab button, and the filler that has to match
    /// their height.
    fn tab_text<'a>(
        self,
        content: impl iced::widget::text::IntoFragment<'a>,
    ) -> iced::widget::Text<'a> {
        text(content)
            .font(self.font)
            .size(self.base)
            .line_height(Pixels((self.base * 1.3).floor()))
    }

    /// The compact controls: a tab's close button, and the find palette's
    /// and changed-on-disk bar's labels and buttons.
    fn control_text<'a>(
        self,
        content: impl iced::widget::text::IntoFragment<'a>,
    ) -> iced::widget::Text<'a> {
        let size = self.control_size();

        text(content)
            .font(self.font)
            .size(size)
            .line_height(Pixels((size * 1.3).ceil()))
    }

    fn control_size(self) -> f32 {
        self.base * 0.75
    }

    /// The round button behind a tab's close icon. Only a little wider than
    /// the glyph, so it reads as a target around the icon rather than a
    /// button the icon happens to sit in, and rounded to whole pixels so its
    /// edge lands on the grid.
    /// How tall the tab strip stands: a title's line box plus the padding
    /// above and below it.
    fn strip_height(self) -> f32 {
        TAB_VERTICAL_PADDING * 2.0 + (self.base * 1.3).floor()
    }

    fn close_button_diameter(self) -> f32 {
        (self.control_size() * ICON_SCALE * 1.7).round()
    }

    /// The new-tab button's, a quarter wider again. It is the strip's one
    /// standing control rather than something that appears beside a title,
    /// so it can carry more weight.
    fn new_tab_button_diameter(self) -> f32 {
        (self.close_button_diameter() * 1.25).round()
    }

    /// An icon at tab-title size. The face is the icon font rather than the
    /// configured UI one, because these codepoints sit in the Private Use
    /// Area, where every other face draws nothing.
    fn tab_icon<'a>(self, icon: char) -> Text<'a> {
        self.tab_text(icon.to_string())
            .font(ICON_FONT)
            .size(self.base * ICON_SCALE)
    }

    /// An icon at the compact controls' size.
    fn control_icon<'a>(self, icon: char) -> Text<'a> {
        self.control_text(icon.to_string())
            .font(ICON_FONT)
            .size(self.control_size() * ICON_SCALE)
    }

    /// Full sentences - dialog prompts, the error banner, the empty and
    /// drop-target states - which run at the default relative line height.
    fn body_text<'a>(
        self,
        content: impl iced::widget::text::IntoFragment<'a>,
    ) -> iced::widget::Text<'a> {
        text(content).font(self.font).size(self.base)
    }

    /// The find palette's query field, which takes a bare size rather than
    /// a `Text`.
    fn input_size(self) -> f32 {
        self.base * 0.875
    }
}

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
    if is_active {
        text
    } else {
        text.scale_alpha(0.7)
    }
}

/// A tab's title button: always transparent - the enclosing `tab_frame_style`
/// container is what paints the tab's background, so title and close never
/// have to agree on a box size to look like one continuous surface.
fn tab_title_style(
    theme: &Theme,
    _status: button::Status,
    is_active: bool,
) -> button::Style {
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
fn tab_close_style(
    theme: &Theme,
    status: button::Status,
    is_active: bool,
    diameter: f32,
) -> button::Style {
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
        border: Border::default().rounded(diameter / 2.0),
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
        container::Style::default()
            .background(darkening_wash(theme, INACTIVE_TAB_DARKEN))
    }
}

/// The new-tab button: transparent and dim at rest so it doesn't compete
/// with the tabs themselves, taking the same round highlight a close button
/// does on hover. It paints nothing of its own at rest, so what shows
/// through is the tab row's background - which is why it sits in a container
/// styled with `tab_bar_style` rather than directly on the window.
fn new_tab_style(
    theme: &Theme,
    status: button::Status,
    diameter: f32,
) -> button::Style {
    let text = theme.extended_palette().background.base.text;
    let (text_color, background) = match status {
        button::Status::Hovered | button::Status::Pressed => {
            (text, Some(text.scale_alpha(0.15).into()))
        }
        _ => (text.scale_alpha(0.4), None),
    };
    button::Style {
        background,
        text_color,
        border: Border::default().rounded(diameter / 2.0),
        ..button::Style::default()
    }
}

/// The tab row's own background - a wash darker than even an inactive tab, so
/// empty space past the last tab reads as a frame, not a gap.
fn tab_bar_style(theme: &Theme) -> container::Style {
    container::Style::default()
        .background(darkening_wash(theme, TAB_ROW_DARKEN))
}

/// A round icon button: a circle of `diameter` with the glyph centred in
/// it, which is the shape its hover highlight takes. Sized rather than
/// padded, because padding around a glyph whose own box is taller than it is
/// wide would give an oval.
fn round_icon_button<'a>(
    icon: Text<'a>,
    diameter: f32,
) -> button::Button<'a, Message> {
    button(icon.width(Fill).height(Fill).center())
        .width(diameter)
        .height(diameter)
        .padding(0)
}

/// One modal choice button, shared by both dialogs - they differ only in
/// their labels and the message each choice sends. The caller adds that with
/// `.on_press`, so a click resolves the dialog the same way Enter does.
fn modal_choice(
    ui: UiText,
    label: &'static str,
    is_focused: bool,
) -> button::Button<'static, Message> {
    button(ui.body_text(label))
        .padding([6, 14])
        .style(move |theme, status| {
            modal_button_style(theme, status, is_focused)
        })
}

/// A modal's box: one line of prompt over a row of choices.
fn modal_dialog<'a>(
    ui: UiText,
    prompt: String,
    choices: iced::widget::Row<'a, Message>,
) -> container::Container<'a, Message> {
    container(
        column![ui.body_text(prompt), choices.spacing(10)]
            .spacing(16)
            .padding(20),
    )
    .style(modal_dialog_style)
}

/// One of a modal's three choices - a colored border marks whichever one
/// keyboard nav currently has focused.
fn modal_button_style(
    theme: &Theme,
    status: button::Status,
    is_focused: bool,
) -> button::Style {
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

fn find_palette_style(theme: &Theme) -> container::Style {
    container::Style::default()
        .background(darkening_wash(theme, FLOATING_SURFACE_DARKEN))
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

fn find_input_style(
    theme: &Theme,
    status: text_input::Status,
) -> text_input::Style {
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

/// The modal's own dialog box - opaque, so it reads as a real window
/// sitting on top of the scrim rather than another translucent layer.
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

/// Mirror of `build_comment_styles` for `[indentation]`, and here for the
/// same reason: the widget crate names its own indentation types so it
/// doesn't have to depend on `jumppad_config`. The width is range-checked
/// on the way through, by the only constructor there is.
fn build_indentation(
    config: &jumppad_config::Config,
) -> jumppad_textarea::Indentation {
    let style = match config.indentation.style {
        jumppad_config::IndentationStyle::Tabs => {
            jumppad_textarea::IndentationStyle::Tabs
        }
        jumppad_config::IndentationStyle::Spaces => {
            jumppad_textarea::IndentationStyle::Spaces
        }
    };
    jumppad_textarea::Indentation::new(style, config.indentation.width)
}

/// A reloaded setting that only applies at startup. Logged, not shown in
/// the banner: the change is valid, it just waits for the next start.
fn restart_required(what: &str) {
    log::info!("{what} changed - takes effect on restart");
}

/// What the OS reports, in JumpPad's terms. `None` means it stated no
/// preference, which leaves `detection = "auto"` on the light slot.
fn system_appearance(reported: iced::theme::Mode) -> Option<Appearance> {
    match reported {
        iced::theme::Mode::Light => Some(Appearance::Light),
        iced::theme::Mode::Dark => Some(Appearance::Dark),
        iced::theme::Mode::None => None,
    }
}

/// Matches a config-file palette name against `Theme::ALL` by display name
/// (`"Dracula"`, `"Solarized Light"`, ...), case-insensitively so hand-edited
/// TOML doesn't have to get the exact casing right. Falls back to `fallback`
/// - and logs why - rather than failing startup over a typo.
///
/// An `iced::Theme` comes back because that is how iced carries a palette:
/// each of its themes is one, named. JumpPad's own themes are the wider
/// thing, in `jumppad_config`.
fn resolve_palette(name: &str, fallback: Theme) -> Theme {
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
            log::warn!(
                "unknown palette {name:?}, using default. Valid options: {valid}"
            );
            fallback
        }
    }
}

/// Matches a config-file font family against the families installed on this
/// machine, case-insensitively so hand-edited TOML doesn't have to get the
/// exact casing right. An unnamed or unavailable family takes `fallback` -
/// and logs why - rather than letting a name nothing provides draw text in
/// whatever face the platform substitutes for it.
pub(crate) fn resolve_font(family: Option<&str>, fallback: Font) -> Font {
    let Some(name) = family.map(str::trim).filter(|name| !name.is_empty())
    else {
        return fallback;
    };

    jumppad_textarea::font::installed(name).unwrap_or_else(|| {
        log::warn!(
            "font family {name:?} isn't installed, using the default font"
        );
        fallback
    })
}

/// The chrome's text for a theme's `ui.font` section. Its fallback is the default
/// sans face rather than the editor's monospace one: chrome set in a
/// monospaced face because a family was misspelled would look like a
/// different bug than the one it is.
pub(crate) fn ui_text(font: &jumppad_config::ResolvedFont) -> UiText {
    UiText::new(
        resolve_font(font.family.as_deref(), Font::DEFAULT),
        font.size,
    )
}

/// Where syntax-highlighting wasm grammars (`<extension>.wasm`) are looked for.
fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.join("syntaxes"));
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
                    .filter(|path| {
                        path.extension().and_then(|ext| ext.to_str())
                            == Some("wasm")
                    })
                    .filter_map(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .collect();
                wasm_files.sort();
                if wasm_files.is_empty() {
                    log::debug!(
                        "{}: exists but no .wasm files found",
                        dir.display()
                    );
                } else {
                    log::debug!(
                        "{}: found {} .wasm file(s): {}",
                        dir.display(),
                        wasm_files.len(),
                        wasm_files.join(", ")
                    );
                }
            }
            Err(err) => {
                log::debug!(
                    "{}: couldn't read directory: {err}",
                    dir.display()
                );
            }
        }
    }
}

/// Re-reads a restored tab's real file fresh from disk, never a cached copy.
async fn reload_from_disk(
    path: PathBuf,
) -> Result<Arc<String>, std::io::ErrorKind> {
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
#[path = "app_tests.rs"]
mod tests;
