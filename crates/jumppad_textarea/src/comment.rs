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
#[path = "comment_tests.rs"]
mod tests;
