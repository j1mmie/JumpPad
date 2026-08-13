use std::ops::Range;

/// A delta: the block of lines an edit changed, plus the text on either side
/// of it. Undo and redo replay these instead of whole documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFootprint {
    /// Lines replaced, in the document as it stands.
    replaced: Range<usize>,
    /// Byte offset of `replaced.start`.
    replaced_at: usize,
    /// What those lines say now, endings included.
    displaced: String,
    /// What they will say.
    replacement: String,
}

impl EditFootprint {
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

        // Byte searches, not `str` slicing - a matching run can end
        // mid-codepoint. The boundaries below all land on one.
        let starts_at = line_start_at_or_before(before, matching_prefix);
        let tail_before = before.len() - matching_suffix;
        let tail_after = after.len() - matching_suffix;
        let advance = shared_advance_to_line_start(
            before,
            tail_before,
            after,
            tail_after,
        );

        let displaced = before[starts_at..tail_before + advance].to_owned();
        let replacement = after[starts_at..tail_after + advance].to_owned();
        let first_line = count_lines(&before.as_bytes()[..starts_at]);

        Some(Self {
            replaced: line_span(first_line, &displaced),
            replaced_at: starts_at,
            displaced,
            replacement,
        })
    }

    /// The same delta pointing the other way, so one record serves undo and
    /// redo alike.
    pub fn inverted(&self) -> Self {
        Self {
            replaced: line_span(self.replaced.start, &self.replacement),
            replaced_at: self.replaced_at,
            displaced: self.replacement.clone(),
            replacement: self.displaced.clone(),
        }
    }

    pub fn replaced_lines(&self) -> Range<usize> {
        self.replaced.clone()
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    /// Byte range to overwrite in the source cache.
    pub fn source_range(&self) -> Range<usize> {
        self.replaced_at..self.replaced_at + self.displaced.len()
    }

    /// Where highlighting has to resume from.
    pub fn first_line(&self) -> usize {
        self.replaced.start
    }

    /// Lines changed, counting the wider side.
    pub fn line_count(&self) -> usize {
        let going_out = self.replaced.end - self.replaced.start;
        let coming_in = line_span(0, &self.replacement).end;
        going_out.max(coming_in)
    }
}

fn line_start_at_or_before(text: &str, at: usize) -> usize {
    text.as_bytes()[..at]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |newline| newline + 1)
}

/// How far past the unchanged tail both documents reach to land on a line
/// boundary. One distance for both: the tail is identical in each, so
/// advancing them separately replaces the wrong number of bytes.
fn shared_advance_to_line_start(
    before: &str,
    tail_before: usize,
    after: &str,
    tail_after: usize,
) -> usize {
    if starts_a_line(before, tail_before) && starts_a_line(after, tail_after) {
        return 0;
    }
    let tail = &before.as_bytes()[tail_before..];
    tail.iter()
        .position(|byte| *byte == b'\n')
        .map_or(tail.len(), |newline| newline + 1)
}

fn starts_a_line(text: &str, at: usize) -> bool {
    at == 0 || at == text.len() || text.as_bytes()[at - 1] == b'\n'
}

/// The lines `text` occupies when spliced in at `first`. No trailing ending
/// means it runs to the end of the document.
fn line_span(first: usize, text: &str) -> Range<usize> {
    let mut lines = count_lines(text.as_bytes());
    if !text.is_empty() && !text.ends_with('\n') {
        lines += 1;
    }
    first..first + lines
}

fn count_lines(text: &[u8]) -> usize {
    text.iter().filter(|byte| **byte == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Replays a footprint the way `TextArea` does, so a test can state its
    /// expectation as the document it wants back.
    fn apply(text: &str, footprint: &EditFootprint) -> String {
        let mut applied = text.to_owned();
        applied
            .replace_range(footprint.source_range(), footprint.replacement());
        applied
    }

    #[test]
    fn identical_documents_have_no_footprint() {
        assert_eq!(EditFootprint::between("a\nb\nc", "a\nb\nc"), None);
    }

    #[test]
    fn a_footprint_covers_only_the_line_that_changed() {
        let footprint = EditFootprint::between("a\nb\nc", "a\nB\nc")
            .expect("the middle line changed");
        assert_eq!(footprint.replaced_lines(), 1..2);
        assert_eq!(footprint.replacement(), "B\n");
        assert_eq!(footprint.first_line(), 1);
    }

    #[test]
    fn applying_a_footprint_produces_the_document_it_was_taken_from() {
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
            let footprint = EditFootprint::between(before, after)
                .unwrap_or_else(|| panic!("{before:?} -> {after:?} changed"));
            assert_eq!(
                apply(before, &footprint),
                after,
                "{before:?} -> {after:?}"
            );
        }
    }

    #[test]
    fn an_inverted_footprint_walks_the_change_back() {
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
            let footprint = EditFootprint::between(before, after)
                .unwrap_or_else(|| panic!("{before:?} -> {after:?} changed"));
            let back = footprint.inverted();
            assert_eq!(apply(after, &back), before, "{before:?} -> {after:?}");
            assert_eq!(back.inverted(), footprint, "{before:?} -> {after:?}");
        }
    }

    #[test]
    fn the_replaced_range_names_the_lines_that_actually_go_out() {
        // Deleting the middle line takes lines 1..2 out of a three-line
        // document; the replacement puts nothing back in their place.
        let footprint =
            EditFootprint::between("a\nb\nc", "a\nc").expect("a line went");
        assert_eq!(footprint.replaced_lines(), 1..2);
        assert_eq!(footprint.replacement(), "");
    }

    #[test]
    fn an_insertion_past_the_last_line_replaces_it() {
        // The last line carries no ending, so appending after it has to
        // rewrite that line to give it one.
        let footprint =
            EditFootprint::between("a\nb", "a\nb\nc").expect("a line arrived");
        assert_eq!(footprint.replaced_lines(), 1..2);
        assert_eq!(footprint.replacement(), "b\nc");
    }

    #[test]
    fn a_multibyte_edit_lands_on_character_boundaries() {
        // The matching runs meet mid-codepoint here; the boundaries still
        // have to be sliceable.
        let footprint = EditFootprint::between("héllo\nworld", "héllo\nwörld")
            .expect("the second line changed");
        assert_eq!(footprint.replaced_lines(), 1..2);
        assert_eq!(footprint.replacement(), "wörld");
    }

    #[test]
    fn deleting_an_ending_reaches_past_the_line_that_lost_it() {
        // The two documents disagree about the line break where their tails
        // meet - `a\n|x` against `a|x` - so both ends have to reach further.
        let footprint =
            EditFootprint::between("a\nx", "ax").expect("an ending went");
        assert_eq!(footprint.replaced_lines(), 0..2);
        assert_eq!(footprint.replacement(), "ax");
        assert_eq!(apply("a\nx", &footprint), "ax");
    }

    #[test]
    fn line_count_counts_the_wider_side() {
        let grew = EditFootprint::between("a\nb\nc", "a\nb1\nb2\nb3\nc")
            .expect("one line became three");
        assert_eq!(grew.line_count(), 3);
        assert_eq!(grew.inverted().line_count(), 3);
    }
}
