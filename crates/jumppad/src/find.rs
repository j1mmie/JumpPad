//! State for one tab's find palette.
//!
//! Kept out of `editor_core::Tab` on purpose: `editor_core` is the boundary
//! between the shell and whatever renders text, and a palette's query string
//! is shell UI state. `JumpPadApp` keys these by the tab's stable `id`
//! instead, the same id `keyed_column` and the session manifest already use.

use editor_core::FindMatch;

/// One tab's find palette. Survives closing the palette (so reopening keeps
/// the query) but not closing the tab.
#[derive(Debug, Clone, Default)]
pub struct FindState {
    pub query: String,
    pub matches: Vec<FindMatch>,
    /// Which match is selected, if any.
    pub current: Option<usize>,
    /// Where the cursor was when the palette opened.
    ///
    /// Live search anchors here rather than at the live cursor: selecting a
    /// match moves the cursor, so re-anchoring on every keystroke would walk
    /// the selection forward through the document as the user types.
    pub origin: (usize, usize),
    /// Whether the palette is on screen. `false` keeps `query` for reopen.
    pub open: bool,
}

impl FindState {
    /// Recomputes `matches` for `text`, then re-points `current` at the
    /// first match at or after `origin`, wrapping to the top.
    pub fn search(&mut self, text: &str) {
        self.matches = editor_core::find_matches(text, &self.query);
        self.current = self.first_at_or_after(self.origin);
    }

    /// The first match starting at or after `position`, wrapping around to
    /// the first match overall when everything lies before it. `None` only
    /// when there are no matches at all.
    pub fn first_at_or_after(&self, position: (usize, usize)) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let at_or_after = self
            .matches
            .iter()
            .position(|found| (found.line, found.start) >= position);
        Some(at_or_after.unwrap_or(0))
    }

    /// The match `position` falls inside, treating the range as inclusive of
    /// its end so a cursor resting just past a match still counts as
    /// touching it - which is where selecting a match leaves the cursor.
    pub fn index_at(&self, position: (usize, usize)) -> Option<usize> {
        self.matches.iter().position(|found| {
            position.0 == found.line
                && position.1 >= found.start
                && position.1 <= found.end
        })
    }

    /// Steps `current` by `delta` matches, wrapping at both ends. Starts
    /// from the first match after `origin` if nothing is selected yet.
    pub fn step(&mut self, delta: isize) {
        if self.matches.is_empty() {
            self.current = None;
            return;
        }
        let count = self.matches.len() as isize;
        self.current = Some(match self.current {
            Some(current) => {
                (current as isize + delta).rem_euclid(count) as usize
            }
            None => self.first_at_or_after(self.origin).unwrap_or(0),
        });
    }

    /// The match `current` points at, if any.
    pub fn current_match(&self) -> Option<FindMatch> {
        self.matches.get(self.current?).copied()
    }

    /// The palette's counter text: `None` hides it entirely.
    pub fn counter(&self) -> Option<String> {
        if self.query.is_empty() {
            return None;
        }
        if self.matches.is_empty() {
            return Some("No results".to_string());
        }
        let position = match self.current {
            Some(current) => current + 1,
            None => 0,
        };
        Some(format!("{position} of {}", self.matches.len()))
    }
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
