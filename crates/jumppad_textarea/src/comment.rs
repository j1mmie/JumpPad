//! The toggle-comment transformation: pure functions over line texts,
//! glued to the document by `TextArea::toggle_comment`.

use editor_core::{SavedSelection, SelectionKind};

/// A file type's comment syntax - this crate's own mirror of the config
/// type, so it doesn't depend on `jumppad_config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentStyle {
    Single(String),
    Multi { left: String, right: String },
}

/// One edit on a covered line, for shifting a caret column across it.
/// `column` is in pre-toggle byte coordinates; a positive `delta` inserts
/// there, a negative one removes `[column, column + |delta|)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineEdit {
    pub column: usize,
    pub delta: isize,
}

/// The transformed lines plus each line's edits, ascending by column -
/// empty for an untouched line. Both parallel the input.
pub struct ToggledLines {
    pub lines: Vec<String>,
    pub edits: Vec<Vec<LineEdit>>,
}

/// Uncomments `lines` when the coverage is already commented in `style`,
/// else comments it. `None` = nothing to do.
pub fn toggle_comment(lines: &[&str], style: &CommentStyle) -> Option<ToggledLines> {
    match style {
        CommentStyle::Single(prefix) => toggle_single(lines, prefix),
        CommentStyle::Multi { left, right } => toggle_multi(lines, left, right),
    }
}

fn blank(line: &str) -> bool {
    line.trim_start().is_empty()
}

fn leading_whitespace_len(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Single-line style: every non-blank line gets the prefix at the leftmost
/// non-whitespace column, or loses it when all of them already have it.
fn toggle_single(lines: &[&str], prefix: &str) -> Option<ToggledLines> {
    let token = prefix.trim_end();
    if token.is_empty() || lines.iter().all(|line| blank(line)) {
        return None;
    }

    let all_commented = lines
        .iter()
        .filter(|line| !blank(line))
        .all(|line| line.trim_start().starts_with(token));

    let toggled = if all_commented {
        single_uncomment(lines, prefix, token)
    } else {
        single_comment(lines, prefix)
    };
    Some(toggled)
}

fn single_comment(lines: &[&str], prefix: &str) -> ToggledLines {
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
            toggled.edits.push(Vec::new());
            continue;
        }
        // The min indent was measured on another line's whitespace; back it
        // down to a char boundary of this one (NBSP would otherwise split).
        let mut column = insert_col;
        while !line.is_char_boundary(column) {
            column -= 1;
        }
        let mut commented = String::with_capacity(line.len() + prefix.len());
        commented.push_str(&line[..column]);
        commented.push_str(prefix);
        commented.push_str(&line[column..]);
        toggled.lines.push(commented);
        toggled.edits.push(vec![LineEdit { column, delta: prefix.len() as isize }]);
    }
    toggled
}

fn single_uncomment(lines: &[&str], prefix: &str, token: &str) -> ToggledLines {
    let mut toggled = ToggledLines { lines: Vec::new(), edits: Vec::new() };
    for line in lines {
        if blank(line) {
            toggled.lines.push(line.to_string());
            toggled.edits.push(Vec::new());
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
        toggled.edits.push(vec![LineEdit { column, delta: -(removed as isize) }]);
    }
    toggled
}

/// Multi-line style: `left` goes after the first non-blank line's leading
/// whitespace, `right` at the last one's very end - trailing whitespace
/// included in the comment. Lines in between are untouched content.
fn toggle_multi(lines: &[&str], left: &str, right: &str) -> Option<ToggledLines> {
    let left_token = left.trim_end();
    let right_token = right.trim_start();
    if left_token.is_empty() || right_token.is_empty() {
        return None;
    }
    let first = lines.iter().position(|line| !blank(line))?;
    let last = lines.iter().rposition(|line| !blank(line))?;

    let toggled = if is_wrapped(lines[first], lines[last], first == last, left_token, right_token) {
        multi_uncomment(lines, first, last, left, right, left_token, right_token)
    } else {
        multi_comment(lines, first, last, left, right)
    };
    Some(toggled)
}

fn is_wrapped(
    first: &str,
    last: &str,
    same_line: bool,
    left_token: &str,
    right_token: &str,
) -> bool {
    if !first.trim_start().starts_with(left_token) {
        return false;
    }
    let trimmed = last.trim_end();
    if !trimmed.ends_with(right_token) {
        return false;
    }
    if !same_line {
        return true;
    }
    // On one line the two matches must not overlap: `<!-->` is not wrapped.
    let left_end = leading_whitespace_len(first) + left_token.len();
    trimmed.len() - right_token.len() >= left_end
}

fn multi_comment(lines: &[&str], first: usize, last: usize, left: &str, right: &str) -> ToggledLines {
    let mut toggled = ToggledLines { lines: Vec::new(), edits: Vec::new() };
    for (index, line) in lines.iter().enumerate() {
        let mut text = line.to_string();
        let mut edits = Vec::new();
        if index == first {
            let column = leading_whitespace_len(line);
            text.insert_str(column, left);
            edits.push(LineEdit { column, delta: left.len() as isize });
        }
        if index == last {
            // Appended after trailing whitespace, with no edit recorded: an
            // end-of-line caret must stay put, i.e. before `right`.
            text.push_str(right);
        }
        toggled.lines.push(text);
        toggled.edits.push(edits);
    }
    toggled
}

fn multi_uncomment(
    lines: &[&str],
    first: usize,
    last: usize,
    left: &str,
    right: &str,
    left_token: &str,
    right_token: &str,
) -> ToggledLines {
    let mut toggled = ToggledLines { lines: Vec::new(), edits: Vec::new() };
    for (index, line) in lines.iter().enumerate() {
        let left_span = (index == first).then(|| {
            let column = leading_whitespace_len(line);
            let after = &line[column + left_token.len()..];
            let removed = if left.ends_with(' ') && after.starts_with(' ') {
                left_token.len() + 1
            } else {
                left_token.len()
            };
            (column, removed)
        });
        let right_span = (index == last).then(|| {
            // At the trim_end boundary, so whitespace after `right` survives.
            let mut start = line.trim_end().len() - right_token.len();
            let mut removed = right_token.len();
            // The space-eat may not reach into (or past) the left removal.
            let floor = left_span.map_or(0, |(column, removed)| column + removed);
            if right.starts_with(' ') && start > floor && line.as_bytes()[start - 1] == b' ' {
                start -= 1;
                removed += 1;
            }
            (start, removed)
        });

        let mut text = line.to_string();
        if let Some((start, removed)) = right_span {
            text.replace_range(start..start + removed, "");
        }
        if let Some((column, removed)) = left_span {
            text.replace_range(column..column + removed, "");
        }

        let mut edits = Vec::new();
        if let Some((column, removed)) = left_span {
            edits.push(LineEdit { column, delta: -(removed as isize) });
        }
        if let Some((start, removed)) = right_span {
            edits.push(LineEdit { column: start, delta: -(removed as isize) });
        }
        toggled.lines.push(text);
        toggled.edits.push(edits);
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

/// Shifts a saved `(line, byte column)` across a line's edits: inserts at
/// or before it push it right, removals pull it left, and a caret inside a
/// removed span pins to where the span started.
pub fn shift_position(
    pos: (usize, usize),
    first_line: usize,
    edits: &[Vec<LineEdit>],
) -> (usize, usize) {
    let (line, column) = pos;
    let Some(line_edits) = line.checked_sub(first_line).and_then(|i| edits.get(i)) else {
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

    fn single(prefix: &str) -> CommentStyle {
        CommentStyle::Single(prefix.to_string())
    }

    fn html_multi() -> CommentStyle {
        CommentStyle::Multi { left: "<!--".to_string(), right: "-->".to_string() }
    }

    fn toggle(lines: &[&str]) -> ToggledLines {
        toggle_comment(lines, &single("// ")).unwrap()
    }

    fn toggle_html(lines: &[&str]) -> ToggledLines {
        toggle_comment(lines, &html_multi()).unwrap()
    }

    #[test]
    fn comments_a_single_line() {
        let toggled = toggle(&["fn main() {}"]);
        assert_eq!(toggled.lines, vec!["// fn main() {}"]);
        assert_eq!(toggled.edits, vec![vec![LineEdit { column: 0, delta: 3 }]]);
    }

    #[test]
    fn toggling_twice_round_trips() {
        let toggled = toggle(&["    let x = 1;"]);
        assert_eq!(toggled.lines, vec!["    // let x = 1;"]);
        let refs: Vec<&str> = toggled.lines.iter().map(String::as_str).collect();
        let back = toggle(&refs);
        assert_eq!(back.lines, vec!["    let x = 1;"]);
        assert_eq!(back.edits, vec![vec![LineEdit { column: 4, delta: -3 }]]);
    }

    #[test]
    fn uncomments_a_prefix_without_the_space() {
        let toggled = toggle(&["//fn"]);
        assert_eq!(toggled.lines, vec!["fn"]);
        assert_eq!(toggled.edits, vec![vec![LineEdit { column: 0, delta: -2 }]]);
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
        assert!(toggled.edits[1].is_empty());
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
        assert!(toggle_comment(&["", "   "], &single("// ")).is_none());
        assert!(toggle_comment(&["", "   "], &html_multi()).is_none());
    }

    #[test]
    fn a_whitespace_only_prefix_is_a_no_op() {
        assert!(toggle_comment(&["text"], &single("")).is_none());
        assert!(toggle_comment(&["text"], &single("   ")).is_none());
    }

    #[test]
    fn empty_multi_delimiters_are_a_no_op() {
        let style = CommentStyle::Multi { left: "  ".to_string(), right: "-->".to_string() };
        assert!(toggle_comment(&["text"], &style).is_none());
        let style = CommentStyle::Multi { left: "<!--".to_string(), right: String::new() };
        assert!(toggle_comment(&["text"], &style).is_none());
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

    #[test]
    fn multi_wraps_the_users_html_example_with_exact_edits() {
        // The acceptance example: selection anchor at (line 0, col 24),
        // cursor at (line 1, col 25) of these covered lines.
        let lines = [
            "        <li>Do you have an Internet connection? </li>",
            "        <li>Is anti-virus software or a firewall preventing ROBLOX from accessing the Internet?</li>     ",
        ];
        let toggled = toggle_html(&lines);
        assert_eq!(
            toggled.lines,
            vec![
                "        <!--<li>Do you have an Internet connection? </li>",
                "        <li>Is anti-virus software or a firewall preventing ROBLOX from accessing the Internet?</li>     -->",
            ]
        );
        assert_eq!(toggled.edits[0], vec![LineEdit { column: 8, delta: 4 }]);
        assert!(toggled.edits[1].is_empty(), "an EOL append moves no caret");
        // The selection's ends keep covering the same characters.
        assert_eq!(shift_position((0, 24), 0, &toggled.edits), (0, 28));
        assert_eq!(shift_position((1, 25), 0, &toggled.edits), (1, 25));

        // And the second toggle returns everything exactly.
        let refs: Vec<&str> = toggled.lines.iter().map(String::as_str).collect();
        let back = toggle_html(&refs);
        assert_eq!(back.lines.as_slice(), &lines);
        assert_eq!(back.edits[0], vec![LineEdit { column: 8, delta: -4 }]);
        let right_start = toggled.lines[1].len() - 3;
        assert_eq!(back.edits[1], vec![LineEdit { column: right_start, delta: -3 }]);
        assert_eq!(shift_position((0, 28), 0, &back.edits), (0, 24));
        assert_eq!(shift_position((1, 25), 0, &back.edits), (1, 25));
    }

    #[test]
    fn multi_toggle_on_one_line_round_trips() {
        let toggled = toggle_html(&["    foo"]);
        assert_eq!(toggled.lines, vec!["    <!--foo-->"]);
        assert_eq!(toggled.edits, vec![vec![LineEdit { column: 4, delta: 4 }]]);
        // A caret at the old EOL lands right before the appended `-->`.
        assert_eq!(shift_position((0, 7), 0, &toggled.edits), (0, 11));

        let back = toggle_html(&["    <!--foo-->"]);
        assert_eq!(back.lines, vec!["    foo"]);
        assert_eq!(
            back.edits,
            vec![vec![
                LineEdit { column: 4, delta: -4 },
                LineEdit { column: 11, delta: -3 },
            ]]
        );
        assert_eq!(shift_position((0, 11), 0, &back.edits), (0, 7));
    }

    #[test]
    fn multi_skips_blank_first_and_last_covered_lines() {
        let toggled = toggle_html(&["", "  a", "  b", "   "]);
        assert_eq!(toggled.lines, vec!["", "  <!--a", "  b-->", "   "]);
        assert!(toggled.edits[0].is_empty());
        assert!(toggled.edits[3].is_empty());
    }

    #[test]
    fn multi_uncomment_strips_right_before_trailing_whitespace() {
        let toggled = toggle_html(&["<!--foo-->   "]);
        assert_eq!(toggled.lines, vec!["foo   "], "whitespace after --> survives");
    }

    #[test]
    fn multi_partial_coverage_double_wraps_and_round_trips() {
        // Only the left half of an existing wrap is covered: not commented,
        // so it wraps again - and one more toggle returns to this state.
        let toggled = toggle_html(&["<!--foo"]);
        assert_eq!(toggled.lines, vec!["<!--<!--foo-->"]);
        let back = toggle_html(&["<!--<!--foo-->"]);
        assert_eq!(back.lines, vec!["<!--foo"]);
    }

    #[test]
    fn multi_overlapping_delimiters_on_one_line_wrap_again() {
        // "<!-->" matches left and right only by overlapping - not wrapped.
        let toggled = toggle_html(&["<!-->"]);
        assert_eq!(toggled.lines, vec!["<!--<!-->-->"]);
    }

    #[test]
    fn multi_spaced_delimiters_eat_one_space_back() {
        let style = CommentStyle::Multi { left: "<!-- ".to_string(), right: " -->".to_string() };
        let toggled = toggle_comment(&["foo"], &style).unwrap();
        assert_eq!(toggled.lines, vec!["<!-- foo -->"]);
        let back = toggle_comment(&["<!-- foo -->"], &style).unwrap();
        assert_eq!(back.lines, vec!["foo"]);
        // The eat stops at the left removal's edge instead of overlapping it.
        let hollow = toggle_comment(&["<!-- -->"], &style).unwrap();
        assert_eq!(hollow.lines, vec![""]);
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
        let edits = [vec![LineEdit { column: 4, delta: 3 }]];
        assert_eq!(shift_position((0, 2), 0, &edits), (0, 2), "before the edit");
        assert_eq!(shift_position((0, 4), 0, &edits), (0, 7), "at the edit");
        assert_eq!(shift_position((0, 9), 0, &edits), (0, 12), "after the edit");
        assert_eq!(shift_position((5, 9), 0, &edits), (5, 9), "uncovered line");
    }

    #[test]
    fn shift_position_clamps_a_caret_inside_a_removed_prefix() {
        // Caret sat on the second slash of a removed "// " (delta -3).
        let edits = [vec![LineEdit { column: 4, delta: -3 }]];
        assert_eq!(shift_position((0, 5), 0, &edits), (0, 4));
    }

    #[test]
    fn shift_position_compounds_two_removals_on_one_line() {
        // "    <!--foo-->" uncommenting: left [4,8), right [11,14).
        let edits = [vec![
            LineEdit { column: 4, delta: -4 },
            LineEdit { column: 11, delta: -3 },
        ]];
        assert_eq!(shift_position((0, 2), 0, &edits), (0, 2), "before both");
        assert_eq!(shift_position((0, 4), 0, &edits), (0, 4), "at left span start");
        assert_eq!(shift_position((0, 6), 0, &edits), (0, 4), "inside left span");
        assert_eq!(shift_position((0, 9), 0, &edits), (0, 5), "between the spans");
        assert_eq!(shift_position((0, 12), 0, &edits), (0, 7), "inside right span");
        assert_eq!(shift_position((0, 14), 0, &edits), (0, 7), "at old EOL");
    }
}
