use serde::{Deserialize, Serialize};

/// What the Tab key inserts, and how wide a tab character is drawn. `width`
/// means columns in both modes: the stops a tab advances to, and the stops
/// the spaces reach. Clamped where applied, not here, so this crate doesn't
/// need to know what the editor considers a usable range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndentationConfig {
    pub style: IndentationStyle,
    pub width: u16,
}

impl Default for IndentationConfig {
    fn default() -> Self {
        Self {
            style: IndentationStyle::Tabs,
            width: 4,
        }
    }
}

/// VS Code's `editor.wordSeparators`, character for character - the list
/// JumpPad ships with, so a `settings.json` line can be pasted straight
/// across. Mirrored by `jumppad_textarea`'s `word::DEFAULT_SEPARATORS`,
/// which is what a widget nobody has told otherwise uses.
pub const DEFAULT_WORD_SEPARATORS: &str = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?";

/// What ends a word: the characters a caret moving by one word stops at,
/// and the ones a double click stops selecting at.
///
/// Whitespace always separates words, whatever this names, so a list never
/// has to spell out a space, a tab or a newline. Anything the list leaves
/// out is part of a word - drop `-` from it and `font-size` is one word
/// rather than three.
///
/// The whole list is replaced rather than added to, the way a
/// user-provided `[[languages]]` array replaces the built-in one, and the
/// way VS Code's own setting behaves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WordsConfig {
    pub separators: String,
}

impl Default for WordsConfig {
    fn default() -> Self {
        Self {
            separators: DEFAULT_WORD_SEPARATORS.to_string(),
        }
    }
}

/// Whether an indent is one tab character or a run of spaces. A file's
/// existing tabs are drawn at `width` under either, since the setting
/// describes the document as much as the keystroke.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum IndentationStyle {
    /// One tab character, however many columns that covers.
    #[default]
    Tabs,
    /// However many spaces reach the next stop from where the caret is.
    Spaces,
}
