//! What one press of Tab is worth: pure arithmetic over a line's text, glued
//! to the document by `TextArea::indent`.

use std::ops::RangeInclusive;

use crate::line_edit::{EditedLines, LineEdit};

/// Whether an indent is one tab character or a run of spaces - this crate's
/// own mirror of the config type, so it doesn't depend on `jumppad_config`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndentationStyle {
    #[default]
    Tabs,
    Spaces,
}

/// The widths a tab stop is allowed to sit at. The ceiling is only there to
/// keep a typo'd config from indenting a line off the screen; the floor is
/// one because zero has no stops at all - and cosmic-text ignores a zero
/// outright, which would leave the drawn width disagreeing with the
/// inserted one.
pub const WIDTH_RANGE: RangeInclusive<u16> = 1..=16;

/// The width JumpPad ships with, matching `[indentation] width`'s own
/// default. Only reached by a widget nobody has told otherwise.
pub const DEFAULT_WIDTH: u16 = 4;

/// What the Tab key inserts, and how wide a tab character is drawn - one
/// setting, because a document whose tabs draw at one width and indent at
/// another is the thing this exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Indentation {
    style: IndentationStyle,
    width: u16,
}

impl Default for Indentation {
    fn default() -> Self {
        Self::new(IndentationStyle::Tabs, DEFAULT_WIDTH)
    }
}

impl Indentation {
    /// The only way to build one, so a width outside `WIDTH_RANGE` can't
    /// reach the arithmetic below, or the buffer that draws the tabs.
    pub fn new(style: IndentationStyle, width: u16) -> Self {
        Self {
            style,
            width: width.clamp(*WIDTH_RANGE.start(), *WIDTH_RANGE.end()),
        }
    }

    pub fn style(self) -> IndentationStyle {
        self.style
    }

    /// Columns between one tab stop and the next, already in range.
    pub fn width(self) -> u16 {
        self.width
    }

    /// Where a byte column falls on the drawn line: a tab jumps to the next
    /// stop, anything else takes one column.
    ///
    /// Exact in the monospace face the editor defaults to. A proportional
    /// family has no columns to count, and the buffer puts its stops at
    /// multiples of a space's advance instead - near enough that an indent
    /// still lands on a stop, but not a promise this can keep.
    pub fn visual_column(self, line: &str, byte_column: usize) -> usize {
        let width = usize::from(self.width);
        line.char_indices()
            .take_while(|(offset, _)| *offset < byte_column)
            .fold(0, |column, (_, character)| match character {
                '\t' => (column / width + 1) * width,
                _ => column + 1,
            })
    }

    /// One indent, typed at `visual_column`: a tab character, or the spaces
    /// that reach the next stop - a full width of them from a caret already
    /// standing on one.
    pub fn text_at(self, visual_column: usize) -> String {
        match self.style {
            IndentationStyle::Tabs => "\t".to_string(),
            IndentationStyle::Spaces => {
                let width = usize::from(self.width);
                " ".repeat(width - visual_column % width)
            }
        }
    }

    /// How wide `line`'s leading whitespace draws. Only spaces and tabs
    /// count as indentation; anything else ends it, so a line led by an
    /// exotic space is left to the transforms below as content.
    fn indent_columns(self, line: &str) -> usize {
        let width = usize::from(self.width);
        let mut columns = 0;
        for character in line.chars() {
            columns = match character {
                '\t' => (columns / width + 1) * width,
                ' ' => columns + 1,
                _ => break,
            };
        }
        columns
    }

    /// One more indent on the front of every line, each reaching the next
    /// stop from the indentation it already has. Blank lines are left alone -
    /// indenting one only buys it trailing whitespace - so a block of
    /// nothing but blanks is `None`, with no edit to make.
    pub fn indent_lines(self, lines: &[&str]) -> Option<EditedLines> {
        let mut indented = EditedLines::default();
        let mut changed = false;
        for line in lines {
            if line.trim_start().is_empty() {
                indented.push_unchanged(line);
                continue;
            }
            changed = true;
            let text = self.text_at(self.indent_columns(line));
            indented.push(
                format!("{text}{line}"),
                vec![LineEdit {
                    column: 0,
                    delta: text.len() as isize,
                }],
            );
        }
        changed.then_some(indented)
    }

    /// The mirror of [`Self::indent_lines`]: every line's indentation back to
    /// the previous stop. `None` when no line has any indentation left to
    /// give up.
    pub fn outdent_lines(self, lines: &[&str]) -> Option<EditedLines> {
        let mut outdented = EditedLines::default();
        let mut changed = false;
        for line in lines {
            let removed = self.outdent_len(line);
            if removed == 0 {
                outdented.push_unchanged(line);
                continue;
            }
            changed = true;
            outdented.push(
                line[removed..].to_string(),
                vec![LineEdit {
                    column: 0,
                    delta: -(removed as isize),
                }],
            );
        }
        changed.then_some(outdented)
    }

    /// The leading bytes one outdent takes off `line`: enough whitespace to
    /// fall back to the previous stop, and never a character of content.
    ///
    /// A tab that overshoots the stop still goes, but only as the first
    /// character removed - otherwise a line indented `" \t"` would have
    /// nothing an outdent could take, and Shift+Tab would sit there doing
    /// nothing however often it was pressed.
    fn outdent_len(self, line: &str) -> usize {
        // Blank lines skipped for the same reason `indent_lines` skips them:
        // the two have to undo each other.
        if line.trim_start().is_empty() {
            return 0;
        }
        let width = usize::from(self.width);
        let wanted = match self.indent_columns(line) % width {
            0 => width,
            remainder => remainder,
        };

        let mut removed = 0;
        let mut bytes = 0;
        for (offset, character) in line.char_indices() {
            let reached = match character {
                '\t' => (removed / width + 1) * width,
                ' ' => removed + 1,
                _ => break,
            };
            if reached > wanted && bytes > 0 {
                break;
            }
            removed = reached;
            bytes = offset + character.len_utf8();
            if removed >= wanted {
                break;
            }
        }
        bytes
    }
}

#[cfg(test)]
#[path = "indent_tests.rs"]
mod tests;
