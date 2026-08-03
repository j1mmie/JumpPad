mod debounce;
mod find;
mod tab;
mod theme;
mod widget;

pub use debounce::Debounce;
pub use find::{FindMatch, find_matches};
pub use tab::{Document, Tab};
pub use theme::{
    FLOATING_SURFACE_DARKEN, SCROLLBAR_THUMB_WASH, WASH_ALPHA_CEILING, darkening_wash,
    scrollbar_wash,
};
pub use widget::{
    EDITOR_WIDGET_ID, EditorFactory, EditorMessage, SavedSelection, SelectionKind,
    TextEditorWidget,
};
