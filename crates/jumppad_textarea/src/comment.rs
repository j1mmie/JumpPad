//! The toggle-comment transformation: pure functions over line texts,
//! glued to the document by `TextArea::toggle_comment`.

use crate::line_edit::{EditedLines, LineEdit};

/// A file type's comment syntax - this crate's own mirror of the config
/// type, so it doesn't depend on `jumppad_config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentStyle {
    Single(String),
    Multi { left: String, right: String },
}

/// Uncomments `lines` when the coverage is already commented in `style`,
/// else comments it. `None` = nothing to do.
pub fn toggle_comment(
    lines: &[&str],
    style: &CommentStyle,
) -> Option<EditedLines> {
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
fn toggle_single(lines: &[&str], prefix: &str) -> Option<EditedLines> {
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

fn single_comment(lines: &[&str], prefix: &str) -> EditedLines {
    let insert_col = lines
        .iter()
        .filter(|line| !blank(line))
        .map(|line| leading_whitespace_len(line))
        .min()
        .unwrap_or(0);

    let mut toggled = EditedLines::default();
    for line in lines {
        if blank(line) {
            toggled.push_unchanged(line);
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
        toggled.push(
            commented,
            vec![LineEdit {
                column,
                delta: prefix.len() as isize,
            }],
        );
    }
    toggled
}

fn single_uncomment(lines: &[&str], prefix: &str, token: &str) -> EditedLines {
    let mut toggled = EditedLines::default();
    for line in lines {
        if blank(line) {
            toggled.push_unchanged(line);
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
        toggled.push(
            uncommented,
            vec![LineEdit {
                column,
                delta: -(removed as isize),
            }],
        );
    }
    toggled
}

/// Multi-line style: `left` goes after the first non-blank line's leading
/// whitespace, `right` at the last one's very end - trailing whitespace
/// included in the comment. Lines in between are untouched content.
fn toggle_multi(
    lines: &[&str],
    left: &str,
    right: &str,
) -> Option<EditedLines> {
    let left_token = left.trim_end();
    let right_token = right.trim_start();
    if left_token.is_empty() || right_token.is_empty() {
        return None;
    }
    let first = lines.iter().position(|line| !blank(line))?;
    let last = lines.iter().rposition(|line| !blank(line))?;

    let toggled = if is_wrapped(
        lines[first],
        lines[last],
        first == last,
        left_token,
        right_token,
    ) {
        multi_uncomment(
            lines,
            first,
            last,
            left,
            right,
            left_token,
            right_token,
        )
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

fn multi_comment(
    lines: &[&str],
    first: usize,
    last: usize,
    left: &str,
    right: &str,
) -> EditedLines {
    let mut toggled = EditedLines::default();
    for (index, line) in lines.iter().enumerate() {
        let mut text = line.to_string();
        let mut edits = Vec::new();
        if index == first {
            let column = leading_whitespace_len(line);
            text.insert_str(column, left);
            edits.push(LineEdit {
                column,
                delta: left.len() as isize,
            });
        }
        if index == last {
            // Appended after trailing whitespace, with no edit recorded: an
            // end-of-line caret must stay put, i.e. before `right`.
            text.push_str(right);
        }
        toggled.push(text, edits);
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
) -> EditedLines {
    let mut toggled = EditedLines::default();
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
            let floor =
                left_span.map_or(0, |(column, removed)| column + removed);
            if right.starts_with(' ')
                && start > floor
                && line.as_bytes()[start - 1] == b' '
            {
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
            edits.push(LineEdit {
                column,
                delta: -(removed as isize),
            });
        }
        if let Some((start, removed)) = right_span {
            edits.push(LineEdit {
                column: start,
                delta: -(removed as isize),
            });
        }
        toggled.push(text, edits);
    }
    toggled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_edit::shift_position;

    fn single(prefix: &str) -> CommentStyle {
        CommentStyle::Single(prefix.to_string())
    }

    fn html_multi() -> CommentStyle {
        CommentStyle::Multi {
            left: "<!--".to_string(),
            right: "-->".to_string(),
        }
    }

    fn toggle(lines: &[&str]) -> EditedLines {
        toggle_comment(lines, &single("// ")).unwrap()
    }

    fn toggle_html(lines: &[&str]) -> EditedLines {
        toggle_comment(lines, &html_multi()).unwrap()
    }

    #[test]
    fn comments_a_single_line() {
        let toggled = toggle(&["fn main() {}"]);
        assert_eq!(toggled.lines, vec!["// fn main() {}"]);
        assert_eq!(
            toggled.edits,
            vec![vec![LineEdit {
                column: 0,
                delta: 3
            }]]
        );
    }

    #[test]
    fn toggling_twice_round_trips() {
        let toggled = toggle(&["    let x = 1;"]);
        assert_eq!(toggled.lines, vec!["    // let x = 1;"]);
        let refs: Vec<&str> =
            toggled.lines.iter().map(String::as_str).collect();
        let back = toggle(&refs);
        assert_eq!(back.lines, vec!["    let x = 1;"]);
        assert_eq!(
            back.edits,
            vec![vec![LineEdit {
                column: 4,
                delta: -3
            }]]
        );
    }

    #[test]
    fn uncomments_a_prefix_without_the_space() {
        let toggled = toggle(&["//fn"]);
        assert_eq!(toggled.lines, vec!["fn"]);
        assert_eq!(
            toggled.edits,
            vec![vec![LineEdit {
                column: 0,
                delta: -2
            }]]
        );
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
        let toggled = toggle(&[
            "            if (a > b) {",
            "                return false",
        ]);
        assert_eq!(
            toggled.lines,
            vec![
                "            // if (a > b) {",
                "            //     return false"
            ]
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
        let style = CommentStyle::Multi {
            left: "  ".to_string(),
            right: "-->".to_string(),
        };
        assert!(toggle_comment(&["text"], &style).is_none());
        let style = CommentStyle::Multi {
            left: "<!--".to_string(),
            right: String::new(),
        };
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
        assert_eq!(
            toggled.edits[0],
            vec![LineEdit {
                column: 8,
                delta: 4
            }]
        );
        assert!(toggled.edits[1].is_empty(), "an EOL append moves no caret");
        // The selection's ends keep covering the same characters.
        assert_eq!(shift_position((0, 24), 0, &toggled.edits), (0, 28));
        assert_eq!(shift_position((1, 25), 0, &toggled.edits), (1, 25));

        // And the second toggle returns everything exactly.
        let refs: Vec<&str> =
            toggled.lines.iter().map(String::as_str).collect();
        let back = toggle_html(&refs);
        assert_eq!(back.lines.as_slice(), &lines);
        assert_eq!(
            back.edits[0],
            vec![LineEdit {
                column: 8,
                delta: -4
            }]
        );
        let right_start = toggled.lines[1].len() - 3;
        assert_eq!(
            back.edits[1],
            vec![LineEdit {
                column: right_start,
                delta: -3
            }]
        );
        assert_eq!(shift_position((0, 28), 0, &back.edits), (0, 24));
        assert_eq!(shift_position((1, 25), 0, &back.edits), (1, 25));
    }

    #[test]
    fn multi_toggle_on_one_line_round_trips() {
        let toggled = toggle_html(&["    foo"]);
        assert_eq!(toggled.lines, vec!["    <!--foo-->"]);
        assert_eq!(
            toggled.edits,
            vec![vec![LineEdit {
                column: 4,
                delta: 4
            }]]
        );
        // A caret at the old EOL lands right before the appended `-->`.
        assert_eq!(shift_position((0, 7), 0, &toggled.edits), (0, 11));

        let back = toggle_html(&["    <!--foo-->"]);
        assert_eq!(back.lines, vec!["    foo"]);
        assert_eq!(
            back.edits,
            vec![vec![
                LineEdit {
                    column: 4,
                    delta: -4
                },
                LineEdit {
                    column: 11,
                    delta: -3
                },
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
        assert_eq!(
            toggled.lines,
            vec!["foo   "],
            "whitespace after --> survives"
        );
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
        let style = CommentStyle::Multi {
            left: "<!-- ".to_string(),
            right: " -->".to_string(),
        };
        let toggled = toggle_comment(&["foo"], &style).unwrap();
        assert_eq!(toggled.lines, vec!["<!-- foo -->"]);
        let back = toggle_comment(&["<!-- foo -->"], &style).unwrap();
        assert_eq!(back.lines, vec!["foo"]);
        // The eat stops at the left removal's edge instead of overlapping it.
        let hollow = toggle_comment(&["<!-- -->"], &style).unwrap();
        assert_eq!(hollow.lines, vec![""]);
    }
}
