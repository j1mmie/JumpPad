mod tab;
mod widget;

pub use tab::{Document, Tab};
pub use widget::{
    EDITOR_WIDGET_ID, EditorFactory, EditorMessage, SavedSelection, SelectionKind,
    TextEditorWidget,
};
