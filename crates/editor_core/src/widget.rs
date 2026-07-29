use iced::widget::text_editor;

/// The abstraction boundary between the app shell (tabs, menus, file I/O)
/// and whatever actually renders and edits text on screen.
///
/// Swapping text-editing implementations (e.g. moving off
/// `iced::widget::text_editor` to a custom rope-backed widget) means writing
/// a new implementation of this trait — the rest of the app does not need
/// to change.
pub trait TextEditorWidget {
    /// Builds the editor's view, filling the available space.
    fn view(&self) -> iced::Element<'_, EditorMessage>;

    /// Applies a message produced by a previous `view()`. Returns `true` if
    /// the text was modified.
    fn update(&mut self, message: EditorMessage) -> bool;

    /// The current contents of the editor.
    fn text(&self) -> String;

    /// Replace the editor's contents wholesale (e.g. after loading a file).
    fn set_text(&mut self, text: &str);

    /// Advances any in-progress syntax-highlighting grammar load one step.
    /// The app shell calls this periodically, since iced only re-runs app
    /// code in response to a message.
    fn poll_highlighting(&mut self);

    /// The cursor's current (line, column) - saved by the app shell when a
    /// tab is deselected, so `move_cursor_to` can put it back on reselect.
    fn cursor_position(&self) -> (usize, usize);

    /// Moves the cursor to the given (line, column), clamping to the
    /// nearest valid position if the document has since gotten shorter.
    fn move_cursor_to(&mut self, line: usize, column: usize);

    /// Whether this editor is still waiting on a grammar load - used by the
    /// app shell to decide whether the highlighting-poll subscription needs
    /// to stay active.
    fn has_pending_highlighting(&self) -> bool;
}

/// The message type produced by a `TextEditorWidget`'s view. Concrete, not
/// generic per-backend, since there's exactly one editor implementation live at a time.
#[derive(Debug, Clone)]
pub enum EditorMessage {
    Action(text_editor::Action),
    /// Restore the most recent entry from this editor's own undo history -
    /// not a `text_editor::Action` since `text_editor::Content` has no undo/redo of its own.
    Undo,
    /// Mirror of `Undo`, restoring the most recently undone entry.
    Redo,
}

/// Constructs a boxed editor widget seeded with the given initial text and,
/// if known, the file extension it was opened from (no leading dot). A
/// boxed closure so the app shell can capture shared state (e.g. a syntax
/// registry) once, without knowing the concrete widget type.
pub type EditorFactory = Box<dyn Fn(&str, Option<&str>) -> Box<dyn TextEditorWidget>>;
