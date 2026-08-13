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
mod tests {
    use super::*;

    /// A state pre-searched over `text` for `query`, as if just opened at
    /// the document start.
    fn searched(text: &str, query: &str) -> FindState {
        let mut state = FindState {
            query: query.to_string(),
            open: true,
            ..FindState::default()
        };
        state.search(text);
        state
    }

    #[test]
    fn search_selects_the_first_match_from_the_origin() {
        let mut state = searched("one two\nthree two", "two");
        assert_eq!(state.matches.len(), 2);
        assert_eq!(state.current, Some(0));

        // Opening lower down starts at the match below it instead.
        state.origin = (1, 0);
        state.search("one two\nthree two");
        assert_eq!(state.current, Some(1));
    }

    #[test]
    fn search_wraps_to_the_top_when_nothing_follows_the_origin() {
        let mut state = searched("two here", "two");
        state.origin = (9, 9); // past every match
        state.search("two here");
        assert_eq!(state.current, Some(0));
    }

    #[test]
    fn step_wraps_around_in_both_directions() {
        let mut state = searched("a a a", "a");
        assert_eq!(state.matches.len(), 3);
        assert_eq!(state.current, Some(0));

        state.step(1);
        assert_eq!(state.current, Some(1));
        state.step(1);
        assert_eq!(state.current, Some(2));
        state.step(1);
        assert_eq!(
            state.current,
            Some(0),
            "next past the last wraps to the first"
        );

        state.step(-1);
        assert_eq!(
            state.current,
            Some(2),
            "previous before the first wraps to the last"
        );
    }

    #[test]
    fn step_on_no_matches_selects_nothing() {
        let mut state = searched("nothing here", "zebra");
        state.step(1);
        assert_eq!(state.current, None);
        assert_eq!(state.current_match(), None);
    }

    #[test]
    fn index_at_finds_the_match_the_cursor_touches() {
        let state = searched("one two\nthree two", "two");
        assert_eq!(state.index_at((0, 4)), Some(0), "at the start of a match");
        // Selecting a match leaves the cursor at its end, which still counts.
        assert_eq!(state.index_at((0, 7)), Some(0), "at the end of a match");
        assert_eq!(state.index_at((1, 7)), Some(1));
        assert_eq!(state.index_at((0, 0)), None, "outside every match");
    }

    #[test]
    fn counter_reports_position_results_or_nothing() {
        let mut state = searched("a a a", "a");
        assert_eq!(state.counter().as_deref(), Some("1 of 3"));
        state.step(1);
        assert_eq!(state.counter().as_deref(), Some("2 of 3"));

        assert_eq!(
            searched("text", "zebra").counter().as_deref(),
            Some("No results")
        );
        assert_eq!(
            searched("text", "").counter(),
            None,
            "an empty query shows no counter"
        );
    }
}
