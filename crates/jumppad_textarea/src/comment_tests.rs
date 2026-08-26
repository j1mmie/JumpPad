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
