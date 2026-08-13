use std::ops::Range;

/// The part of the document an edit disturbed, and both sides of it - the
/// unit undo and redo replay instead of a whole document. Everything outside
/// it keeps its shaped layout and its highlight attributes, so an undo costs
/// the lines it changes rather than the file they sit in.
///
/// Boundaries are whole lines: that is the unit
/// [`TextArea::paste_over_lines`] splices in, and it makes the line numbers
/// the caller needs fall out of the byte offsets for free.
///
/// [`TextArea::paste_over_lines`]: crate::TextArea
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFootprint {
    /// Lines this replaces, numbered in the document as it stands.
    replaced: Range<usize>,
    /// Byte offset of `replaced.start`, so the source cache can be spliced
    /// without walking the document to re-derive it.
    replaced_at: usize,
    /// Exactly what stands there now, line endings included. Byte-exact
    /// rather than a list of lines, so restoring can't normalize an ending
    /// the way re-joining split lines would.
    displaced: String,
    /// Exactly what goes in its place.
    replacement: String,
}

impl EditFootprint {
    /// The smallest whole-line replacement that turns `before` into `after`,
    /// or `None` if they already match.
    ///
    /// Two memcmp-speed passes over the document, against the full rebuild
    /// this exists to avoid.
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
        // Capped so the two runs can't claim the same bytes, which is what
        // keeps the line boundaries below from crossing.
        let matching_suffix = before
            .as_bytes()
            .iter()
            .rev()
            .zip(after.as_bytes().iter().rev())
            .take_while(|(a, b)| a == b)
            .count()
            .min(overlap - matching_prefix);

        // Byte searches, not `str` slicing: a matching run can end mid-
        // codepoint, where slicing would panic. Every boundary these produce
        // is a document start, an end, or just past a `\n` - all char
        // boundaries, so the slices at the bottom are safe.
        let starts_at = line_start_at_or_before(before, matching_prefix);
        let tail_before = before.len() - matching_suffix;
        let tail_after = after.len() - matching_suffix;
        let advance = shared_advance_to_line_start(
            before,
            tail_before,
            after,
            tail_after,
        );
        let ends_before = tail_before + advance;
        let ends_after = tail_after + advance;

        let displaced = before[starts_at..ends_before].to_owned();
        let replacement = after[starts_at..ends_after].to_owned();
        let first_line = count_lines(&before.as_bytes()[..starts_at]);

        Some(Self {
            replaced: line_span(first_line, &displaced),
            replaced_at: starts_at,
            displaced,
            replacement,
        })
    }

    /// The same change pointing the other way. One stored footprint serves
    /// undo and redo alike, which is why nothing else has to be kept.
    pub fn inverted(&self) -> Self {
        Self {
            replaced: line_span(self.replaced.start, &self.replacement),
            replaced_at: self.replaced_at,
            displaced: self.replacement.clone(),
            replacement: self.displaced.clone(),
        }
    }

    /// The lines to splice over, in the document as it stands.
    pub fn replaced_lines(&self) -> Range<usize> {
        self.replaced.clone()
    }

    /// The bytes that go in, endings included.
    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    /// Where the change lands in the source cache, for a `replace_range`.
    pub fn source_range(&self) -> Range<usize> {
        self.replaced_at..self.replaced_at + self.displaced.len()
    }

    /// The first line the change touches - the point highlighting has to
    /// resume from, and everything above it keeps the colors it has.
    pub fn first_line(&self) -> usize {
        self.replaced.start
    }

    /// How many lines this moves, counting the wider of its two sides.
    /// A footprint past a certain reach is a whole-file replacement rather
    /// than an edit, and wants rebuilding rather than splicing.
    pub fn line_reach(&self) -> usize {
        let going_out = self.replaced.end - self.replaced.start;
        let coming_in = line_span(0, &self.replacement).end;
        going_out.max(coming_in)
    }
}

/// The start of the line containing `at`.
fn line_start_at_or_before(text: &str, at: usize) -> usize {
    text.as_bytes()[..at]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |newline| newline + 1)
}

/// How far past the unchanged tail both documents must reach to end on a
/// line boundary. One distance for both sides, not one each: the tail is the
/// same bytes in each document, so advancing by the same amount leaves them
/// the same length - which is what makes the two halves of a footprint
/// describe a single change. Advancing them separately silently produces a
/// footprint that replaces the wrong number of bytes.
///
/// Any advance past the first byte lands on the shared tail, where the two
/// documents necessarily agree about where the lines break. Only standing
/// still can disagree, so that case wants both sides already on a boundary.
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

/// Whether `at` is the start of a line - the top of the document, the end of
/// it, or just past an ending.
fn starts_a_line(text: &str, at: usize) -> bool {
    at == 0 || at == text.len() || text.as_bytes()[at - 1] == b'\n'
}

/// The line range `text` occupies when spliced in starting at `first`. A
/// trailing ending is one the splice swallows; its absence means the block
/// runs to the end of the document, where there is no ending to take.
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
        // The two documents disagree about where the line breaks sit at the
        // point their tails meet - `a\n|x` against `a|x` - so the footprint
        // has to reach to the end of both rather than stopping where only
        // one of them has a boundary.
        let footprint =
            EditFootprint::between("a\nx", "ax").expect("an ending went");
        assert_eq!(footprint.replaced_lines(), 0..2);
        assert_eq!(footprint.replacement(), "ax");
        assert_eq!(apply("a\nx", &footprint), "ax");
    }

    #[test]
    fn line_reach_counts_the_wider_side() {
        let grew = EditFootprint::between("a\nb\nc", "a\nb1\nb2\nb3\nc")
            .expect("one line became three");
        assert_eq!(grew.line_reach(), 3);
        assert_eq!(grew.inverted().line_reach(), 3);
    }
}
