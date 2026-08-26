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
    fn reload_text(&mut self, text: &str) {
        self.0 = text.to_string();
    }
    fn poll_highlighting(&mut self) {}
    fn cursor_position(&self) -> (usize, usize) {
        (0, 0)
    }
    fn move_cursor_to(&mut self, _line: usize, _column: usize) {}
    fn selection(&self) -> Option<editor_core::SavedSelection> {
        None
    }
    fn restore_selection(
        &mut self,
        _selection: editor_core::SavedSelection,
        _cursor: (usize, usize),
    ) {
    }
    fn set_find_matches(
        &mut self,
        _matches: Vec<editor_core::FindMatch>,
        _current: Option<usize>,
    ) {
    }
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
    let dir = std::env::temp_dir()
        .join(format!("jumppad-session-test-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn manifest_round_trips_through_disk() {
    let dir = scratch_dir();
    let factory = stub_factory();

    let untitled_dirty = Tab::restored(0, None, "hello", true, &factory);
    let clean_file = Tab::restored(
        1,
        Some(PathBuf::from("/tmp/file.txt")),
        "",
        false,
        &factory,
    );
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

    let loaded = load_manifest(std::slice::from_ref(&dir))
        .expect("manifest should parse back");
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

    assert!(
        !draft_path(&dir, 42).exists(),
        "orphaned draft should be pruned"
    );
    assert!(
        draft_path(&dir, 7).exists(),
        "still-referenced draft should survive"
    );

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
