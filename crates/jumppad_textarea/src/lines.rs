//! The line-wise commands - delete, move, duplicate - as pure index
//! arithmetic over line texts, glued to the document by `TextArea`.

use std::ops::Range;

use editor_core::{SavedSelection, SelectionKind};

/// What a line command does to the document: the lines that take the place
/// of `replaced`, and where that leaves the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSplice {
    pub replaced: Range<usize>,
    pub lines: Vec<String>,
    pub caret: Caret,
}

/// Where a line command leaves the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caret {
    /// Every covered row moves this far, and a selection rides along.
    Shifted(isize),
    /// Collapsed onto this row, keeping the column it had.
    Collapsed(usize),
}

/// The inclusive `(first, last)` line range the command covers.
pub fn covered_lines(
    cursor: (usize, usize),
    selection: Option<SavedSelection>,
) -> (usize, usize) {
    let Some(selection) = selection else {
        return (cursor.0, cursor.0);
    };
    match selection.kind {
        // Word and Line selections report anchor == cursor and never span
        // lines - the real bounds live in the kind.
        SelectionKind::Word | SelectionKind::Line => {
            (selection.anchor.0, selection.anchor.0)
        }
        SelectionKind::Range => {
            let (top, bottom) = if selection.anchor <= cursor {
                (selection.anchor, cursor)
            } else {
                (cursor, selection.anchor)
            };
            // A selection whose bottom edge sits at column 0 merely starts
            // that line - it shouldn't be covered.
            if bottom.0 > top.0 && bottom.1 == 0 {
                (top.0, bottom.0 - 1)
            } else {
                (top.0, bottom.0)
            }
        }
    }
}

/// The covered lines removed; whatever slides up into their place takes the
/// caret.
pub fn delete((first, last): (usize, usize)) -> LineSplice {
    LineSplice {
        replaced: first..last + 1,
        lines: Vec::new(),
        caret: Caret::Collapsed(first),
    }
}

/// The covered block one row higher, with `above` pushed underneath it. The
/// caller only has an `above` when there is a row to move into.
pub fn move_up(
    (first, last): (usize, usize),
    above: String,
    block: Vec<String>,
) -> LineSplice {
    let mut lines = block;
    lines.push(above);
    LineSplice {
        replaced: first - 1..last + 1,
        lines,
        caret: Caret::Shifted(-1),
    }
}

/// Mirror of [`move_up`]: the covered block one row lower, `below` pulled
/// above it.
pub fn move_down(
    (first, last): (usize, usize),
    below: String,
    block: Vec<String>,
) -> LineSplice {
    let mut lines = vec![below];
    lines.extend(block);
    LineSplice {
        replaced: first..last + 2,
        lines,
        caret: Caret::Shifted(1),
    }
}

/// The covered block duplicated in place. Both directions write the same
/// document; only which copy keeps the caret differs, so repeating either
/// grows the block away from where it started.
pub fn copy(
    (first, last): (usize, usize),
    block: Vec<String>,
    downward: bool,
) -> LineSplice {
    let height = block.len();
    let mut lines = block.clone();
    lines.extend(block);
    let shift = if downward { height as isize } else { 0 };
    LineSplice {
        replaced: first..last + 1,
        lines,
        caret: Caret::Shifted(shift),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    fn range(anchor: (usize, usize)) -> Option<SavedSelection> {
        Some(SavedSelection {
            anchor,
            kind: SelectionKind::Range,
        })
    }

    #[test]
    fn covered_lines_without_a_selection_is_the_cursor_line() {
        assert_eq!(covered_lines((3, 7), None), (3, 3));
    }

    #[test]
    fn covered_lines_orders_a_reversed_range() {
        assert_eq!(covered_lines((1, 2), range((4, 0))), (1, 3));
        assert_eq!(covered_lines((4, 5), range((1, 2))), (1, 4));
    }

    #[test]
    fn a_bottom_edge_at_column_zero_excludes_that_line() {
        // Shift+Down from line 1 stops at (2, 0): line 2 is merely started.
        assert_eq!(covered_lines((2, 0), range((1, 0))), (1, 1));
        // ...but a single-line selection ending at column 0 keeps its line.
        assert_eq!(covered_lines((1, 0), range((1, 4))), (1, 1));
    }

    #[test]
    fn word_and_line_selections_cover_the_anchor_line_only() {
        for kind in [SelectionKind::Word, SelectionKind::Line] {
            let selection = Some(SavedSelection {
                anchor: (2, 5),
                kind,
            });
            assert_eq!(covered_lines((2, 5), selection), (2, 2));
        }
    }

    #[test]
    fn delete_replaces_the_covered_range_with_nothing() {
        let splice = delete((1, 2));
        assert_eq!(splice.replaced, 1..3);
        assert!(splice.lines.is_empty());
        assert_eq!(splice.caret, Caret::Collapsed(1));
    }

    #[test]
    fn move_up_puts_the_line_above_under_the_block() {
        let splice = move_up((1, 2), "aaa".to_string(), block(&["bbb", "ccc"]));
        assert_eq!(splice.replaced, 0..3);
        assert_eq!(splice.lines, block(&["bbb", "ccc", "aaa"]));
        assert_eq!(splice.caret, Caret::Shifted(-1));
    }

    #[test]
    fn move_down_pulls_the_line_below_above_the_block() {
        let splice =
            move_down((0, 1), "ccc".to_string(), block(&["aaa", "bbb"]));
        assert_eq!(splice.replaced, 0..3);
        assert_eq!(splice.lines, block(&["ccc", "aaa", "bbb"]));
        assert_eq!(splice.caret, Caret::Shifted(1));
    }

    #[test]
    fn copy_writes_the_same_lines_in_both_directions() {
        let down = copy((0, 1), block(&["aaa", "bbb"]), true);
        let up = copy((0, 1), block(&["aaa", "bbb"]), false);
        assert_eq!(down.lines, block(&["aaa", "bbb", "aaa", "bbb"]));
        assert_eq!(down.lines, up.lines);
        assert_eq!(down.replaced, up.replaced);
        assert_eq!(down.replaced, 0..2);
    }

    #[test]
    fn copy_down_lands_the_caret_a_block_below_and_copy_up_stays_put() {
        assert_eq!(
            copy((0, 1), block(&["aaa", "bbb"]), true).caret,
            Caret::Shifted(2)
        );
        assert_eq!(
            copy((0, 1), block(&["aaa", "bbb"]), false).caret,
            Caret::Shifted(0)
        );
    }
}
