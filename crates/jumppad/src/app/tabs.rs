use super::*;

impl JumpPadApp {
    pub(super) fn new_tab(&mut self) -> Task<Message> {
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
    pub(super) fn open_paths(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
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
    pub(super) fn tab_index_holding(
        &self,
        id: u64,
        path: &Path,
    ) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            tab.id == id && tab.document.path.as_deref() == Some(path)
        })
    }

    /// The tab already showing `path`, if one is open.
    pub(super) fn tab_index_for(&self, path: &Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.document.path.as_deref() == Some(path))
    }

    /// Opens an already-read file, loading it into the active tab when that tab
    /// is an untouched scratch tab rather than leaving a stray "Untitled"
    /// behind. Shared by dropped files and files named on the command line.
    pub(super) fn open_loaded_file(
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
    pub(super) fn request_close(&mut self, index: usize) -> Task<Message> {
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
    pub(super) fn show_next_modal(&mut self) -> Task<Message> {
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
    pub(super) fn open_conflict_prompt(&mut self, id: u64) -> Task<Message> {
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
    pub(super) fn save_tab_forcing_overwrite(
        &mut self,
        id: u64,
    ) -> Task<Message> {
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
    pub(super) fn reload_from_conflict(&mut self, id: u64) -> Task<Message> {
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

    pub(super) fn close_tab(&mut self, index: usize) -> Task<Message> {
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

    pub(super) fn switch_active(&mut self, index: usize) -> Task<Message> {
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
    pub(super) fn cycle_tab(&mut self, delta: isize) -> Task<Message> {
        if self.tabs.is_empty() {
            return Task::none();
        }
        let len = self.tabs.len() as isize;
        let next = (self.active as isize + delta).rem_euclid(len) as usize;
        self.switch_active(next)
    }

    pub(super) fn save_active_tab(
        &mut self,
        force_dialog: bool,
    ) -> Task<Message> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Task::none();
        };
        let id = tab.id;
        self.save_tab(id, force_dialog)
    }

    /// Saves the tab with the given id. Shows a file dialog if the tab has no
    /// associated file. Otherwise, saves to the associated file
    pub(super) fn save_tab(
        &mut self,
        id: u64,
        force_dialog: bool,
    ) -> Task<Message> {
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
    pub(super) fn save_expectation(
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
}

/// Whether writing to `path` now would clobber a change made since the tab
/// last looked.
///
/// A missing file is never a conflict: there is nothing to clobber, and the
/// write recreates it - the delete rule. A file that *appeared* where the
/// tab saw none is, though, which is why `Seen` carries an `Option` rather
/// than folding "saw nothing" into `Unchecked`.
pub(super) fn conflicts(path: &Path, expected: SaveExpectation) -> bool {
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

/// Re-reads a restored tab's real file fresh from disk, never a cached copy.
pub(super) async fn reload_from_disk(
    path: PathBuf,
) -> Result<Arc<String>, std::io::ErrorKind> {
    tokio::fs::read_to_string(&path)
        .await
        .map(Arc::new)
        .map_err(|err| err.kind())
}

/// Reads a path that's already known - a dropped file, and the tail of the
/// Open dialog once it has produced one.
pub(super) async fn read_path(
    path: PathBuf,
) -> Result<(PathBuf, Arc<String>), OpenError> {
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
pub(super) async fn open_and_read() -> Result<(PathBuf, Arc<String>), OpenError>
{
    let handle = rfd::AsyncFileDialog::new()
        .pick_file()
        .await
        .ok_or(OpenError::DialogClosed)?;
    read_path(handle.path().to_owned()).await
}
