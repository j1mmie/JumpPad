//! Every action JumpPad can perform, named once.
//!
//! This crate knows *what* the product does and nothing about *how* anyone
//! asks for it. Keys live in `jumppad_keybinds`; a mouse or gesture binding
//! would be a sibling crate beside it, mapping its own input onto the same
//! [`Action`]. That only holds while this crate has no input dependency, so
//! it has none at all - see the note in its `Cargo.toml`.
//!
//! Adding an action is one row in [`actions!`], and the compiler finds the
//! rest: the enum, [`ACTIONS`] and [`Action::ALL`] are generated together, so
//! they cannot drift, and `jumppad`'s wiring test fails until something can
//! actually perform it.

use std::fmt;

/// Which layer performs an action - and so which crate maps it to something
/// that happens. `Editor` actions are handled inside the text widget,
/// `App` actions by the application shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    App,
    Editor,
}

/// When an action is eligible to fire.
///
/// Only the two states the code actually gates on today. A richer scheme -
/// VS Code's `when` expressions, say - would replace this enum without
/// disturbing the rows, since each row names a context rather than spelling
/// out a condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    /// Whenever the window has focus.
    Always,
    /// Only while the text editor holds focus.
    EditorFocused,
}

/// One action's full identity: what it is called in `keybinds.toml`, what to
/// show a human, who performs it, and when it applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionDef {
    pub action: Action,
    /// The `keybinds.toml` name. Stable once shipped - it is written down in
    /// users' config files, so renaming one silently breaks their bindings.
    pub name: &'static str,
    /// Human label, for a command palette, menu, or shortcut sheet.
    pub label: &'static str,
    pub category: Category,
    pub context: Context,
}

/// Declares the whole action set: the [`Action`] enum, the [`ACTIONS`] table
/// and [`Action::ALL`], all from one list, so a new action cannot be half
/// added. The generated order is shared, which is what lets [`Action::def`]
/// index straight into [`ACTIONS`].
macro_rules! actions {
    ($($variant:ident => $name:literal, $label:literal,
       $category:ident, $context:ident;)*) => {
        /// Something JumpPad can be asked to do.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Action {
            $($variant,)*
        }

        /// Every action, in declaration order.
        pub const ACTIONS: &[ActionDef] = &[
            $(ActionDef {
                action: Action::$variant,
                name: $name,
                label: $label,
                category: Category::$category,
                context: Context::$context,
            },)*
        ];

        impl Action {
            /// Every variant, in the same order as [`ACTIONS`].
            pub const ALL: &'static [Action] = &[$(Action::$variant,)*];
        }
    };
}

actions! {
    // Application shell.
    NewTab => "new_tab", "New Tab", App, Always;
    OpenFile => "open_file", "Open File", App, Always;
    SaveFile => "save_file", "Save", App, Always;
    SaveFileAs => "save_file_as", "Save As", App, Always;
    CloseActiveTab => "close_active_tab", "Close Tab", App, Always;
    SelectPreviousTab => "select_previous_tab", "Previous Tab", App, Always;
    SelectNextTab => "select_next_tab", "Next Tab", App, Always;
    SelectPreviousActiveTab =>
        "select_previous_active_tab", "Last Used Tab", App, Always;
    Find => "find", "Find", App, Always;
    FindNext => "find_next", "Find Next", App, Always;
    FindPrevious => "find_previous", "Find Previous", App, Always;

    // Text editing.
    WordDeleteBackward =>
        "word_delete_backward", "Delete Word Left", Editor, EditorFocused;
    WordDeleteForward =>
        "word_delete_forward", "Delete Word Right", Editor, EditorFocused;
    DocumentStart =>
        "document_start", "Go to Start of Document", Editor, EditorFocused;
    SelectDocumentStart =>
        "select_document_start", "Select to Start", Editor, EditorFocused;
    DocumentEnd =>
        "document_end", "Go to End of Document", Editor, EditorFocused;
    SelectDocumentEnd =>
        "select_document_end", "Select to End", Editor, EditorFocused;
    Undo => "undo", "Undo", Editor, EditorFocused;
    Redo => "redo", "Redo", Editor, EditorFocused;
    ToggleComment =>
        "toggle_comment", "Toggle Comment", Editor, EditorFocused;
    DeleteLine => "delete_line", "Delete Line", Editor, EditorFocused;
    MoveLineUp => "move_line_up", "Move Line Up", Editor, EditorFocused;
    MoveLineDown => "move_line_down", "Move Line Down", Editor, EditorFocused;
    CopyLineUp => "copy_line_up", "Duplicate Line Up", Editor, EditorFocused;
    CopyLineDown =>
        "copy_line_down", "Duplicate Line Down", Editor, EditorFocused;
}

impl Action {
    /// This action's row. `ACTIONS` and `ALL` are generated from one list, so
    /// the discriminant is the row's index.
    pub fn def(self) -> &'static ActionDef {
        &ACTIONS[self as usize]
    }

    /// The `keybinds.toml` name.
    pub fn name(self) -> &'static str {
        self.def().name
    }

    /// The human label.
    pub fn label(self) -> &'static str {
        self.def().label
    }

    pub fn category(self) -> Category {
        self.def().category
    }

    pub fn context(self) -> Context {
        self.def().context
    }

    /// The action a `keybinds.toml` name refers to, or `None` if no action
    /// answers to it - which is how a typo'd override gets reported.
    pub fn from_name(name: &str) -> Option<Self> {
        ACTIONS
            .iter()
            .find(|def| def.name == name)
            .map(|def| def.action)
    }

    /// Every action a given layer performs.
    pub fn in_category(
        category: Category,
    ) -> impl Iterator<Item = Action> + 'static {
        ACTIONS
            .iter()
            .filter(move |def| def.category == category)
            .map(|def| def.action)
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_action_has_exactly_one_row() {
        assert_eq!(Action::ALL.len(), ACTIONS.len());
        for action in Action::ALL {
            assert_eq!(
                action.def().action,
                *action,
                "{action}'s row is out of order, so `def` indexes the wrong one"
            );
        }
    }

    #[test]
    fn names_are_unique() {
        // Names are what `keybinds.toml` binds to, so a duplicate would make
        // one of the two actions unreachable from a config file.
        let mut seen = HashSet::new();
        for action in Action::ALL {
            assert!(seen.insert(action.name()), "duplicate name {action}");
        }
    }

    #[test]
    fn names_are_snake_case_and_labels_are_not_empty() {
        for action in Action::ALL {
            let name = action.name();
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || c == '_'),
                "{name} should be snake_case"
            );
            assert!(!action.label().is_empty(), "{name} has no label");
        }
    }

    #[test]
    fn a_name_round_trips_to_its_action() {
        for action in Action::ALL {
            assert_eq!(Action::from_name(action.name()), Some(*action));
        }
        assert_eq!(Action::from_name("no_such_action"), None);
    }

    #[test]
    fn editor_actions_want_the_editor_focused() {
        // Not a law of nature, but it is true of every action today, and an
        // editor action that fires with the editor unfocused would be a
        // surprise worth writing down deliberately.
        for action in Action::in_category(Category::Editor) {
            assert_eq!(action.context(), Context::EditorFocused, "{action}");
        }
    }
}
