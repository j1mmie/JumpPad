//! What a line transform hands back, and how a caret rides across it. Shared
//! by the transforms in `comment` and `indent`, so a caret lands the same way
//! whichever one moved the text under it.

/// One edit on a covered line. `column` is in pre-edit byte coordinates; a
/// positive `delta` inserts there, a negative one removes
/// `[column, column + |delta|)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineEdit {
    pub column: usize,
    pub delta: isize,
}

/// The transformed lines plus each line's edits, ascending by column - empty
/// for an untouched line. Both parallel the input.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EditedLines {
    pub lines: Vec<String>,
    pub edits: Vec<Vec<LineEdit>>,
}

impl EditedLines {
    pub fn push(&mut self, line: String, edits: Vec<LineEdit>) {
        self.lines.push(line);
        self.edits.push(edits);
    }

    /// A line the transform passed over: it goes back as it came, and no
    /// caret standing on it moves.
    pub fn push_unchanged(&mut self, line: &str) {
        self.push(line.to_string(), Vec::new());
    }
}

/// Shifts a saved `(line, byte column)` across a line's edits: inserts at
/// or before it push it right, removals pull it left, and a caret inside a
/// removed span pins to where the span started.
pub fn shift_position(
    pos: (usize, usize),
    first_line: usize,
    edits: &[Vec<LineEdit>],
) -> (usize, usize) {
    let (line, column) = pos;
    let Some(line_edits) =
        line.checked_sub(first_line).and_then(|i| edits.get(i))
    else {
        return pos;
    };
    let mut shifted = column as isize;
    for edit in line_edits {
        if edit.delta >= 0 {
            if column >= edit.column {
                shifted += edit.delta;
            }
        } else {
            let removed_end = edit.column + edit.delta.unsigned_abs();
            if column >= removed_end {
                shifted += edit.delta;
            } else if column > edit.column {
                shifted -= (column - edit.column) as isize;
            }
        }
    }
    (line, shifted.max(0) as usize)
}

#[cfg(test)]
#[path = "line_edit_tests.rs"]
mod tests;
