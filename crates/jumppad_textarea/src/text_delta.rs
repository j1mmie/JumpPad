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
#[path = "text_delta_tests.rs"]
mod tests;
