use super::*;

/// The matched text itself, for asserting on results readably.
fn matched<'a>(text: &'a str, found: &FindMatch) -> &'a str {
    let line = text.split('\n').nth(found.line).expect("line in range");
    &line[found.start..found.end]
}

#[test]
fn matches_regardless_of_case() {
    let text = "Hello HELLO hello";
    let found = find_matches(text, "hello");
    assert_eq!(found.len(), 3);
    assert_eq!(
        found[0],
        FindMatch {
            line: 0,
            start: 0,
            end: 5
        }
    );
    assert!(
        found
            .iter()
            .all(|f| matched(text, f).eq_ignore_ascii_case("hello"))
    );

    // A mixed-case query finds the same three.
    assert_eq!(find_matches(text, "HeLLo").len(), 3);
}

#[test]
fn reports_line_relative_columns_across_lines() {
    let text = "one two\nthree two";
    let found = find_matches(text, "two");
    assert_eq!(
        found,
        vec![
            FindMatch {
                line: 0,
                start: 4,
                end: 7
            },
            FindMatch {
                line: 1,
                start: 6,
                end: 9
            },
        ]
    );
}

#[test]
fn empty_query_matches_nothing() {
    assert!(find_matches("anything at all", "").is_empty());
}

#[test]
fn absent_query_matches_nothing() {
    assert!(find_matches("one two three", "zebra").is_empty());
}

#[test]
fn occurrences_do_not_overlap() {
    // "aa" in "aaa" is one match, not two - the second would overlap.
    let found = find_matches("aaa", "aa");
    assert_eq!(
        found,
        vec![FindMatch {
            line: 0,
            start: 0,
            end: 2
        }]
    );
}

#[test]
fn carriage_returns_are_not_part_of_the_line() {
    // A CRLF document must not leave `\r` inside a match at line end.
    let text = "find me\r\nsecond";
    let found = find_matches(text, "me");
    assert_eq!(
        found,
        vec![FindMatch {
            line: 0,
            start: 5,
            end: 7
        }]
    );
}

#[test]
fn non_ascii_text_keeps_offsets_aligned() {
    // Regression guard for the lowering-offset table. `É` is two bytes,
    // so a naive search of a lowercased copy would slice mid-character.
    let text = "CAFÉ and café";
    let found = find_matches(text, "café");
    assert_eq!(found.len(), 2);
    assert_eq!(matched(text, &found[0]), "CAFÉ");
    assert_eq!(matched(text, &found[1]), "café");
}

#[test]
fn a_match_inside_a_growing_lowercase_expansion_covers_the_character() {
    // `İ` (2 bytes) lowercases to `i` + a combining dot (3 bytes), so a
    // search for "i" lands part-way into the expansion. The whole
    // original character is reported rather than a split of it.
    let text = "İstanbul";
    let found = find_matches(text, "i");
    assert_eq!(found.len(), 1);
    assert_eq!(matched(text, &found[0]), "İ");
}

#[test]
fn matches_are_ordered_and_slice_cleanly() {
    let text = "ab ab\nab";
    let found = find_matches(text, "ab");
    assert_eq!(found.len(), 3);
    // Every reported range must be a valid slice of its own line.
    for entry in &found {
        assert_eq!(matched(text, entry), "ab");
    }
}
