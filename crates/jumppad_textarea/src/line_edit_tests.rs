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
