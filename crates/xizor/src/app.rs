use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use editor_core::{EditorFactory, EditorMessage, Tab};
use iced::advanced::widget::{operate, operation};
use iced::widget::{button, column, container, keyed_column, row, scrollable, text};
use iced::{Center, Color, Element, Fill, Subscription, Task, Theme, keyboard};

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
        let registry = syntax_registry::SyntaxRegistry::new(
            search_dirs,
            config.syntaxes.extension_to_grammar(),
            || {},
        );
        // Which `TextEditorWidget` implementation new tabs are created with.
        // Swapping editor backends later means changing this one line (and
        // the `iced_text_editor` dependency) - nothing else in this file
        // needs to know.
        let editor_factory: EditorFactory =
            Box::new(iced_text_editor::IcedTextEditor::factory(registry));

        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            error: None,
            editor_factory,
            theme: resolve_theme(&config.theme),
            redraw_nudge: false,
        };
        let task = app.new_tab();
        (app, task)
    }

    fn new_tab(&mut self) -> Task<Message> {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab::untitled(id, &self.editor_factory));
        self.switch_active(self.tabs.len() - 1)
    }

    fn close_tab(&mut self, index: usize) -> Task<Message> {
        if index >= self.tabs.len() {
            return Task::none();
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.new_tab()
        } else if self.active >= self.tabs.len() {
            self.switch_active(self.tabs.len() - 1)
        } else {
            Task::none()
        }
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
        }
        self.active = index;
        self.redraw_nudge = !self.redraw_nudge;
        if let Some(tab) = self.tabs.get_mut(index) {
            let (line, column) = tab.last_cursor;
            tab.editor.move_cursor_to(line, column);
        }
        operate(operation::focusable::focus_next())
    }

    fn save_active_tab(&self, force_dialog: bool) -> Task<Message> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Task::none();
        };
        let id = tab.id;
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
                Task::none()
            }
            Message::FileSaved(_, Err(SaveError::DialogClosed)) => Task::none(),
            Message::FileSaved(id, Err(SaveError::Io { path, kind })) => {
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
            Message::CloseTab(index) => self.close_tab(index),
            Message::CloseActiveTab => self.close_tab(self.active),
            Message::DismissError => {
                self.error = None;
                Task::none()
            }
            Message::Editor(index, editor_message) => {
                if let Some(tab) = self.tabs.get_mut(index) {
                    if tab.editor.update(editor_message) {
                        tab.dirty = true;
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
        }
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
            container(row![title, close].spacing(0).align_y(Center))
                .style(move |theme| tab_frame_style(theme, is_active))
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
        let mut subscriptions = vec![keyboard::on_key_press(handle_hotkey)];

        // Only ticks while some tab is actually waiting on a grammar load -
        // avoids a permanent background timer once every tab's highlighting
        // has settled.
        if self.tabs.iter().any(|tab| tab.editor.has_pending_highlighting()) {
            subscriptions.push(
                iced::time::every(Duration::from_millis(50)).map(|_| Message::PollHighlighting),
            );
        }

        Subscription::batch(subscriptions)
    }
}

fn handle_hotkey(key: keyboard::Key, modifiers: keyboard::Modifiers) -> Option<Message> {
    if !modifiers.command() {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character("n") => Some(Message::NewTab),
        keyboard::Key::Character("o") => Some(Message::OpenFile),
        keyboard::Key::Character("s") if modifiers.shift() => Some(Message::SaveFileAs),
        keyboard::Key::Character("s") => Some(Message::SaveFile),
        keyboard::Key::Character("w") => Some(Message::CloseActiveTab),
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
            Theme::default()
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
