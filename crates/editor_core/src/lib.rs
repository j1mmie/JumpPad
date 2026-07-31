mod find;
mod tab;
mod theme;
mod widget;

pub use find::{FindMatch, find_matches};
pub use tab::{Document, Tab};
pub use theme::{FLOATING_SURFACE_DARKEN, WASH_ALPHA_CEILING, darkening_wash};
pub use widget::{
    EDITOR_WIDGET_ID, EditorFactory, EditorMessage, SavedSelection, SelectionKind,
    TextEditorWidget,
};
