//! Where one word ends and the next begins: pure scanning over a line's
//! text, glued to the document by `TextArea`.
//!
//! cosmic-text answers this question itself, with Unicode word segmentation,
//! and offers no way to be told a different answer - so the word motions and
//! the double click are resolved here and reach the buffer as a cursor
//! position rather than as a word action.

use std::ops::Range;

/// VS Code's `editor.wordSeparators`, character for character. Shipped as
/// the default because a user arriving from another editor already knows
/// what it does, and matching it exactly means a `settings.json` line can be
/// pasted straight across.
///
/// Matches `[words] separators`' own default in `jumppad_config`. Only
/// reached by a widget nobody has told otherwise.
pub const DEFAULT_SEPARATORS: &str = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?";

/// The characters that end a word - this crate's own mirror of the config
/// setting, so it doesn't depend on `jumppad_config`.
///
/// Whitespace is never part of a word whatever this holds, which is why a
/// separator list never has to name it. A character in neither group is a
/// word character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordSeparators(String);

impl Default for WordSeparators {
    fn default() -> Self {
        Self::new(DEFAULT_SEPARATORS)
    }
}

/// What a character is worth to a word boundary, in the order a double click
/// prefers them: a word beats punctuation, and punctuation beats a space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    Whitespace,
    Separator,
    Word,
}

impl WordSeparators {
    /// The whole set, replacing the default rather than adding to it - the
    /// same rule `[[languages]]` follows, and the one VS Code's setting
    /// follows.
    pub fn new(separators: &str) -> Self {
        Self(separators.to_string())
    }

    /// The start of the word before `column` - where one press of word-left
    /// lands. The start of the line when there is no word behind it.
    pub fn start_before(&self, line: &str, column: usize) -> usize {
        let column = boundary(line, column);
        // The gap first: a caret just past a word steps back over the
        // whitespace between them to reach it.
        let column = self.run_start(line, column, Class::Whitespace);
        match self.class_before(line, column) {
            Some(class) => self.run_start(line, column, class),
            None => 0,
        }
    }

    /// The end of the word after `column` - where one press of word-right
    /// lands. The end of the line when there is no word ahead of it.
    pub fn end_after(&self, line: &str, column: usize) -> usize {
        let column = boundary(line, column);
        let column = self.run_end(line, column, Class::Whitespace);
        match self.class_at(line, column) {
            Some(class) => self.run_end(line, column, class),
            None => line.len(),
        }
    }

    /// The run of like characters `column` falls in - what a double click
    /// takes.
    ///
    /// The caret sits *between* two characters, and the higher-ranked of the
    /// two decides which run that is: clicking the `=` in `a = b` takes the
    /// `=` from either side of it, and only a caret with whitespace on both
    /// sides selects whitespace. A tie goes to the character on the left, so
    /// a caret against the end of a word takes that word rather than what
    /// follows it.
    pub fn run_around(&self, line: &str, column: usize) -> Range<usize> {
        let column = boundary(line, column);
        let class = match (
            self.class_before(line, column),
            self.class_at(line, column),
        ) {
            (Some(before), Some(at)) => before.max(at),
            (Some(class), None) | (None, Some(class)) => class,
            // An empty line: nothing on either side to take.
            (None, None) => return column..column,
        };
        self.run_start(line, column, class)..self.run_end(line, column, class)
    }

    fn class_of(&self, character: char) -> Class {
        if character.is_whitespace() {
            Class::Whitespace
        } else if self.0.contains(character) {
            Class::Separator
        } else {
            Class::Word
        }
    }

    fn class_before(&self, line: &str, column: usize) -> Option<Class> {
        line[..column].chars().next_back().map(|c| self.class_of(c))
    }

    fn class_at(&self, line: &str, column: usize) -> Option<Class> {
        line[column..].chars().next().map(|c| self.class_of(c))
    }

    /// Where the run of `class` characters ending at `column` starts.
    /// `column` itself when the character before it is of another class.
    fn run_start(&self, line: &str, column: usize, class: Class) -> usize {
        let mut start = column;
        for (offset, character) in line[..column].char_indices().rev() {
            if self.class_of(character) != class {
                break;
            }
            start = offset;
        }
        start
    }

    /// Where the run of `class` characters starting at `column` ends.
    fn run_end(&self, line: &str, column: usize, class: Class) -> usize {
        let mut end = column;
        for (offset, character) in line[column..].char_indices() {
            if self.class_of(character) != class {
                break;
            }
            end = column + offset + character.len_utf8();
        }
        end
    }
}

/// A byte column pulled inside the line and back onto a character boundary,
/// so nothing here can slice a `str` in half. The same guard
/// `clamp_position` puts on a saved cursor, applied where the scanning
/// happens rather than trusted from every caller.
fn boundary(line: &str, column: usize) -> usize {
    let mut column = column.min(line.len());
    while !line.is_char_boundary(column) {
        column -= 1;
    }
    column
}

#[cfg(test)]
#[path = "word_tests.rs"]
mod tests;
