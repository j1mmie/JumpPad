use super::*;

fn default() -> WordSeparators {
    WordSeparators::default()
}

#[test]
fn a_word_is_what_no_separator_and_no_space_interrupts() {
    let separators = default();
    // The whole line is one word, underscores and digits included.
    assert_eq!(separators.run_around("snake_case_99", 4), 0..13);
    assert_eq!(separators.end_after("snake_case_99", 0), 13);
    assert_eq!(separators.start_before("snake_case_99", 13), 0);
}

#[test]
fn a_separator_ends_a_word() {
    let separators = default();
    assert_eq!(separators.run_around("foo.bar", 1), 0..3);
    assert_eq!(separators.run_around("foo.bar", 5), 4..7);
    assert_eq!(separators.end_after("foo.bar", 0), 3);
    assert_eq!(separators.start_before("foo.bar", 7), 4);
}

#[test]
fn a_separator_the_config_leaves_out_stays_inside_the_word() {
    // The point of the whole setting: `-` dropped from the list makes
    // kebab-case one word rather than three.
    let separators =
        WordSeparators::new("`~!@#$%^&*()=+[{]}\\|;:'\",.<>/?");
    assert_eq!(separators.run_around("font-size-large", 6), 0..15);
    assert_eq!(separators.end_after("font-size-large", 0), 15);
    assert_eq!(separators.start_before("font-size-large", 15), 0);
    // And it still ends at one that is on the list.
    assert_eq!(separators.run_around("font-size:12", 6), 0..9);
}

#[test]
fn whitespace_separates_words_whatever_the_list_says() {
    let separators = WordSeparators::new("");
    assert_eq!(separators.run_around("foo.bar baz", 2), 0..7);
    assert_eq!(separators.end_after("foo.bar baz", 0), 7);
    assert_eq!(separators.start_before("foo.bar baz", 11), 8);
    // A tab and a newline are whitespace too - a line holds no newline,
    // but a tab is ordinary indentation.
    assert_eq!(separators.run_around("\tfoo", 2), 1..4);
}

#[test]
fn a_run_of_separators_is_a_word_of_its_own() {
    let separators = default();
    // Double clicking the arrow in `a->b` takes the arrow, not `a` and
    // not `b`.
    assert_eq!(separators.run_around("a->b", 2), 1..3);
    // And the caret walks over it rather than past it.
    assert_eq!(separators.end_after("a->b", 1), 3);
    assert_eq!(separators.start_before("a->b", 3), 1);
}

#[test]
fn a_double_click_between_two_classes_takes_the_higher_ranked_one() {
    let separators = default();
    // Against the end of a word, from either side of the `=`, and in the
    // whitespace beyond it - the word wins, then the separator, and only
    // then the space.
    assert_eq!(separators.run_around("a = b", 1), 0..1);
    assert_eq!(separators.run_around("a = b", 2), 2..3);
    assert_eq!(separators.run_around("a = b", 3), 2..3);
    assert_eq!(separators.run_around("a  =", 2), 1..3);
}

#[test]
fn a_double_click_in_whitespace_takes_only_the_whitespace() {
    let separators = default();
    assert_eq!(separators.run_around("foo   bar", 4), 3..6);
    // Against a word on one side, the word wins.
    assert_eq!(separators.run_around("foo   bar", 3), 0..3);
    assert_eq!(separators.run_around("foo   bar", 6), 6..9);
}

#[test]
fn the_line_ends_a_word_as_surely_as_a_separator_does() {
    let separators = default();
    assert_eq!(separators.end_after("foo", 1), 3);
    // Trailing whitespace is no part of the word, but with no word left
    // ahead of it the caret runs out at the end of the line.
    assert_eq!(separators.end_after("foo   ", 1), 3);
    assert_eq!(separators.end_after("foo   ", 3), 6);
    assert_eq!(separators.start_before("foo", 1), 0);
    assert_eq!(separators.start_before("   foo", 3), 0);
    assert_eq!(separators.run_around("", 0), 0..0);
}

#[test]
fn a_caret_already_at_a_word_start_reaches_back_past_it() {
    // What the motion is for: pressing word-left twice crosses two
    // words rather than sticking on the boundary between them.
    let separators = default();
    assert_eq!(separators.start_before("alpha beta", 6), 0);
    assert_eq!(separators.end_after("alpha beta", 5), 10);
}

#[test]
fn a_multibyte_character_is_never_split() {
    let separators = default();
    // "héllo wörld": the columns below land mid-character, which the
    // scan has to back off from rather than panic on.
    let line = "héllo wörld";
    assert_eq!(separators.run_around(line, 2), 0..6);
    assert_eq!(separators.end_after(line, 2), 6);
    assert_eq!(separators.start_before(line, 9), 7);
    // Past the end of the line, too.
    assert_eq!(separators.end_after(line, 99), line.len());
    assert_eq!(separators.start_before(line, 99), 7);
    // And a non-ASCII separator is still a separator.
    let dashes = WordSeparators::new("—");
    assert_eq!(dashes.run_around("a—b", 0), 0..1);
    assert_eq!(dashes.end_after("a—b", 1), 4);
}

#[test]
fn the_default_list_is_the_one_vs_code_ships() {
    // Pinned by hand rather than derived, since the point of it is to be
    // the same characters someone's `settings.json` already names.
    assert_eq!(DEFAULT_SEPARATORS, "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?");
    assert_eq!(
        WordSeparators::default(),
        WordSeparators::new(DEFAULT_SEPARATORS)
    );
}
