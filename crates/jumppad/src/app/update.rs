use super::*;

impl JumpPadApp {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NewTab => self.new_tab(),
            Message::OpenFile => {
                if self.file_dialog_active {
                    Task::none()
                } else {
                    self.file_dialog_active = true;
                    Task::perform(
                        super::tabs::open_and_read(),
                        Message::FileOpened,
                    )
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
                Task::perform(
                    super::tabs::read_path(path),
                    Message::DroppedFileRead,
                )
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
                match crate::hotkey::handle_hotkey(
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
            Message::SystemAppearanceReported(reported) => self
                .apply_os_appearance(super::theme::system_appearance(
                    reported,
                )),
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
}
