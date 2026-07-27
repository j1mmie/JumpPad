use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use editor_core::{EditorFactory, EditorMessage, Tab};
use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Element, Fill, Subscription, Task, Theme, keyboard};

pub struct XizorApp {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    error: Option<String>,
    editor_factory: EditorFactory,
    theme: Theme,
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
    /// Takes the already-loaded config rather than loading it itself: the
    /// renderer backend (`config.renderer`) has to be known - and the
    /// `ICED_BACKEND` env var set - before `main` starts the iced runtime,
    /// which is before this constructor ever runs.
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
        };
        app.new_tab();
        (app, Task::none())
    }

    fn new_tab(&mut self) {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab::untitled(id, &self.editor_factory));
        self.active = self.tabs.len() - 1;
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.new_tab();
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
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
            Message::NewTab => {
                self.new_tab();
                Task::none()
            }
            Message::OpenFile => Task::perform(open_and_read(), Message::FileOpened),
            Message::FileOpened(Ok((path, contents))) => {
                let id = self.next_id;
                self.next_id += 1;
                self.tabs
                    .push(Tab::from_file(id, path, &contents, &self.editor_factory));
                self.active = self.tabs.len() - 1;
                Task::none()
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
            Message::SelectTab(index) => {
                if index < self.tabs.len() {
                    self.active = index;
                }
                Task::none()
            }
            Message::CloseTab(index) => {
                self.close_tab(index);
                Task::none()
            }
            Message::CloseActiveTab => {
                self.close_tab(self.active);
                Task::none()
            }
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
        let toolbar = row![
            button("New Tab").on_press(Message::NewTab),
            button("Open...").on_press(Message::OpenFile),
            button("Save").on_press(Message::SaveFile),
            button("Save As...").on_press(Message::SaveFileAs),
            button("Close Tab").on_press(Message::CloseActiveTab),
        ]
        .spacing(6)
        .padding(6);

        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            row![
                button(text(tab.title())).style(if index == self.active {
                    button::primary
                } else {
                    button::secondary
                }).on_press(Message::SelectTab(index)),
                button("x").on_press(Message::CloseTab(index)),
            ]
            .spacing(2)
            .into()
        });
        let tab_bar = scrollable(
            row(tabs.chain(std::iter::once(
                button("+").on_press(Message::NewTab).into(),
            )))
            .spacing(6)
            .padding(6),
        )
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new(),
        ));

        let editor: Element<'_, Message> = if let Some(tab) = self.tabs.get(self.active) {
            let index = self.active;
            tab.editor.view().map(move |message| Message::Editor(index, message))
        } else {
            text("No open tabs").into()
        };

        let mut content = column![
            toolbar,
            tab_bar,
            container(editor).width(Fill).height(Fill),
        ];

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
        self.theme.clone()
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
