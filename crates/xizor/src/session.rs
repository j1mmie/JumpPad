//! Draft autosave / crash-recovery persistence.
//!
//! One `session.toml` manifest lists every open tab's id/path/dirty state
//! plus which was active; each dirty tab also gets a `<id>.draft` file
//! holding its unsaved content.

use std::path::{Path, PathBuf};

use editor_core::Tab;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionManifest {
    pub active: usize,
    pub tabs: Vec<TabEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabEntry {
    pub id: u64,
    pub path: Option<PathBuf>,
    pub dirty: bool,
}

const MANIFEST_FILE: &str = "session.toml";

/// Where the session manifest and draft files live: next to the running
/// executable, then `./drafts` (a `cargo run` convenience).
pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("drafts"));
        }
    }
    dirs.push(PathBuf::from("drafts"));
    dirs
}

/// Reads and parses the first `session.toml` found among `candidates`.
/// Returns `None` if none exist or it's unparseable - never blocks startup.
pub fn load_manifest(candidates: &[PathBuf]) -> Option<SessionManifest> {
    for dir in candidates {
        let path = dir.join(MANIFEST_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        return match toml::from_str(&text) {
            Ok(manifest) => Some(manifest),
            Err(err) => {
                eprintln!(
                    "xizor: {}: {err}, starting without a restored session",
                    path.display()
                );
                None
            }
        };
    }
    None
}

pub fn draft_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{id}.draft"))
}

/// Snapshots the current tab list into a `SessionManifest` - pure, no I/O.
pub fn build_manifest(tabs: &[Tab], active: usize) -> SessionManifest {
    SessionManifest {
        active,
        tabs: tabs
            .iter()
            .map(|tab| TabEntry {
                id: tab.id,
                path: tab.document.path.clone(),
                dirty: tab.dirty,
            })
            .collect(),
    }
}

/// Writes `manifest` to `dir/session.toml`, then deletes any `<id>.draft`
/// file that no longer belongs to a dirty tab in `manifest`.
pub fn write_manifest_sync(dir: &Path, manifest: &SessionManifest) {
    if let Err(err) = std::fs::create_dir_all(dir) {
        eprintln!("xizor: couldn't create {}: {err}", dir.display());
        return;
    }
    match toml::to_string_pretty(manifest) {
        Ok(text) => {
            if let Err(err) = std::fs::write(dir.join(MANIFEST_FILE), text) {
                eprintln!("xizor: couldn't write session manifest: {err}");
            }
        }
        Err(err) => eprintln!("xizor: couldn't serialize session manifest: {err}"),
    }

    prune_orphaned_drafts(dir, manifest);
}

fn prune_orphaned_drafts(dir: &Path, manifest: &SessionManifest) {
    let live_ids: std::collections::HashSet<u64> = manifest
        .tabs
        .iter()
        .filter(|entry| entry.dirty)
        .map(|entry| entry.id)
        .collect();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("draft") {
            continue;
        }
        let is_live = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u64>().ok())
            .is_some_and(|id| live_ids.contains(&id));
        if !is_live {
            if let Err(err) = std::fs::remove_file(&path) {
                eprintln!("xizor: couldn't remove stale draft {}: {err}", path.display());
            }
        }
    }
}

/// Every tab whose content has changed since its draft was last written -
/// `(id, generation, text)`.
pub fn stale_tabs(tabs: &[Tab]) -> Vec<(u64, u64, String)> {
    tabs.iter()
        .filter(|tab| tab.dirty && tab.draft_generation != tab.flushed_generation)
        .map(|tab| (tab.id, tab.draft_generation, tab.editor.text()))
        .collect()
}

/// Writes one tab's draft content asynchronously. Best-effort: errors are
/// logged, not surfaced. Returns `(id, generation)` for the caller to mark flushed.
pub async fn flush_draft_async(dir: PathBuf, id: u64, generation: u64, text: String) -> (u64, u64) {
    if let Err(err) = tokio::fs::create_dir_all(&dir).await {
        eprintln!("xizor: couldn't create {}: {err}", dir.display());
        return (id, generation);
    }
    if let Err(err) = tokio::fs::write(draft_path(&dir, id), text).await {
        eprintln!("xizor: couldn't write draft for tab {id}: {err}");
    }
    (id, generation)
}

/// Synchronous last-ditch flush run when the app is quitting: writes the
/// manifest and every stale tab's draft before the window closes.
pub fn flush_on_exit(dir: &Path, tabs: &[Tab], active: usize) {
    let manifest = build_manifest(tabs, active);
    write_manifest_sync(dir, &manifest);
    for (id, _generation, text) in stale_tabs(tabs) {
        if let Err(err) = std::fs::write(draft_path(dir, id), text) {
            eprintln!("xizor: couldn't write draft for tab {id} on exit: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{EditorFactory, EditorMessage, TextEditorWidget};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A minimal `TextEditorWidget` stub for building real `Tab`s in tests.
    struct StubEditor(String);

    impl TextEditorWidget for StubEditor {
        fn view(&self) -> iced::Element<'_, EditorMessage> {
            iced::widget::text(self.0.clone()).into()
        }
        fn update(&mut self, _message: EditorMessage) -> bool {
            false
        }
        fn text(&self) -> String {
            self.0.clone()
        }
        fn set_text(&mut self, text: &str) {
            self.0 = text.to_string();
        }
        fn poll_highlighting(&mut self) {}
        fn cursor_position(&self) -> (usize, usize) {
            (0, 0)
        }
        fn move_cursor_to(&mut self, _line: usize, _column: usize) {}
        fn has_pending_highlighting(&self) -> bool {
            false
        }
    }

    fn stub_factory() -> EditorFactory {
        Box::new(|text: &str, _extension: Option<&str>| {
            Box::new(StubEditor(text.to_string())) as Box<dyn TextEditorWidget>
        })
    }

    /// A fresh scratch directory per test, under the OS temp dir.
    fn scratch_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "xizor-session-test-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn manifest_round_trips_through_disk() {
        let dir = scratch_dir();
        let factory = stub_factory();

        let untitled_dirty = Tab::restored(0, None, "hello", true, &factory);
        let clean_file = Tab::restored(1, Some(PathBuf::from("/tmp/file.txt")), "", false, &factory);
        let dirty_file = Tab::restored(
            2,
            Some(PathBuf::from("/tmp/file2.txt")),
            "edited",
            true,
            &factory,
        );
        let tabs = vec![untitled_dirty, clean_file, dirty_file];

        let manifest = build_manifest(&tabs, 2);
        write_manifest_sync(&dir, &manifest);

        // write_manifest_sync never writes draft files itself - simulate that separately.
        std::fs::write(draft_path(&dir, 0), "hello").unwrap();
        std::fs::write(draft_path(&dir, 2), "edited").unwrap();

        let loaded = load_manifest(std::slice::from_ref(&dir)).expect("manifest should parse back");
        assert_eq!(loaded.active, 2);
        assert_eq!(loaded.tabs.len(), 3);
        assert_eq!(loaded.tabs[0].id, 0);
        assert_eq!(loaded.tabs[0].path, None);
        assert!(loaded.tabs[0].dirty);
        assert_eq!(loaded.tabs[1].path, Some(PathBuf::from("/tmp/file.txt")));
        assert!(!loaded.tabs[1].dirty);
        assert_eq!(loaded.tabs[2].path, Some(PathBuf::from("/tmp/file2.txt")));
        assert!(loaded.tabs[2].dirty);

        assert_eq!(
            std::fs::read_to_string(draft_path(&dir, 0)).unwrap(),
            "hello"
        );
        assert_eq!(
            std::fs::read_to_string(draft_path(&dir, 2)).unwrap(),
            "edited"
        );
        // The clean tab never gets a draft file at all.
        assert!(!draft_path(&dir, 1).exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_tabs_only_reports_unflushed_dirty_content() {
        let factory = stub_factory();
        let mut clean = Tab::untitled(0, &factory);
        let mut dirty_unflushed = Tab::untitled(1, &factory);
        dirty_unflushed.dirty = true;
        dirty_unflushed.draft_generation = 2;
        dirty_unflushed.flushed_generation = 1;
        let mut dirty_flushed = Tab::untitled(2, &factory);
        dirty_flushed.dirty = true;
        dirty_flushed.draft_generation = 3;
        dirty_flushed.flushed_generation = 3;
        clean.dirty = false;

        let tabs = vec![clean, dirty_unflushed, dirty_flushed];
        let stale = stale_tabs(&tabs);

        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].0, 1);
        assert_eq!(stale[0].1, 2);
    }

    #[test]
    fn write_manifest_sync_prunes_orphaned_drafts() {
        let dir = scratch_dir();
        std::fs::create_dir_all(&dir).unwrap();

        // Simulate leftover draft files from tabs that no longer exist (or
        // no longer need one) in the manifest being written now.
        std::fs::write(draft_path(&dir, 42), "stale").unwrap();
        std::fs::write(draft_path(&dir, 7), "still live").unwrap();

        let manifest = SessionManifest {
            active: 0,
            tabs: vec![TabEntry {
                id: 7,
                path: None,
                dirty: true,
            }],
        };
        write_manifest_sync(&dir, &manifest);

        assert!(!draft_path(&dir, 42).exists(), "orphaned draft should be pruned");
        assert!(draft_path(&dir, 7).exists(), "still-referenced draft should survive");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_manifest_returns_none_when_missing_or_corrupt() {
        let dir = scratch_dir();
        assert!(load_manifest(std::slice::from_ref(&dir)).is_none());

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), "not valid toml {{{").unwrap();
        assert!(load_manifest(std::slice::from_ref(&dir)).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
