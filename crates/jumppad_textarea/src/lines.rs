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
        // A Line selection reports anchor == cursor and never spans lines -
        // the real bounds live in the kind.
        SelectionKind::Line => (selection.anchor.0, selection.anchor.0),
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
#[path = "lines_tests.rs"]
mod tests;
