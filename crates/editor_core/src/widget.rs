use iced::advanced::text::editor as text_editor;

use crate::find::FindMatch;

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

    /// Replaces the document with new on-disk contents, keeping the scroll
    /// position, clamping the caret, and recording an undo step - a file that
    /// changed underneath the editor undoes like any other change, unlike
    /// `set_text`, which is for loading a different file into the tab.
    fn reload_text(&mut self, text: &str);

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

    /// The current selection, if text is selected - saved by the app shell
    /// on deselect, like `cursor_position`.
    fn selection(&self) -> Option<SavedSelection>;

    /// Restores a previously saved selection, with the moving end at
    /// `cursor`, clamping like `move_cursor_to` does.
    fn restore_selection(&mut self, selection: SavedSelection, cursor: (usize, usize));

    /// Ranges to mark as find matches, and which of them is current.
    /// An empty slice clears the marking.
    ///
    /// Marking goes through the widget rather than the app shell reusing the
    /// editor's selection because iced's `text_editor` only draws a
    /// selection while it is focused - and the find field holds focus
    /// exactly when matches need to be visible (see AGENTS.md).
    fn set_find_matches(&mut self, matches: Vec<FindMatch>, current: Option<usize>);

    /// Whether this editor is still waiting on a grammar load - its own, or
    /// an injection target its grammar needs to color embedded content.
    /// Used by the app shell to decide whether the highlighting-poll
    /// subscription needs to stay active.
    fn has_pending_highlighting(&self) -> bool;
}

/// The `iced::advanced::widget::Id` every `TextEditorWidget` implementation
/// tags its focusable widget with, so the app shell can restore focus to it
/// deterministically (`focusable::focus_next` skips the editor when
/// something is already focused at operation time).
pub const EDITOR_WIDGET_ID: &str = "editor";

/// A selection saved when a tab is deselected: the fixed anchor plus how
/// the selection was made. Word/line selections (double/triple click) keep
/// their anchor at the click position *inside* the selected region - the
/// bounds live in the kind, so an anchor-to-cursor range can't rebuild them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedSelection {
    pub anchor: (usize, usize),
    pub kind: SelectionKind,
}

/// How a [`SavedSelection`] gets rebuilt on restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// A plain anchor-to-cursor span (mouse drag, shift+arrows, select-all).
    Range,
    /// The word around the anchor (double click).
    Word,
    /// The line containing the anchor (triple click).
    Line,
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
    /// Comment or uncomment the cursor's line / the selection's lines.
    /// Like `Undo`, not a `text_editor::Action` - the transformation is the
    /// widget's own logic.
    ToggleComment,
    /// Remove the cursor's line / the selection's lines outright. Like
    /// `ToggleComment`, whole-line surgery the widget does itself.
    DeleteLine,
    /// Swap the covered lines with the line above them.
    MoveLineUp,
    /// Swap the covered lines with the line below them.
    MoveLineDown,
    /// Duplicate the covered lines, leaving the cursor on the upper copy.
    CopyLineUp,
    /// Duplicate the covered lines, leaving the cursor on the lower copy.
    CopyLineDown,
    /// Scroll the view by a number of *pixels*, which is what the wheel and
    /// the scrollbar thumb actually ask for. Not a `text_editor::Action`:
    /// `Action::Scroll` counts in whole lines and would snap the view to a
    /// line boundary (see AGENTS.md).
    Scroll(f32),
}

impl EditorMessage {
    /// Applies the OS-standard shift+click rule the underlying widget lacks:
    /// a single click while shift is held becomes `Action::Drag`, which
    /// extends the selection from the current cursor to the clicked position
    /// instead of just moving the cursor there. Everything else - including
    /// double/triple clicks, which arrive as `SelectWord`/`SelectLine` -
    /// passes through untouched.
    pub fn with_shift_click(self, shift_held: bool) -> Self {
        match self {
            Self::Action(text_editor::Action::Click(position)) if shift_held => {
                Self::Action(text_editor::Action::Drag(position))
            }
            message => message,
        }
    }
}

/// Constructs a boxed editor widget seeded with the given initial text and,
/// if known, the file extension it was opened from (no leading dot). A
/// boxed closure so the app shell can capture shared state (e.g. a syntax
/// registry) once, without knowing the concrete widget type.
pub type EditorFactory = Box<dyn Fn(&str, Option<&str>) -> Box<dyn TextEditorWidget>>;

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Point;

    #[test]
    fn shift_click_becomes_a_selection_extending_drag() {
        let click = EditorMessage::Action(text_editor::Action::Click(Point::new(3.0, 7.0)));
        assert!(matches!(
            click.with_shift_click(true),
            EditorMessage::Action(text_editor::Action::Drag(position))
                if position == Point::new(3.0, 7.0)
        ));
    }

    #[test]
    fn plain_click_stays_a_click() {
        let click = EditorMessage::Action(text_editor::Action::Click(Point::ORIGIN));
        assert!(matches!(
            click.with_shift_click(false),
            EditorMessage::Action(text_editor::Action::Click(_))
        ));
    }

    #[test]
    fn shift_double_click_word_selection_is_untouched() {
        let double = EditorMessage::Action(text_editor::Action::SelectWord);
        assert!(matches!(
            double.with_shift_click(true),
            EditorMessage::Action(text_editor::Action::SelectWord)
        ));
    }

    #[test]
    fn non_action_messages_pass_through() {
        assert!(matches!(EditorMessage::Undo.with_shift_click(true), EditorMessage::Undo));
    }
}
