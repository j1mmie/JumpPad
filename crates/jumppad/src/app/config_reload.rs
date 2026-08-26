use super::*;
use super::styles::ui_text;
use super::theme::{
    build_comment_styles, build_indentation, resolve_palette,
    restart_required,
};
use crate::hotkey::{build_key_overrides, build_key_resolver};

impl JumpPadApp {
    /// Rewrites the session manifest, pruning orphaned draft files.
    pub(super) fn sync_session_metadata(&self) {
        let manifest = session::build_manifest(&self.tabs, self.active);
        session::write_manifest_sync(&self.session_dir, &manifest);
    }

    /// Reloads whichever config files settled out of a change burst. A file
    /// that no longer parses keeps the current in-memory settings and says
    /// so in the error banner - a save mid-edit must not reset anything.
    pub(super) fn reload_settled_configs(&mut self) -> Task<Message> {
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
    pub(super) fn watched_paths(&self) -> Vec<PathBuf> {
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
    pub(super) fn sweep_documents(&mut self) -> Task<Message> {
        let reads =
            self.resolve_disk_changes()
                .into_iter()
                .map(|(id, path, stamp)| {
                    Task::perform(
                        super::tabs::reload_from_disk(path.clone()),
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
    pub(super) fn resolve_disk_changes(
        &mut self,
    ) -> Vec<(u64, PathBuf, DiskStamp)> {
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
    pub(super) fn apply_config(
        &mut self,
        new: jumppad_config::Config,
    ) -> Task<Message> {
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
    pub(super) fn apply_theme(&mut self) -> Task<Message> {
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
        self.editor_config.set_font(super::styles::resolve_font(
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
    pub(super) fn turn_translucent(&mut self) -> Task<Message> {
        self.arm_surface_reset();
        self.arm_shadow_refresh();

        self.apply_window_blur()
    }

    /// Takes the OS's light/dark setting and switches themes if it moved the
    /// showing slot. Recorded whatever `detection` says, so a reload that
    /// turns `auto` back on resolves against an answer already in hand.
    pub(super) fn apply_os_appearance(
        &mut self,
        os: Option<Appearance>,
    ) -> Task<Message> {
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
    pub(super) fn apply_keybinds(
        &mut self,
        new: jumppad_config::KeybindsConfig,
    ) {
        self.keybind_overrides = Arc::new(build_key_overrides(&new));
        self.editor_config
            .set_resolver(build_key_resolver(self.keybind_overrides.clone()));
        crate::hotkey::warn_unrecognized_overrides(&new.overrides);

        // Re-registered only on an actual change: dropping the old
        // registration releases the chord to other apps, however briefly.
        if self.visor_enabled && new.toggle != self.keybinds.toggle {
            self.hotkey = None;
            self.hotkey = Hotkey::register(new.toggle);
        }
        self.keybinds = new;
    }
}
