//! The toggle-comment transformation: pure functions over line texts, so
//! the whole behavior tests without a window. `TextArea::toggle_comment`
//! glues these to the document.

use editor_core::{SavedSelection, SelectionKind};

/// One covered line's edit, for shifting a caret column on that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineEdit {
    /// Byte column the edit starts at: the insertion point, or the first
    /// byte of the removed prefix.
    pub column: usize,
    /// Signed byte delta: `+prefix.len()` on comment, negative on
    /// uncomment, zero for an untouched (blank) line.
    pub delta: isize,
}

/// The transformed lines plus one [`LineEdit`] per input line, in order.
pub struct ToggledLines {
    pub lines: Vec<String>,
    pub edits: Vec<LineEdit>,
}

/// Comments or uncomments `lines` (the covered lines' texts, endings
/// excluded) with `prefix` (e.g. `"// "`). Uncomments only when every
/// non-blank line is already commented; otherwise comments every non-blank
/// line, at the leftmost non-whitespace column any of them starts at.
/// `None` means nothing to do: every line is blank, or the prefix trims to
/// empty (a whitespace-only configured prefix would match everything).
pub fn toggle_comment(lines: &[&str], prefix: &str) -> Option<ToggledLines> {
    let token = prefix.trim_end();
    if token.is_empty() {
        return None;
    }
    let blank = |line: &str| line.trim_start().is_empty();
    if lines.iter().all(|line| blank(line)) {
        return None;
    }

    let all_commented = lines
        .iter()
        .filter(|line| !blank(line))
        .all(|line| line.trim_start().starts_with(token));

    let toggled = if all_commented {
        uncomment(lines, prefix, token, blank)
    } else {
        comment(lines, prefix, blank)
    };
    Some(toggled)
}

fn leading_whitespace_len(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn comment(lines: &[&str], prefix: &str, blank: impl Fn(&str) -> bool) -> ToggledLines {
    let insert_col = lines
        .iter()
        .filter(|line| !blank(line))
        .map(|line| leading_whitespace_len(line))
        .min()
        .unwrap_or(0);

    let mut toggled = ToggledLines { lines: Vec::new(), edits: Vec::new() };
    for line in lines {
        if blank(line) {
            toggled.lines.push(line.to_string());
            toggled.edits.push(LineEdit { column: 0, delta: 0 });
            continue;
        }
        // The min indent was measured on other lines' whitespace, so back
        // it down to a char boundary of this one (multi-byte whitespace
        // like NBSP would otherwise split a char).
        let mut column = insert_col;
        while !line.is_char_boundary(column) {
            column -= 1;
        }
        let mut commented = String::with_capacity(line.len() + prefix.len());
        commented.push_str(&line[..column]);
        commented.push_str(prefix);
        commented.push_str(&line[column..]);
        toggled.lines.push(commented);
        toggled.edits.push(LineEdit { column, delta: prefix.len() as isize });
    }
    toggled
}

fn uncomment(
    lines: &[&str],
    prefix: &str,
    token: &str,
    blank: impl Fn(&str) -> bool,
) -> ToggledLines {
    let mut toggled = ToggledLines { lines: Vec::new(), edits: Vec::new() };
    for line in lines {
        if blank(line) {
            toggled.lines.push(line.to_string());
            toggled.edits.push(LineEdit { column: 0, delta: 0 });
            continue;
        }
        let column = leading_whitespace_len(line);
        let after_token = &line[column + token.len()..];
        // A prefix configured with a trailing space eats one back, so
        // `// foo` and `//foo` both uncomment to the same thing.
        let removed = if prefix.ends_with(' ') && after_token.starts_with(' ') {
            token.len() + 1
        } else {
            token.len()
        };
        let mut uncommented = String::with_capacity(line.len() - removed);
        uncommented.push_str(&line[..column]);
        uncommented.push_str(&line[column + removed..]);
        toggled.lines.push(uncommented);
        toggled.edits.push(LineEdit { column, delta: -(removed as isize) });
    }
    toggled
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
        SelectionKind::Word | SelectionKind::Line => (selection.anchor.0, selection.anchor.0),
        SelectionKind::Range => {
            let (top, bottom) = if selection.anchor <= cursor {
                (selection.anchor, cursor)
            } else {
                (cursor, selection.anchor)
            };
            // A selection whose bottom edge sits at column 0 merely starts
            // that line - it shouldn't get commented.
            if bottom.0 > top.0 && bottom.1 == 0 {
                (top.0, bottom.0 - 1)
            } else {
                (top.0, bottom.0)
            }
        }
    }
}

/// Shifts a saved `(line, byte column)` across the toggle's per-line edits.
/// Columns before an edit stay put; columns at or after it move by the
/// delta, clamped so a caret inside a removed prefix lands where the prefix
/// was. Upper clamping is the caller's restore path's job.
pub fn shift_position(
    pos: (usize, usize),
    first_line: usize,
    edits: &[LineEdit],
) -> (usize, usize) {
    let (line, column) = pos;
    let Some(edit) = line.checked_sub(first_line).and_then(|i| edits.get(i)) else {
        return pos;
    };
    if edit.delta == 0 || column < edit.column {
        return pos;
    }
    let shifted = column
        .saturating_add_signed(edit.delta)
        .max(edit.column);
    (line, shifted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toggle(lines: &[&str]) -> ToggledLines {
        toggle_comment(lines, "// ").unwrap()
    }

    #[test]
    fn comments_a_single_line() {
        let toggled = toggle(&["fn main() {}"]);
        assert_eq!(toggled.lines, vec!["// fn main() {}"]);
        assert_eq!(toggled.edits, vec![LineEdit { column: 0, delta: 3 }]);
    }

    #[test]
    fn toggling_twice_round_trips() {
        let toggled = toggle(&["    let x = 1;"]);
        assert_eq!(toggled.lines, vec!["    // let x = 1;"]);
        let refs: Vec<&str> = toggled.lines.iter().map(String::as_str).collect();
        let back = toggle(&refs);
        assert_eq!(back.lines, vec!["    let x = 1;"]);
        assert_eq!(back.edits, vec![LineEdit { column: 4, delta: -3 }]);
    }

    #[test]
    fn uncomments_a_prefix_without_the_space() {
        let toggled = toggle(&["//fn"]);
        assert_eq!(toggled.lines, vec!["fn"]);
        assert_eq!(toggled.edits, vec![LineEdit { column: 0, delta: -2 }]);
    }

    #[test]
    fn uncommenting_strips_exactly_one_space() {
        let toggled = toggle(&["//  x"]);
        assert_eq!(toggled.lines, vec![" x"]);
    }

    #[test]
    fn inserts_uniformly_at_the_minimum_indent() {
        // The user-facing spec example: deeper lines get the prefix at the
        // shallowest line's column, keeping the block aligned.
        let toggled = toggle(&["            if (a > b) {", "                return false"]);
        assert_eq!(
            toggled.lines,
            vec!["            // if (a > b) {", "            //     return false"]
        );
    }

    #[test]
    fn blank_lines_are_untouched_and_do_not_drag_the_indent_to_zero() {
        let toggled = toggle(&["    a", "", "    b"]);
        assert_eq!(toggled.lines, vec!["    // a", "", "    // b"]);
        assert_eq!(toggled.edits[1], LineEdit { column: 0, delta: 0 });
    }

    #[test]
    fn mixed_coverage_comments_everything() {
        // The already-commented line gains a second prefix; toggling again
        // returns to this exact mixed state.
        let toggled = toggle(&["// a", "b"]);
        assert_eq!(toggled.lines, vec!["// // a", "// b"]);
    }

    #[test]
    fn all_blank_coverage_is_a_no_op() {
        assert!(toggle_comment(&["", "   "], "// ").is_none());
    }

    #[test]
    fn a_whitespace_only_prefix_is_a_no_op() {
        assert!(toggle_comment(&["text"], "").is_none());
        assert!(toggle_comment(&["text"], "   ").is_none());
    }

    #[test]
    fn tab_indentation_measures_in_bytes() {
        let toggled = toggle(&["\tx", "\t\ty"]);
        assert_eq!(toggled.lines, vec!["\t// x", "\t// \ty"]);
    }

    #[test]
    fn multibyte_whitespace_backs_down_to_a_char_boundary() {
        // NBSP is two bytes; a min indent measured on the space-indented
        // line would split it. No panic, prefix lands before the NBSP.
        let toggled = toggle(&["\u{a0}x", " y"]);
        assert_eq!(toggled.lines, vec!["// \u{a0}x", " // y"]);
    }

    fn range(anchor: (usize, usize)) -> Option<SavedSelection> {
        Some(SavedSelection { anchor, kind: SelectionKind::Range })
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
            let selection = Some(SavedSelection { anchor: (2, 5), kind });
            assert_eq!(covered_lines((2, 5), selection), (2, 2));
        }
    }

    #[test]
    fn shift_position_moves_only_columns_at_or_after_the_edit() {
        let edits = [LineEdit { column: 4, delta: 3 }];
        assert_eq!(shift_position((0, 2), 0, &edits), (0, 2), "before the edit");
        assert_eq!(shift_position((0, 4), 0, &edits), (0, 7), "at the edit");
        assert_eq!(shift_position((0, 9), 0, &edits), (0, 12), "after the edit");
        assert_eq!(shift_position((5, 9), 0, &edits), (5, 9), "uncovered line");
    }

    #[test]
    fn shift_position_clamps_a_caret_inside_a_removed_prefix() {
        // Caret sat on the second slash of a removed "// " (delta -3).
        let edits = [LineEdit { column: 4, delta: -3 }];
        assert_eq!(shift_position((0, 5), 0, &edits), (0, 4));
    }
}
