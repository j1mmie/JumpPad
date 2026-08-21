//! What one press of Tab is worth: pure arithmetic over a line's text, glued
//! to the document by `TextArea::indent`.

use std::ops::RangeInclusive;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spaces(width: u16) -> Indentation {
        Indentation::new(IndentationStyle::Spaces, width)
    }

    fn tabs(width: u16) -> Indentation {
        Indentation::new(IndentationStyle::Tabs, width)
    }

    #[test]
    fn the_tabs_style_inserts_one_character_whatever_the_width() {
        for width in [1, 2, 4, 8] {
            assert_eq!(tabs(width).text_at(0), "\t");
            assert_eq!(tabs(width).text_at(7), "\t");
        }
    }

    #[test]
    fn spaces_reach_the_next_stop() {
        // The user-facing spec example: width 8, caret at column 12, the next
        // stop at 16, so four spaces.
        assert_eq!(spaces(8).text_at(12), "    ");
    }

    #[test]
    fn a_caret_already_on_a_stop_gets_a_whole_width() {
        assert_eq!(spaces(4).text_at(0), "    ");
        assert_eq!(spaces(4).text_at(4), "    ");
        assert_eq!(spaces(8).text_at(16), " ".repeat(8));
    }

    #[test]
    fn spaces_fill_the_remainder_of_the_stop_the_caret_is_inside() {
        assert_eq!(spaces(4).text_at(1), "   ");
        assert_eq!(spaces(4).text_at(2), "  ");
        assert_eq!(spaces(4).text_at(3), " ");
    }

    #[test]
    fn a_width_out_of_range_is_pulled_back_into_it() {
        assert_eq!(Indentation::new(IndentationStyle::Tabs, 0).width(), 1);
        assert_eq!(Indentation::new(IndentationStyle::Tabs, 999).width(), 16);
        // And the arithmetic still terminates, rather than dividing by zero.
        assert_eq!(spaces(0).text_at(3), " ");
    }

    #[test]
    fn the_default_is_tabs_at_four() {
        let indentation = Indentation::default();
        assert_eq!(indentation.style(), IndentationStyle::Tabs);
        assert_eq!(indentation.width(), DEFAULT_WIDTH);
    }

    #[test]
    fn a_column_with_no_tabs_before_it_counts_characters() {
        assert_eq!(spaces(4).visual_column("let x = 1;", 7), 7);
        assert_eq!(spaces(4).visual_column("    let x", 4), 4);
    }

    #[test]
    fn a_tab_before_the_column_advances_to_its_stop() {
        // One tab, then two characters: 4 + 2.
        assert_eq!(spaces(4).visual_column("\tab", 3), 6);
        // Two tabs cover two stops, not one plus a character.
        assert_eq!(spaces(4).visual_column("\t\t", 2), 8);
    }

    #[test]
    fn a_tab_part_way_through_a_stop_still_lands_on_the_next_one() {
        // "ab" leaves the column at 2; the tab covers the remaining 2.
        assert_eq!(spaces(4).visual_column("ab\tc", 4), 5);
        // A tab from a column already on a stop covers a whole width.
        assert_eq!(spaces(4).visual_column("abcd\t", 5), 8);
    }

    #[test]
    fn the_column_is_measured_at_the_width_in_hand() {
        assert_eq!(spaces(2).visual_column("\tx", 2), 3);
        assert_eq!(spaces(8).visual_column("\tx", 2), 9);
    }

    #[test]
    fn a_multibyte_character_counts_one_column_not_its_bytes() {
        // "é" is two bytes, so the byte column past it is 2.
        assert_eq!(spaces(4).visual_column("é", 2), 1);
        assert_eq!(spaces(4).visual_column("ééx", 5), 3);
    }

    #[test]
    fn a_byte_column_past_the_line_measures_the_whole_line() {
        // `clamp_position` keeps this from happening, but the arithmetic
        // should not depend on that - and must not slice a `str` to do it.
        assert_eq!(spaces(4).visual_column("ab", 99), 2);
        assert_eq!(spaces(4).visual_column("", 99), 0);
    }

    #[test]
    fn an_indent_from_a_tabbed_line_reaches_the_next_stop() {
        // The two halves together: a caret after one tab on a width-4 line
        // is at column 4, so the indent is a full four spaces.
        let indentation = spaces(4);
        let column = indentation.visual_column("\t", 1);
        assert_eq!(indentation.text_at(column), "    ");
    }
}
