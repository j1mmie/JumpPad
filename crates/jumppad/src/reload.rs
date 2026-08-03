//! Live config reload: notices changes to `config.toml` and `keybinds.toml`
//! and tells the app when a settled burst is ready to apply.
//!
//! Three signals feed one [`ConfigWatch`], which is what keeps them from
//! conflicting: the editor saving a watched file itself, native file-system
//! events, and a fingerprint check when the window regains focus (the
//! safety net for events the watcher never delivered). Every signal only
//! marks a file dirty and pokes a shared debounce; [`ConfigWatch::settled`]
//! is the single decider of what reloads, at most once per burst.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use editor_core::Debounce;
use iced::Subscription;
use notify::Watcher;

use crate::app::Message;

/// How long the files must sit quiet before a reload applies - long enough
/// to swallow an editor's atomic-save dance (write, rename, write again),
/// short enough to feel immediate.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(300);

/// Granularity of the settle check. Its timer only runs while a burst is
/// pending (see `JumpPadApp::subscription`), so this isn't an idle cost.
pub const SETTLE_TICK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchedFile {
    Config,
    Keybinds,
}

impl WatchedFile {
    pub fn name(self) -> &'static str {
        match self {
            WatchedFile::Config => "config.toml",
            WatchedFile::Keybinds => "keybinds.toml",
        }
    }

    fn effective_path(self) -> Option<PathBuf> {
        match self {
            WatchedFile::Config => jumppad_config::config_file(),
            WatchedFile::Keybinds => jumppad_config::keybinds_file(),
        }
    }
}

/// Which watched file `path` is, by filename alone - enough for watcher
/// events, which are already scoped to the candidate directories. Editor
/// saves go through `note_saved`, which also compares the full path.
fn classify(path: &Path) -> Option<WatchedFile> {
    match path.file_name()?.to_str()? {
        "config.toml" => Some(WatchedFile::Config),
        "keybinds.toml" => Some(WatchedFile::Keybinds),
        _ => None,
    }
}

/// On-disk identity cheap enough to stat on demand: which candidate path is
/// effective, its mtime, and its length. mtime granularity is
/// filesystem-dependent, so a same-length rewrite within one tick can slip
/// past a comparison - `note_saved` marks dirty unconditionally to cover
/// the editor's own saves, and the residual for external ones is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    path: PathBuf,
    mtime: SystemTime,
    len: u64,
}

fn fingerprint(file: WatchedFile) -> Option<Fingerprint> {
    let path = file.effective_path()?;
    let metadata = std::fs::metadata(&path).ok()?;
    let mtime = metadata.modified().ok()?;
    Some(Fingerprint { path, mtime, len: metadata.len() })
}

struct FileState {
    fingerprint: Option<Fingerprint>,
    dirty: bool,
}

impl FileState {
    /// Records `current` and reports whether the file moved - appeared,
    /// disappeared, or changed in place.
    fn observe(&mut self, current: Option<Fingerprint>) -> bool {
        let moved = self.fingerprint != current;
        self.fingerprint = current;
        moved
    }
}

pub struct ConfigWatch {
    debounce: Debounce,
    config: FileState,
    keybinds: FileState,
}

impl ConfigWatch {
    /// Seeds fingerprints from what's on disk, so boot state never reads as
    /// a change.
    pub fn new() -> Self {
        Self::seeded(
            fingerprint(WatchedFile::Config),
            fingerprint(WatchedFile::Keybinds),
        )
    }

    fn seeded(config: Option<Fingerprint>, keybinds: Option<Fingerprint>) -> Self {
        Self {
            debounce: Debounce::new(RELOAD_DEBOUNCE),
            config: FileState { fingerprint: config, dirty: false },
            keybinds: FileState { fingerprint: keybinds, dirty: false },
        }
    }

    /// The editor saved `path`. A save to the effective config file is
    /// dirty by definition, without consulting the fingerprint (see
    /// [`Fingerprint`] on why it can miss a rewrite).
    pub fn note_saved(&mut self, path: &Path, now: Instant) {
        let Some(file) = classify(path) else {
            return;
        };
        let Some(effective) = file.effective_path() else {
            return;
        };
        // The saved path is absolute (it came from a dialog or a restored
        // tab); the effective candidate can be the relative `config.toml`.
        // Both exist right now, so canonicalizing both makes them comparable.
        let same = match (std::fs::canonicalize(path), std::fs::canonicalize(&effective)) {
            (Ok(saved), Ok(effective)) => saved == effective,
            _ => false,
        };
        if same {
            self.mark_dirty(file, now);
        }
    }

    /// A native file-system event touched `file`.
    pub fn note_event(&mut self, file: WatchedFile, now: Instant) {
        self.mark_dirty(file, now);
    }

    /// Focus-gain check: stat both files and dirty whichever moved since
    /// last observed.
    pub fn check(&mut self, now: Instant) {
        self.check_with(
            fingerprint(WatchedFile::Config),
            fingerprint(WatchedFile::Keybinds),
            now,
        );
    }

    fn check_with(
        &mut self,
        config: Option<Fingerprint>,
        keybinds: Option<Fingerprint>,
        now: Instant,
    ) {
        if self.config.observe(config) {
            self.mark_dirty(WatchedFile::Config, now);
        }
        if self.keybinds.observe(keybinds) {
            self.mark_dirty(WatchedFile::Keybinds, now);
        }
    }

    fn mark_dirty(&mut self, file: WatchedFile, now: Instant) {
        match file {
            WatchedFile::Config => self.config.dirty = true,
            WatchedFile::Keybinds => self.keybinds.dirty = true,
        }
        self.debounce.poke(now);
    }

    /// Whether a reload burst is waiting - gates the settle timer.
    pub fn pending(&self) -> bool {
        self.debounce.pending()
    }

    /// The files to reload now: non-empty at most once per settled burst.
    /// Dirt is cleared even if the caller's reload then fails to parse -
    /// the next save re-dirties, so a broken file surfaces one error
    /// instead of a retry loop.
    pub fn settled(&mut self, now: Instant) -> Vec<WatchedFile> {
        self.settled_with(
            fingerprint(WatchedFile::Config),
            fingerprint(WatchedFile::Keybinds),
            now,
        )
    }

    fn settled_with(
        &mut self,
        config: Option<Fingerprint>,
        keybinds: Option<Fingerprint>,
        now: Instant,
    ) -> Vec<WatchedFile> {
        if !self.debounce.fire_if_settled(now) {
            return Vec::new();
        }
        // Re-observe so the next focus check compares against the state
        // this reload is about to read, not what it replaced.
        self.config.fingerprint = config;
        self.keybinds.fingerprint = keybinds;

        let mut files = Vec::new();
        if std::mem::take(&mut self.config.dirty) {
            files.push(WatchedFile::Config);
        }
        if std::mem::take(&mut self.keybinds.dirty) {
            files.push(WatchedFile::Keybinds);
        }
        files
    }
}

/// Streams config-file activity from the OS's file watcher. Same shape as
/// `hotkey::subscription` - a plain `fn` stream, so iced starts it once.
pub fn subscription() -> Subscription<Message> {
    Subscription::run(watch_events).map(Message::ConfigFileEvent)
}

fn watch_events() -> impl iced::futures::Stream<Item = WatchedFile> {
    iced::stream::channel(16, |output: iced::futures::channel::mpsc::Sender<WatchedFile>| async move {
        // Held across the pending() so the OS registration lives as long
        // as the app; the callback inside is what delivers events. If the
        // watcher couldn't start, the stream stays open but silent - saves
        // and focus checks still trigger reloads.
        let _watcher = start_watcher(output);
        std::future::pending::<()>().await
    })
}

fn start_watcher(
    output: iced::futures::channel::mpsc::Sender<WatchedFile>,
) -> Option<notify::RecommendedWatcher> {
    let callback = move |event: Result<notify::Event, notify::Error>| {
        let Ok(event) = event else {
            return;
        };
        // Reads can't change a file.
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        for file in event.paths.iter().filter_map(|path| classify(path)) {
            // `try_send` needs `&mut`; cloning the sender is cheap.
            let _ = output.clone().try_send(file);
        }
    };
    let mut watcher = match notify::recommended_watcher(callback) {
        Ok(watcher) => watcher,
        Err(err) => {
            eprintln!(
                "jumppad: config watcher unavailable ({err}) - \
                 config reloads still happen on save and window focus"
            );
            return None;
        }
    };
    // Directories, not the files themselves: a directory watch survives the
    // delete/rename dance editors use for atomic saves, where a file watch
    // is lost with the old inode on some platforms.
    for dir in jumppad_config::candidate_dirs() {
        if let Err(err) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
            eprintln!(
                "jumppad: couldn't watch {} for config changes: {err}",
                dir.display()
            );
        }
    }
    Some(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint_at(path: &str, mtime_secs: u64, len: u64) -> Option<Fingerprint> {
        Some(Fingerprint {
            path: PathBuf::from(path),
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(mtime_secs),
            len,
        })
    }

    fn beyond_debounce() -> Duration {
        RELOAD_DEBOUNCE + Duration::from_millis(1)
    }

    #[test]
    fn classify_matches_the_two_config_filenames_only() {
        assert_eq!(classify(Path::new("/some/dir/config.toml")), Some(WatchedFile::Config));
        assert_eq!(classify(Path::new("keybinds.toml")), Some(WatchedFile::Keybinds));
        assert_eq!(classify(Path::new("/some/dir/notes.txt")), None);
        assert_eq!(classify(Path::new("/some/dir/config.toml.bak")), None);
    }

    #[test]
    fn observe_reports_every_kind_of_movement_once() {
        let mut state = FileState { fingerprint: fingerprint_at("config.toml", 1, 10), dirty: false };

        assert!(!state.observe(fingerprint_at("config.toml", 1, 10)), "unchanged");
        assert!(state.observe(fingerprint_at("config.toml", 2, 10)), "mtime moved");
        assert!(state.observe(fingerprint_at("config.toml", 2, 11)), "length moved");
        assert!(state.observe(fingerprint_at("other/config.toml", 2, 11)), "candidate flipped");
        assert!(state.observe(None), "disappeared");
        assert!(state.observe(fingerprint_at("config.toml", 3, 5)), "appeared");
        // Observation recorded the new state each time - a repeat is quiet.
        assert!(!state.observe(fingerprint_at("config.toml", 3, 5)));
    }

    #[test]
    fn a_settled_burst_reports_each_dirty_file_once() {
        let mut watch = ConfigWatch::seeded(None, None);
        let start = Instant::now();
        watch.note_event(WatchedFile::Config, start);
        watch.note_event(WatchedFile::Keybinds, start);
        assert!(watch.pending());

        assert_eq!(
            watch.settled_with(None, None, start + beyond_debounce()),
            vec![WatchedFile::Config, WatchedFile::Keybinds]
        );
        assert!(!watch.pending());
        assert!(watch.settled_with(None, None, start + beyond_debounce() * 2).is_empty());
    }

    #[test]
    fn an_unsettled_burst_reports_nothing() {
        let mut watch = ConfigWatch::seeded(None, None);
        let start = Instant::now();
        watch.note_event(WatchedFile::Config, start);
        assert!(watch.settled_with(None, None, start + RELOAD_DEBOUNCE / 2).is_empty());
        assert!(watch.pending(), "the burst is still waiting");
    }

    #[test]
    fn a_fingerprint_check_dirties_only_what_moved() {
        let seeded = fingerprint_at("config.toml", 1, 10);
        let mut watch = ConfigWatch::seeded(seeded.clone(), None);
        let start = Instant::now();

        // Nothing moved: no burst starts.
        watch.check_with(seeded.clone(), None, start);
        assert!(!watch.pending());

        // Only the config file moved: keybinds stays clean.
        watch.check_with(fingerprint_at("config.toml", 2, 10), None, start);
        assert_eq!(
            watch.settled_with(None, None, start + beyond_debounce()),
            vec![WatchedFile::Config]
        );
    }

    #[test]
    fn note_saved_ignores_paths_that_are_not_a_config_file() {
        let mut watch = ConfigWatch::seeded(None, None);
        watch.note_saved(Path::new("/tmp/notes.txt"), Instant::now());
        assert!(!watch.pending());
    }
}
