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
fn a_line_selection_covers_the_anchor_line_only() {
    let selection = Some(SavedSelection {
        anchor: (2, 5),
        kind: SelectionKind::Line,
    });
    assert_eq!(covered_lines((2, 5), selection), (2, 2));
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
