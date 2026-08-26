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
#[path = "find_tests.rs"]
mod tests;
