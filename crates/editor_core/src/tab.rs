use std::path::PathBuf;

use crate::widget::{EditorFactory, TextEditorWidget};

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
    /// Where the cursor was the last time this tab was the active one -
    /// saved on deselect, restored on reselect (see `XizorApp`'s tab
    /// switching). Starts at the document start, same as a freshly created
    /// editor's actual cursor position.
    pub last_cursor: (usize, usize),
    /// Bumped every time an edit changes the text. Compared against
    /// `flushed_generation` to tell whether this tab's on-disk draft file
    /// (see `xizor`'s `session` module) is stale - the single predicate for
    /// "does this tab need a draft write" used by both the periodic
    /// autosave timer and the exit-time flush.
    pub draft_generation: u64,
    /// The `draft_generation` value that was last successfully written to
    /// this tab's draft file.
    pub flushed_generation: u64,
}

impl Tab {
    pub fn untitled(id: u64, factory: &EditorFactory) -> Self {
        Self {
            id,
            document: Document::default(),
            dirty: false,
            editor: factory("", None),
            last_cursor: (0, 0),
            draft_generation: 0,
            flushed_generation: 0,
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
            draft_generation: 0,
            flushed_generation: 0,
        }
    }

    /// Builds a tab from a restored session entry (see `xizor`'s `session`
    /// module): either a draft file's content (dirty tabs, with or without
    /// a real path) or a fresh read of the real file (clean, file-backed
    /// tabs) - the caller decides which `content` to pass in and what
    /// `dirty` should be; this constructor just assembles the `Tab`.
    /// `flushed_generation` starts equal to `draft_generation` (both `0`)
    /// since `content` is, by construction, already what's on disk.
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
            draft_generation: 0,
            flushed_generation: 0,
        }
    }

    pub fn title(&self) -> String {
        let name = self.document.display_name();
        if self.dirty {
            format!("{name} \u{2022}")
        } else {
            name
        }
    }
}
