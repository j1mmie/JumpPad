mod config_reload;
mod find;
mod styles;
mod tabs;
mod theme;
mod update;
mod view;
mod window_sync;

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
    Text, button, center, container, keyed_column, mouse_area, row,
    scrollable, stack, text, text_input,
};
use iced::{
    Border, Center, Color, Element, Fill, Font, Padding, Pixels, Point, Right,
    Subscription, Task, Theme, Top, keyboard,
};

use jumppad_config::Appearance;

use crate::docwatch;
use crate::find::FindState;
use crate::hotkey::{
    self, Hotkey, KeyOverrides, build_key_overrides, build_key_resolver,
    warn_unrecognized_overrides,
};
use crate::reload;
use crate::session;
use crate::visor::{self, Animation};
use crate::window;

use styles::{UiText, premultiply, ui_text};

// Test-only: these live in sibling modules and are otherwise reached only
// through `self.method()` calls (which resolve across files on their own) or
// fully-qualified paths, but `app_tests.rs`'s `use super::*` needs them
// bound here by name, the same way it needs everything else in this file.
#[cfg(test)]
use crate::hotkey::{handle_hotkey, message_for};
#[cfg(test)]
use jumppad_actions::Action;
#[cfg(test)]
use styles::{
    CLEAR_COLOR_NEEDS_PREMULTIPLY, TAB_ROW_DARKEN, resolve_font,
    tab_frame_style,
};
#[cfg(test)]
use tabs::conflicts;
#[cfg(test)]
use theme::resolve_palette;

/// The face the icon glyphs are drawn in, selected by the family name the
/// font records. `run` registers [`ICON_FONT_BYTES`] at startup.
const ICON_FONT: Font = Font::with_name(jumppad_icons::FAMILY_NAME);

/// The font `cargo build_fonts` writes, embedded for `run` to hand to iced.
pub(crate) const ICON_FONT_BYTES: &[u8] =
    include_bytes!("../../../../assets/fonts/jumppad-icons.ttf");

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
    /// The bundles under `syntaxes/` with `config.languages` patched over
    /// them - what every extension and comment-style lookup actually reads.
    languages: jumppad_config::Languages,
    /// Where those bundles were found, kept so a config reload can resolve
    /// them again against the file it just read.
    grammar_search_dirs: Vec<PathBuf>,
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
        let search_dirs = crate::grammar_paths::default_search_dirs();
        let languages =
            jumppad_config::Languages::resolve(&config.languages, &search_dirs);
        crate::grammar_paths::log_bundles_found(&search_dirs, &languages);
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
        editor_config
            .set_comment_styles(theme::build_comment_styles(&languages));
        editor_config.set_indentation(theme::build_indentation(&config));
        editor_config.set_word_separators(
            jumppad_textarea::WordSeparators::new(&config.words.separators),
        );

        let registry = syntax_registry::SyntaxRegistry::new(
            search_dirs.clone(),
            crate::grammar_paths::grammar_lookup(&languages),
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
            languages,
            grammar_search_dirs: search_dirs,
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
                            tabs::reload_from_disk(path),
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
                        tabs::reload_from_disk(path.clone()),
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
            if styles::CLEAR_COLOR_NEEDS_PREMULTIPLY {
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

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
