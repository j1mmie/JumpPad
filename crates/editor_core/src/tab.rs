use std::path::PathBuf;

use crate::disk::DiskStamp;
use crate::widget::{EditorFactory, SavedSelection, TextEditorWidget};

/// Everything about a tab that is *not* the live text buffer: where it's
/// saved (if anywhere) and whether it has unsaved changes.
#[derive(Default)]
pub struct Document {
    pub path: Option<PathBuf>,
}

impl Document {
    pub fn display_name(&self) -> String {
        match &self.path {
            Some(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".to_string()),
            None => "Untitled".to_string(),
        }
    }
}

/// One open tab: metadata plus a boxed, swappable editor widget that owns
/// the actual text buffer.
pub struct Tab {
    pub id: u64,
    pub document: Document,
    pub dirty: bool,
    pub editor: Box<dyn TextEditorWidget>,
    /// Where the cursor was the last time this tab was active - saved on
    /// deselect, restored on reselect.
    pub last_cursor: (usize, usize),
    /// The selection the last time this tab was active (the cursor is a
    /// range selection's other end) - in-memory only, never written to the
    /// session manifest, and dropped with the tab.
    pub last_selection: Option<SavedSelection>,
    /// Bumped on every edit; compared against `flushed_generation` to tell
    /// whether this tab's draft file is stale.
    pub draft_generation: u64,
    /// The `draft_generation` value that was last successfully written to
    /// this tab's draft file.
    pub flushed_generation: u64,
    /// The file as JumpPad last saw it. `None` for an untitled tab, or one
    /// whose file isn't on disk (named on the command line and never saved,
    /// or deleted underneath us).
    pub disk: Option<DiskStamp>,
    /// The file changed on disk while this tab had unsaved edits. Drives the
    /// banner and the save-time conflict prompt; cleared by a reload, a
    /// successful save, or the user acknowledging it.
    pub externally_changed: bool,
}

impl Tab {
    pub fn untitled(id: u64, factory: &EditorFactory) -> Self {
        Self {
            id,
            document: Document::default(),
            dirty: false,
            editor: factory("", None),
            last_cursor: (0, 0),
            last_selection: None,
            draft_generation: 0,
            flushed_generation: 0,
            disk: None,
            externally_changed: false,
        }
    }

    pub fn from_file(id: u64, path: PathBuf, text: &str, factory: &EditorFactory) -> Self {
        let extension = path.extension().and_then(|ext| ext.to_str());
        let editor = factory(text, extension);
        Self {
            id,
            document: Document { path: Some(path) },
            dirty: false,
            editor,
            last_cursor: (0, 0),
            last_selection: None,
            draft_generation: 0,
            flushed_generation: 0,
            disk: None,
            externally_changed: false,
        }
    }

    /// Builds a tab from a restored session entry - the caller decides
    /// what `content` and `dirty` should be; this just assembles the `Tab`.
    pub fn restored(
        id: u64,
        path: Option<PathBuf>,
        content: &str,
        dirty: bool,
        factory: &EditorFactory,
    ) -> Self {
        let extension = path
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|ext| ext.to_str());
        let editor = factory(content, extension);
        Self {
            id,
            document: Document { path },
            dirty,
            editor,
            last_cursor: (0, 0),
            last_selection: None,
            draft_generation: 0,
            flushed_generation: 0,
            disk: None,
            externally_changed: false,
        }
    }

    /// Re-reads this tab's on-disk identity - what "the file as JumpPad last
    /// saw it" means after a load, a save, or a reload. Constructors leave it
    /// `None` so no `Tab` ever does I/O of its own; the app calls this.
    pub fn restamp(&mut self) {
        self.disk = self.document.path.as_deref().and_then(DiskStamp::of);
    }

    /// The tab chip's label: the file name, a bullet while there are unsaved
    /// changes, and a warning sign while the file has also moved on disk.
    pub fn title(&self) -> String {
        let mut title = self.document.display_name();
        if self.dirty {
            title.push_str(" \u{2022}");
        }
        if self.externally_changed {
            title.push_str(" \u{26a0}");
        }
        title
    }
}
