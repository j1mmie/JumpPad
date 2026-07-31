//! Case-insensitive, single-file text search backing the find palette.
//!
//! Pure text logic with no widget or iced dependency, so it unit-tests
//! without a GUI. Lives here rather than in the app shell because
//! [`FindMatch`] crosses the [`crate::TextEditorWidget`] boundary - the app
//! produces matches, the widget renders them.

/// One occurrence of a find query, as byte columns within a single line -
/// the same units [`crate::TextEditorWidget::cursor_position`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindMatch {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// Every non-overlapping, case-insensitive occurrence of `query` in `text`,
/// ordered top to bottom then left to right. An empty query matches nothing.
///
/// Searches line by line rather than over the whole string: it keeps the
/// returned columns line-relative with no offset arithmetic, and sidesteps
/// the line-ending ambiguity in `Content::text()`. Multiline matching is out
/// of scope, so nothing is lost by not searching across the joins.
pub fn find_matches(text: &str, query: &str) -> Vec<FindMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    let mut matches = Vec::new();

    for (line_index, raw_line) in text.split('\n').enumerate() {
        // `text` joins lines with their own ending, so a CRLF document
        // leaves the `\r` on the end of each split piece.
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let (lowered, spans) = lower_with_spans(line);

        let mut from = 0;
        while let Some(hit) = lowered[from..].find(&needle) {
            let lowered_start = from + hit;
            let lowered_end = lowered_start + needle.len();
            matches.push(FindMatch {
                line: line_index,
                start: spans[lowered_start].0,
                // `- 1` so the span consulted is the last byte *inside* the
                // match rather than the one past it.
                end: spans[lowered_end - 1].1,
            });
            from = lowered_end;
        }
    }
    matches
}

/// Lowercases `line`, and for every byte of the result records the byte
/// range of the original character that produced it.
///
/// The table is what keeps matches aligned: `to_lowercase` is not
/// length-preserving (`İ` lowercases to two characters), so an offset found
/// in the lowered string does not index the original. Mapping through a
/// character's full range also means a match landing part-way into a
/// multi-character expansion covers that whole character instead of
/// splitting it.
fn lower_with_spans(line: &str) -> (String, Vec<(usize, usize)>) {
    let mut lowered = String::with_capacity(line.len());
    let mut spans = Vec::with_capacity(line.len());

    for (index, character) in line.char_indices() {
        let span = (index, index + character.len_utf8());
        for lowered_character in character.to_lowercase() {
            for _ in 0..lowered_character.len_utf8() {
                spans.push(span);
            }
            lowered.push(lowered_character);
        }
    }
    (lowered, spans)
}

#[cfg(test)]
mod tests {
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
        assert_eq!(found[0], FindMatch { line: 0, start: 0, end: 5 });
        assert!(found.iter().all(|f| matched(text, f).eq_ignore_ascii_case("hello")));

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
                FindMatch { line: 0, start: 4, end: 7 },
                FindMatch { line: 1, start: 6, end: 9 },
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
        assert_eq!(found, vec![FindMatch { line: 0, start: 0, end: 2 }]);
    }

    #[test]
    fn carriage_returns_are_not_part_of_the_line() {
        // A CRLF document must not leave `\r` inside a match at line end.
        let text = "find me\r\nsecond";
        let found = find_matches(text, "me");
        assert_eq!(found, vec![FindMatch { line: 0, start: 5, end: 7 }]);
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
}
