mod comment;
mod drag_scroll;
pub mod font;
mod highlighter;
mod history;
mod indent;
mod keybindings;
mod line_edit;
mod lines;
mod safe_area;
mod scrollbar;
mod shared_config;
mod style;
mod text_area;
mod text_delta;
pub mod text_editor;
mod word;

pub use comment::CommentStyle;
pub use indent::{Indentation, IndentationStyle};
pub use keybindings::{KeyResolver, binding_for};
pub use shared_config::SharedEditorConfig;
pub use style::scrollbar_thumb_style;
pub use text_area::TextArea;
pub use word::WordSeparators;

/// Re-exported so the app can write the [`KeyResolver`] it hands down without
/// naming the widget module.
pub use text_editor::KeyPress;

// Test-only: these live in sibling modules and are otherwise reached only
// through fully-qualified paths, but `lib_tests.rs`'s `use super::*` needs
// them bound here by name, the same way it needs everything else in this
// file (same pattern as the `jumppad_config` and `app.rs` splits).
#[cfg(test)]
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use editor_core::{
    EditorMessage, FindMatch, SavedSelection, SelectionKind, TextEditorWidget,
};
#[cfg(test)]
use iced::advanced::text::Highlighter;
#[cfg(test)]
use iced::{Background, Color, Theme};
#[cfg(test)]
use jumppad_actions::Action;
#[cfg(test)]
use syntax_registry::{HighlightCategory, SyntaxRegistry};

#[cfg(test)]
use highlighter::{Highlighted, HighlighterSettings, TreeSitterHighlighter, base_color_for, color_for};
#[cfg(test)]
use keybindings::key_binding;
#[cfg(test)]
use style::{apply_alpha, editor_style};
#[cfg(test)]
use text_area::clamp_position;
#[cfg(test)]
use text_editor::{Binding, Content, Cursor, Motion, Position, Status};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
