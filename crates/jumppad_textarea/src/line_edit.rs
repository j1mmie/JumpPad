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
mod tests {
    use super::*;

    #[test]
    fn shift_position_moves_only_columns_at_or_after_the_edit() {
        let edits = [vec![LineEdit {
            column: 4,
            delta: 3,
        }]];
        assert_eq!(
            shift_position((0, 2), 0, &edits),
            (0, 2),
            "before the edit"
        );
        assert_eq!(shift_position((0, 4), 0, &edits), (0, 7), "at the edit");
        assert_eq!(
            shift_position((0, 9), 0, &edits),
            (0, 12),
            "after the edit"
        );
        assert_eq!(shift_position((5, 9), 0, &edits), (5, 9), "uncovered line");
    }

    #[test]
    fn shift_position_clamps_a_caret_inside_a_removed_prefix() {
        // Caret sat on the second slash of a removed "// " (delta -3).
        let edits = [vec![LineEdit {
            column: 4,
            delta: -3,
        }]];
        assert_eq!(shift_position((0, 5), 0, &edits), (0, 4));
    }

    #[test]
    fn shift_position_compounds_two_removals_on_one_line() {
        // "    <!--foo-->" uncommenting: left [4,8), right [11,14).
        let edits = [vec![
            LineEdit {
                column: 4,
                delta: -4,
            },
            LineEdit {
                column: 11,
                delta: -3,
            },
        ]];
        assert_eq!(shift_position((0, 2), 0, &edits), (0, 2), "before both");
        assert_eq!(
            shift_position((0, 4), 0, &edits),
            (0, 4),
            "at left span start"
        );
        assert_eq!(
            shift_position((0, 6), 0, &edits),
            (0, 4),
            "inside left span"
        );
        assert_eq!(
            shift_position((0, 9), 0, &edits),
            (0, 5),
            "between the spans"
        );
        assert_eq!(
            shift_position((0, 12), 0, &edits),
            (0, 7),
            "inside right span"
        );
        assert_eq!(shift_position((0, 14), 0, &edits), (0, 7), "at old EOL");
    }

    #[test]
    fn an_unchanged_line_goes_back_as_it_came() {
        let mut edited = EditedLines::default();
        edited.push_unchanged("  keep me");
        assert_eq!(edited.lines, vec!["  keep me"]);
        assert_eq!(shift_position((0, 5), 0, &edited.edits), (0, 5));
    }
}
