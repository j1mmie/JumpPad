use std::ops::Range;

/// A delta: the run of characters an edit changed, plus the text on either
/// side. Undo and redo replay these instead of whole documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDelta {
    /// Byte offset in the document as it stands.
    at: usize,
    /// What stands there now.
    displaced: String,
    /// What goes in its place.
    replacement: String,
    /// The line `at` falls on. Inversion doesn't move it - everything before
    /// `at` is identical in both documents.
    first_line: usize,
}

impl TextDelta {
    /// The delta between two documents, or `None` if they match.
    pub fn between(before: &str, after: &str) -> Option<Self> {
        if before == after {
            return None;
        }

        let overlap = before.len().min(after.len());
        let matching_prefix = before
            .as_bytes()
            .iter()
            .zip(after.as_bytes())
            .take_while(|(a, b)| a == b)
            .count();
        // Capped so the two runs can't claim the same bytes.
        let matching_suffix = before
            .as_bytes()
            .iter()
            .rev()
            .zip(after.as_bytes().iter().rev())
            .take_while(|(a, b)| a == b)
            .count()
            .min(overlap - matching_prefix);

        // A matching run can end mid-codepoint, so both ends round out to a
        // character boundary before anything gets sliced.
        let at = char_boundary_at_or_before(before, matching_prefix);
        let tail_before = before.len() - matching_suffix;
        let tail_after = after.len() - matching_suffix;
        let advance = shared_advance_to_char_boundary(
            before,
            tail_before,
            after,
            tail_after,
        );

        Some(Self {
            at,
            displaced: before[at..tail_before + advance].to_owned(),
            replacement: after[at..tail_after + advance].to_owned(),
            first_line: count_lines(&before.as_bytes()[..at]),
        })
    }

    /// The same delta pointing the other way, so one record serves undo and
    /// redo alike.
    pub fn inverted(&self) -> Self {
        Self {
            at: self.at,
            displaced: self.replacement.clone(),
            replacement: self.displaced.clone(),
            first_line: self.first_line,
        }
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    /// Byte range to overwrite, in the document as it stands.
    pub fn source_range(&self) -> Range<usize> {
        self.at..self.at + self.displaced.len()
    }

    /// Where highlighting has to resume from.
    pub fn first_line(&self) -> usize {
        self.first_line
    }

    /// Lines touched, counting the wider side. The splice guard reads this
    /// because cosmic-text's line vector is what makes pasting quadratic.
    pub fn line_count(&self) -> usize {
        let going_out = count_lines(self.displaced.as_bytes());
        let coming_in = count_lines(self.replacement.as_bytes());
        going_out.max(coming_in) + 1
    }
}

fn char_boundary_at_or_before(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// How far past the unchanged tail both documents reach to land on a
/// character boundary. One distance for both, never one each - that is what
/// keeps the two sides the same length, and so describing one change.
fn shared_advance_to_char_boundary(
    before: &str,
    tail_before: usize,
    after: &str,
    tail_after: usize,
) -> usize {
    let mut advance = 0;
    while !before.is_char_boundary(tail_before + advance)
        || !after.is_char_boundary(tail_after + advance)
    {
        advance += 1;
    }
    advance
}

fn count_lines(text: &[u8]) -> usize {
    text.iter().filter(|byte| **byte == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Replays a delta the way `TextArea` does, so a test can state its
    /// expectation as the document it wants back.
    fn apply(text: &str, delta: &TextDelta) -> String {
        let mut applied = text.to_owned();
        applied.replace_range(delta.source_range(), delta.replacement());
        applied
    }

    #[test]
    fn identical_documents_have_no_delta() {
        assert_eq!(TextDelta::between("a\nb\nc", "a\nb\nc"), None);
    }

    #[test]
    fn a_delta_covers_only_the_characters_that_changed() {
        let delta = TextDelta::between("a\nb\nc", "a\nB\nc")
            .expect("the middle line changed");
        assert_eq!(delta.source_range(), 2..3);
        assert_eq!(delta.replacement(), "B");
        assert_eq!(delta.first_line(), 1);
    }

    #[test]
    fn typing_a_word_stores_the_word_and_not_the_line() {
        // The reason this is character-precise: with word-sized undo steps,
        // line-rounding made each step carry the whole line again.
        let delta =
            TextDelta::between("Hello ", "Hello my ").expect("a word arrived");
        assert_eq!(delta.replacement(), "my ");
        assert_eq!(delta.source_range(), 6..6);
    }

    #[test]
    fn applying_a_delta_produces_the_document_it_was_taken_from() {
        for (before, after) in [
            ("a\nb\nc", "a\nB\nc"),         // replace a middle line
            ("a\nb\nc", "a\nb\nc\nd"),      // append past the end
            ("a\nb\nc", "a\nc"),            // delete a middle line
            ("a\nb\nc", "a\nb"),            // delete the last line
            ("a\nb\nc", "X\nb\nc"),         // change the first line
            ("a\nb\nc", "a\nb\nC"),         // change the last line
            ("a\nb\nc", "a\nb1\nb2\nc"),    // one line becomes two
            ("a\nb\nc", ""),                // clear the document
            ("", "a\nb\nc"),                // fill an empty one
            ("a\r\nb\r\nc", "a\r\nB\r\nc"), // CRLF stays CRLF
            ("a\nb\nc", "a\nb\nc\n"),       // gain a trailing blank line
            ("a\nx", "ax"),                 // two lines join into one
            ("ax", "a\nx"),                 // and split apart again
        ] {
            let delta = TextDelta::between(before, after)
                .unwrap_or_else(|| panic!("{before:?} -> {after:?} changed"));
            assert_eq!(apply(before, &delta), after, "{before:?} -> {after:?}");
        }
    }

    #[test]
    fn an_inverted_delta_walks_the_change_back() {
        for (before, after) in [
            ("a\nb\nc", "a\nB\nc"),
            ("a\nb\nc", "a\nb\nc\nd"),
            ("a\nb\nc", "a\nc"),
            ("a\nb\nc", "a\nb"),
            ("a\nb\nc", ""),
            ("", "a\nb\nc"),
            ("a\r\nb\r\nc", "a\r\nB\r\nc"),
            ("a\nx", "ax"),
        ] {
            let delta = TextDelta::between(before, after)
                .unwrap_or_else(|| panic!("{before:?} -> {after:?} changed"));
            let back = delta.inverted();
            assert_eq!(apply(after, &back), before, "{before:?} -> {after:?}");
            assert_eq!(back.inverted(), delta, "{before:?} -> {after:?}");
        }
    }

    #[test]
    fn a_deletion_takes_only_the_characters_it_removes() {
        let delta = TextDelta::between("a\nb\nc", "a\nc").expect("a line went");
        assert_eq!(delta.source_range(), 2..4);
        assert_eq!(delta.replacement(), "");
    }

    #[test]
    fn an_insertion_past_the_last_line_carries_its_ending() {
        let delta =
            TextDelta::between("a\nb", "a\nb\nc").expect("a line arrived");
        assert_eq!(delta.source_range(), 3..3);
        assert_eq!(delta.replacement(), "\nc");
    }

    #[test]
    fn multibyte_edits_land_on_character_boundaries() {
        // Both ends of each of these meets mid-codepoint.
        for (before, after, replacement) in [
            ("héllo", "hello", "e"),
            ("hello", "héllo", "é"),
            ("héllo", "hÖllo", "Ö"),
            ("wörld", "wörld!", "!"),
            ("é", "ö", "ö"),
        ] {
            let delta = TextDelta::between(before, after)
                .unwrap_or_else(|| panic!("{before:?} -> {after:?} changed"));
            assert_eq!(delta.replacement(), replacement, "{before:?}");
            assert_eq!(apply(before, &delta), after, "{before:?}");
            assert_eq!(
                apply(after, &delta.inverted()),
                before,
                "{before:?} inverted"
            );
        }
    }

    #[test]
    fn deleting_an_ending_takes_the_ending_and_nothing_else() {
        let delta = TextDelta::between("a\nx", "ax").expect("an ending went");
        assert_eq!(delta.source_range(), 1..2);
        assert_eq!(delta.replacement(), "");
        assert_eq!(apply("a\nx", &delta), "ax");
    }

    #[test]
    fn line_count_counts_the_wider_side() {
        let grew = TextDelta::between("a\nb\nc", "a\nb1\nb2\nb3\nc")
            .expect("one line became three");
        assert_eq!(grew.line_count(), 3);
        assert_eq!(grew.inverted().line_count(), 3);
    }
}
