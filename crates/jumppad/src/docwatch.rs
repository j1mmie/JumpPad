//! Notices when an open tab's file changes on disk, and tells the app when
//! a settled burst is ready to act on.
//!
//! A sibling of `reload.rs`, but much smaller: it tracks no per-file state.
//! Every signal - a native file-system event, a window focus gain - only
//! pokes a debounce, and `JumpPadApp::sweep_documents` re-derives what
//! actually moved by stat'ing each tab's path. Nothing here matches event
//! paths against open files, so relative paths, deleted files and
//! atomic-save renames need no special handling; a spurious poke costs one
//! no-op sweep.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use editor_core::Debounce;
use iced::Subscription;
use notify::Watcher;

use crate::app::Message;

/// Same window as `reload::RELOAD_DEBOUNCE`: long enough to swallow another
/// editor's atomic-save dance (write, rename, write again), short enough to
/// feel immediate.
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(300);

/// Granularity of the settle check. Its timer only runs while a burst is
/// pending (see `JumpPadApp::subscription`), so this isn't an idle cost.
pub const SETTLE_TICK: Duration = Duration::from_millis(100);

/// The debounce between "something happened out there" and "go look."
pub struct DocumentWatch {
    debounce: Debounce,
}

impl DocumentWatch {
    pub fn new() -> Self {
        Self {
            debounce: Debounce::new(CHANGE_DEBOUNCE),
        }
    }

    /// A native file-system event touched a watched directory.
    pub fn note_event(&mut self, now: Instant) {
        self.debounce.poke(now);
    }

    /// Whether a change burst is waiting - gates the settle timer.
    pub fn pending(&self) -> bool {
        self.debounce.pending()
    }

    /// True once per settled burst: time to sweep the open tabs.
    pub fn settled(&mut self, now: Instant) -> bool {
        self.debounce.fire_if_settled(now)
    }
}

/// Streams file-system activity around the open files. `paths` is every
/// file-backed tab's path, **sorted and deduped**: the recipe hashes it, so
/// an unstable order would tear the watcher down and restart it on every
/// unrelated tab switch. Changing the set intentionally *is* how the
/// watched directories are updated.
pub fn subscription(paths: Vec<PathBuf>) -> Subscription<Message> {
    if paths.is_empty() {
        return Subscription::none();
    }
    // `run_with`'s builder is a bare `fn(&D) -> S`, not a closure - it can
    // capture nothing, so everything the watcher needs arrives in `paths`.
    Subscription::run_with(paths, watch_events).map(|()| Message::DocumentFileEvent)
}

// `&Vec` rather than `&[..]`: this is handed to `run_with` as a bare
// `fn(&D) -> S` pointer, and `D` is the `Vec` the recipe hashes. `use<>` keeps
// it one - from edition 2024 an `impl Trait` return captures every lifetime in
// scope unless told otherwise, which would make this borrow `paths`. It clones
// straight away, so there was never anything to capture.
#[allow(clippy::ptr_arg)]
fn watch_events(
    paths: &Vec<PathBuf>,
) -> impl iced::futures::Stream<Item = ()> + use<> {
    let paths = paths.clone();
    iced::stream::channel(16, |output: iced::futures::channel::mpsc::Sender<()>| async move {
        // Held across the pending() so the OS registration lives as long as
        // this recipe does. If the watcher couldn't start, the stream stays
        // open but silent - the focus sweep still notices changes.
        let _watcher = start_watcher(&paths, output);
        std::future::pending::<()>().await
    })
}

fn start_watcher(
    paths: &[PathBuf],
    output: iced::futures::channel::mpsc::Sender<()>,
) -> Option<notify::RecommendedWatcher> {
    // Filenames only, so a noisy directory (Downloads, a build output dir)
    // can't poke the debounce constantly. A false positive that slips
    // through is harmless - it costs one no-op sweep.
    let names: BTreeSet<std::ffi::OsString> = paths
        .iter()
        .filter_map(|path| path.file_name().map(ToOwned::to_owned))
        .collect();

    let callback = move |event: Result<notify::Event, notify::Error>| {
        let Ok(event) = event else {
            return;
        };
        // Reads can't change a file.
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        let touches_an_open_file = event
            .paths
            .iter()
            .any(|path| path.file_name().is_some_and(|name| names.contains(name)));
        if touches_an_open_file {
            // `try_send` needs `&mut`; cloning the sender is cheap.
            let _ = output.clone().try_send(());
        }
    };

    let mut watcher = match notify::recommended_watcher(callback) {
        Ok(watcher) => watcher,
        Err(err) => {
            eprintln!(
                "jumppad: file watcher unavailable ({err}) - \
                 open files are still re-checked when the window regains focus"
            );
            return None;
        }
    };
    // Directories, not the files themselves, and non-recursively: a
    // directory watch survives the delete/rename dance editors use for
    // atomic saves, where a file watch is lost with the old inode on some
    // platforms (same reasoning as `reload.rs`).
    for dir in watched_dirs(paths) {
        if let Err(err) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
            eprintln!(
                "jumppad: couldn't watch {} for file changes: {err}",
                dir.display()
            );
        }
    }
    Some(watcher)
}

/// The directories holding `paths`, deduped. A path with no parent (or an
/// empty one, as a bare relative filename has) watches the current directory.
fn watched_dirs(paths: &[PathBuf]) -> BTreeSet<PathBuf> {
    paths
        .iter()
        .map(|path| match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beyond_debounce() -> Duration {
        CHANGE_DEBOUNCE + Duration::from_millis(1)
    }

    #[test]
    fn a_settled_burst_reports_once() {
        let mut watch = DocumentWatch::new();
        let start = Instant::now();
        watch.note_event(start);
        watch.note_event(start + CHANGE_DEBOUNCE / 2);
        assert!(watch.pending());

        assert!(watch.settled(start + CHANGE_DEBOUNCE / 2 + beyond_debounce()));
        assert!(!watch.pending());
        assert!(
            !watch.settled(start + beyond_debounce() * 3),
            "a settled burst is consumed, not re-reported"
        );
    }

    #[test]
    fn an_unsettled_burst_reports_nothing() {
        let mut watch = DocumentWatch::new();
        let start = Instant::now();
        watch.note_event(start);
        assert!(!watch.settled(start + CHANGE_DEBOUNCE / 2));
        assert!(watch.pending(), "the burst is still waiting");
    }

    #[test]
    fn directories_are_derived_from_the_paths_and_deduped() {
        let dirs = watched_dirs(&[
            PathBuf::from("/home/user/notes/a.txt"),
            PathBuf::from("/home/user/notes/b.txt"),
            PathBuf::from("/etc/hosts"),
            PathBuf::from("relative.md"),
        ]);
        assert!(dirs.contains(&PathBuf::from("/home/user/notes")));
        assert!(dirs.contains(&PathBuf::from("/etc")));
        assert!(dirs.contains(&PathBuf::from(".")), "a bare name watches the cwd");
        assert_eq!(dirs.len(), 3);
    }
}
