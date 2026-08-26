use super::*;

fn spaces(width: u16) -> Indentation {
    Indentation::new(IndentationStyle::Spaces, width)
}

fn tabs(width: u16) -> Indentation {
    Indentation::new(IndentationStyle::Tabs, width)
}

#[test]
fn the_tabs_style_inserts_one_character_whatever_the_width() {
    for width in [1, 2, 4, 8] {
        assert_eq!(tabs(width).text_at(0), "\t");
        assert_eq!(tabs(width).text_at(7), "\t");
    }
}

#[test]
fn spaces_reach_the_next_stop() {
    // The user-facing spec example: width 8, caret at column 12, the next
    // stop at 16, so four spaces.
    assert_eq!(spaces(8).text_at(12), "    ");
}

#[test]
fn a_caret_already_on_a_stop_gets_a_whole_width() {
    assert_eq!(spaces(4).text_at(0), "    ");
    assert_eq!(spaces(4).text_at(4), "    ");
    assert_eq!(spaces(8).text_at(16), " ".repeat(8));
}

#[test]
fn spaces_fill_the_remainder_of_the_stop_the_caret_is_inside() {
    assert_eq!(spaces(4).text_at(1), "   ");
    assert_eq!(spaces(4).text_at(2), "  ");
    assert_eq!(spaces(4).text_at(3), " ");
}

#[test]
fn a_width_out_of_range_is_pulled_back_into_it() {
    assert_eq!(Indentation::new(IndentationStyle::Tabs, 0).width(), 1);
    assert_eq!(Indentation::new(IndentationStyle::Tabs, 999).width(), 16);
    // And the arithmetic still terminates, rather than dividing by zero.
    assert_eq!(spaces(0).text_at(3), " ");
}

#[test]
fn the_default_is_tabs_at_four() {
    let indentation = Indentation::default();
    assert_eq!(indentation.style(), IndentationStyle::Tabs);
    assert_eq!(indentation.width(), DEFAULT_WIDTH);
}

#[test]
fn a_column_with_no_tabs_before_it_counts_characters() {
    assert_eq!(spaces(4).visual_column("let x = 1;", 7), 7);
    assert_eq!(spaces(4).visual_column("    let x", 4), 4);
}

#[test]
fn a_tab_before_the_column_advances_to_its_stop() {
    // One tab, then two characters: 4 + 2.
    assert_eq!(spaces(4).visual_column("\tab", 3), 6);
    // Two tabs cover two stops, not one plus a character.
    assert_eq!(spaces(4).visual_column("\t\t", 2), 8);
}

#[test]
fn a_tab_part_way_through_a_stop_still_lands_on_the_next_one() {
    // "ab" leaves the column at 2; the tab covers the remaining 2.
    assert_eq!(spaces(4).visual_column("ab\tc", 4), 5);
    // A tab from a column already on a stop covers a whole width.
    assert_eq!(spaces(4).visual_column("abcd\t", 5), 8);
}

#[test]
fn the_column_is_measured_at_the_width_in_hand() {
    assert_eq!(spaces(2).visual_column("\tx", 2), 3);
    assert_eq!(spaces(8).visual_column("\tx", 2), 9);
}

#[test]
fn a_multibyte_character_counts_one_column_not_its_bytes() {
    // "é" is two bytes, so the byte column past it is 2.
    assert_eq!(spaces(4).visual_column("é", 2), 1);
    assert_eq!(spaces(4).visual_column("ééx", 5), 3);
}

#[test]
fn a_byte_column_past_the_line_measures_the_whole_line() {
    // `clamp_position` keeps this from happening, but the arithmetic
    // should not depend on that - and must not slice a `str` to do it.
    assert_eq!(spaces(4).visual_column("ab", 99), 2);
    assert_eq!(spaces(4).visual_column("", 99), 0);
}

fn indent(indentation: Indentation, lines: &[&str]) -> Vec<String> {
    indentation.indent_lines(lines).unwrap().lines
}

fn outdent(indentation: Indentation, lines: &[&str]) -> Vec<String> {
    indentation.outdent_lines(lines).unwrap().lines
}

#[test]
fn every_line_of_a_block_gains_one_indent() {
    assert_eq!(indent(tabs(4), &["aaa", "\tbbb"]), ["\taaa", "\t\tbbb"]);
    assert_eq!(
        indent(spaces(4), &["aaa", "    bbb"]),
        ["    aaa", "        bbb"]
    );
}

#[test]
fn a_half_indented_line_is_pushed_onto_the_next_stop() {
    // Two spaces at width four reach the stop with two more, not four.
    assert_eq!(indent(spaces(4), &["  aaa"]), ["    aaa"]);
    assert_eq!(indent(spaces(4), &["     aaa"]), ["        aaa"]);
}

#[test]
fn an_indented_block_records_where_each_line_grew() {
    let indented = spaces(4).indent_lines(&["aaa", "  bbb"]).unwrap();
    assert_eq!(
        indented.edits,
        vec![
            vec![LineEdit {
                column: 0,
                delta: 4
            }],
            vec![LineEdit {
                column: 0,
                delta: 2
            }],
        ]
    );
}

#[test]
fn a_blank_line_is_left_out_of_a_block_indent() {
    // Indenting one only buys it trailing whitespace.
    assert_eq!(
        indent(tabs(4), &["aaa", "", "  ", "bbb"]),
        ["\taaa", "", "  ", "\tbbb"]
    );
}

#[test]
fn a_block_of_nothing_but_blank_lines_has_no_indent_to_make() {
    assert_eq!(tabs(4).indent_lines(&["", "   ", ""]), None);
}

#[test]
fn an_outdent_undoes_an_indent_line_for_line() {
    for indentation in [tabs(4), spaces(4), spaces(8)] {
        let lines = ["aaa", "\t\tbbb", "", "        ccc"];
        let indented = indent(indentation, &lines);
        let borrowed: Vec<&str> =
            indented.iter().map(String::as_str).collect();
        assert_eq!(
            outdent(indentation, &borrowed),
            lines,
            "{indentation:?}"
        );
    }
}

#[test]
fn a_line_between_two_stops_is_pulled_onto_one_either_way() {
    // Which is why the round trip above needs lines that start on a stop:
    // a line that doesn't gets straightened out by the first press,
    // whichever direction it was.
    assert_eq!(indent(spaces(4), &["  aaa"]), ["    aaa"]);
    assert_eq!(outdent(spaces(4), &["  aaa"]), ["aaa"]);
}

#[test]
fn an_outdent_falls_back_to_the_previous_stop() {
    assert_eq!(outdent(spaces(4), &["     aaa"]), ["    aaa"]);
    assert_eq!(outdent(spaces(4), &["    aaa"]), ["aaa"]);
    assert_eq!(outdent(spaces(4), &["  aaa"]), ["aaa"]);
}

#[test]
fn an_outdent_takes_a_whole_tab_and_never_a_character_of_content() {
    assert_eq!(outdent(tabs(4), &["\t\taaa"]), ["\taaa"]);
    assert_eq!(outdent(spaces(4), &["\taaa"]), ["aaa"]);
    assert_eq!(outdent(tabs(4), &["  aaa"]), ["aaa"]);
}

#[test]
fn an_outdent_records_what_it_took_off_the_front() {
    let outdented = spaces(4).outdent_lines(&["    aaa", "bbb"]).unwrap();
    assert_eq!(outdented.lines, ["aaa", "bbb"]);
    assert_eq!(
        outdented.edits,
        vec![
            vec![LineEdit {
                column: 0,
                delta: -4
            }],
            Vec::new(),
        ]
    );
}

#[test]
fn a_block_already_at_the_margin_has_no_outdent_to_make() {
    assert_eq!(tabs(4).outdent_lines(&["aaa", "", "bbb"]), None);
}

#[test]
fn a_mixed_indent_gives_something_up_on_every_press() {
    // A tab that overshoots the stop still goes when it is the first
    // character removed, so repeated presses can't stall part way.
    let mut line = " \t aaa".to_string();
    for _ in 0..3 {
        let outdented = spaces(4).outdent_lines(&[&line]);
        line = outdented.expect("still indented").lines.remove(0);
    }
    assert_eq!(line, "aaa");
}

#[test]
fn an_indent_from_a_tabbed_line_reaches_the_next_stop() {
    // The two halves together: a caret after one tab on a width-4 line
    // is at column 4, so the indent is a full four spaces.
    let indentation = spaces(4);
    let column = indentation.visual_column("\t", 1);
    assert_eq!(indentation.text_at(column), "    ");
}
