use std::time::{Duration, Instant};

/// Cap on how many undo steps are kept - each one holds a full copy of the
/// document text (see `Snapshot`), so this bounds worst-case memory rather
/// than letting an editing session grow the stack unboundedly.
const MAX_DEPTH: usize = 200;

/// Edits within this long of the previous one are folded into the same undo
/// step, so a burst of typing undoes as one word/sentence instead of one
/// keystroke at a time.
const COALESCE_WINDOW: Duration = Duration::from_millis(750);

/// A snapshot-based undo/redo stack, standing in for the undo history
/// `iced::widget::text_editor::Content` doesn't provide. Stores
/// whole-document text rather than diffs - simple, fine at realistic
/// document sizes given the depth cap above.
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit_at: Option<Instant>,
}

struct Snapshot {
    text: String,
    cursor: (usize, usize),
}

impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit_at: None,
        }
    }

    /// Call before performing an edit action, with the document's state as
    /// it stood *before* that edit. Coalesces bursts of edits within
    /// `COALESCE_WINDOW` of each other into a single undo step, and clears
    /// the redo stack, since a fresh edit invalidates whatever future the
    /// undone-then-redoable entries pointed at.
    pub fn record_before_edit(&mut self, text: &str, cursor: (usize, usize)) {
        self.record_before_edit_at(text, cursor, Instant::now());
    }

    fn record_before_edit_at(&mut self, text: &str, cursor: (usize, usize), now: Instant) {
        let start_new_step = match self.last_edit_at {
            Some(last) => now.duration_since(last) > COALESCE_WINDOW,
            None => true,
        };
        if start_new_step {
            self.undo.push(Snapshot {
                text: text.to_string(),
                cursor,
            });
            if self.undo.len() > MAX_DEPTH {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit_at = Some(now);
    }

    /// Pops the most recent undo step, pushing `current` onto the redo stack
    /// so it can be returned to. `None` if there's nothing to undo.
    pub fn undo(
        &mut self,
        current_text: &str,
        current_cursor: (usize, usize),
    ) -> Option<(String, (usize, usize))> {
        let snapshot = self.undo.pop()?;
        self.redo.push(Snapshot {
            text: current_text.to_string(),
            cursor: current_cursor,
        });
        // The next edit should always start a fresh undo step rather than
        // possibly coalescing with whatever came before the undo.
        self.last_edit_at = None;
        Some((snapshot.text, snapshot.cursor))
    }

    /// Mirror of `undo`, restoring the most recently undone step.
    pub fn redo(
        &mut self,
        current_text: &str,
        current_cursor: (usize, usize),
    ) -> Option<(String, (usize, usize))> {
        let snapshot = self.redo.pop()?;
        self.undo.push(Snapshot {
            text: current_text.to_string(),
            cursor: current_cursor,
        });
        self.last_edit_at = None;
        Some((snapshot.text, snapshot.cursor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_on_empty_history_returns_none() {
        let mut history = History::new();
        assert!(history.undo("abc", (0, 3)).is_none());
    }

    #[test]
    fn redo_on_empty_history_returns_none() {
        let mut history = History::new();
        assert!(history.redo("abc", (0, 3)).is_none());
    }

    #[test]
    fn undo_restores_previous_text_and_cursor() {
        let mut history = History::new();
        history.record_before_edit("abc", (0, 3));
        let restored = history.undo("abcd", (0, 4));
        assert_eq!(restored, Some(("abc".to_string(), (0, 3))));
    }

    #[test]
    fn redo_after_undo_restores_the_undone_state() {
        let mut history = History::new();
        history.record_before_edit("abc", (0, 3));
        history.undo("abcd", (0, 4));
        let restored = history.redo("abc", (0, 3));
        assert_eq!(restored, Some(("abcd".to_string(), (0, 4))));
    }

    #[test]
    fn new_edit_after_undo_clears_redo_stack() {
        let mut history = History::new();
        history.record_before_edit("abc", (0, 3));
        history.undo("abcd", (0, 4));
        // A different edit happens instead of a redo.
        history.record_before_edit_at("abc", (0, 3), Instant::now());
        assert!(history.redo("abcX", (0, 4)).is_none());
    }

    #[test]
    fn rapid_edits_within_coalesce_window_collapse_to_one_undo_step() {
        let mut history = History::new();
        let t0 = Instant::now();
        history.record_before_edit_at("a", (0, 1), t0);
        history.record_before_edit_at("ab", (0, 2), t0 + Duration::from_millis(100));
        history.record_before_edit_at("abc", (0, 3), t0 + Duration::from_millis(200));

        // One undo step should skip straight back to before the whole burst.
        let restored = history.undo("abcd", (0, 4));
        assert_eq!(restored, Some(("a".to_string(), (0, 1))));
        assert!(history.undo("abcd", (0, 4)).is_none());
    }

    #[test]
    fn edits_spaced_past_the_coalesce_window_produce_separate_undo_steps() {
        let mut history = History::new();
        let t0 = Instant::now();
        history.record_before_edit_at("a", (0, 1), t0);
        history.record_before_edit_at("ab", (0, 2), t0 + COALESCE_WINDOW + Duration::from_millis(1));

        let restored = history.undo("abc", (0, 3));
        assert_eq!(restored, Some(("ab".to_string(), (0, 2))));
        let restored = history.undo("ab", (0, 2));
        assert_eq!(restored, Some(("a".to_string(), (0, 1))));
    }

    #[test]
    fn undo_stack_is_capped_at_max_depth() {
        let mut history = History::new();
        let t0 = Instant::now();
        for i in 0..MAX_DEPTH + 10 {
            let gap = t0 + (COALESCE_WINDOW + Duration::from_millis(1)) * i as u32;
            history.record_before_edit_at(&i.to_string(), (0, 0), gap);
        }
        assert_eq!(history.undo.len(), MAX_DEPTH);
    }
}
