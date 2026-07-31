mod find;
mod tab;
mod widget;

pub use find::{FindMatch, find_matches};
pub use tab::{Document, Tab};
pub use widget::{
    EDITOR_WIDGET_ID, EditorFactory, EditorMessage, SavedSelection, SelectionKind,
    TextEditorWidget,
};
