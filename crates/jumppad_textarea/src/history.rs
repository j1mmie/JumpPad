use std::time::{Duration, Instant};

use editor_core::{Debounce, SavedSelection};

/// Where the caret was and what was selected - the state undo has to put
/// back, not just the cursor pair. Undoing an edit that replaced a
/// selection (a cut, a paste, or typing over one) has to re-select it, or
/// it leaves the document in a state that never existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorState {
    pub position: (usize, usize),
    pub selection: Option<SavedSelection>,
}

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
    /// A burst of edits is one undo step - its leading edge is what pushes
    /// a snapshot.
    burst: Debounce,
}

struct Snapshot {
    text: String,
    cursor: CursorState,
}

impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            burst: Debounce::new(COALESCE_WINDOW),
        }
    }

    /// Call before performing an edit action, with the document's state as
    /// it stood *before* that edit. Coalesces bursts of edits within
    /// `COALESCE_WINDOW` of each other into a single undo step, and clears
    /// the redo stack, since a fresh edit invalidates whatever future the
    /// undone-then-redoable entries pointed at.
    ///
    /// Coalescing keeps the *first* snapshot of a burst rather than
    /// overwriting it, which is what makes `cursor` the state from before
    /// the whole burst - and so the selection it carries the one the first
    /// keystroke replaced. Selection-restoring undo depends on that.
    pub fn record_before_edit(&mut self, text: &str, cursor: CursorState) {
        self.record_before_edit_at(text, cursor, Instant::now());
    }

    fn record_before_edit_at(&mut self, text: &str, cursor: CursorState, now: Instant) {
        if self.burst.poke(now) {
            self.undo.push(Snapshot {
                text: text.to_string(),
                cursor,
            });
            if self.undo.len() > MAX_DEPTH {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    /// Records an undo step that stands alone: it neither joins the typing
    /// burst before it nor absorbs the edit after it.
    pub fn record_isolated(&mut self, text: &str, cursor: CursorState) {
        self.burst.reset();
        self.record_before_edit(text, cursor);
        self.burst.reset();
    }

    /// Pops the most recent undo step, pushing `current` onto the redo stack
    /// so it can be returned to. `None` if there's nothing to undo.
    ///
    /// The redo entry's caret is the state as it stands *now*, at undo time -
    /// not as it stood when the edit was made. VS Code records the latter
    /// (`afterCursorState`, captured at edit time), so redoing diverges from
    /// it if the caret moved between the edit and the undo. Matching it would
    /// mean an `after: Option<CursorState>` on `Snapshot`, filled in after
    /// each `perform` and overwritten on every coalesced keystroke.
    pub fn undo(
        &mut self,
        current_text: &str,
        current_cursor: CursorState,
    ) -> Option<(String, CursorState)> {
        let snapshot = self.undo.pop()?;
        self.redo.push(Snapshot {
            text: current_text.to_string(),
            cursor: current_cursor,
        });
        // The next edit should always start a fresh undo step rather than
        // possibly coalescing with whatever came before the undo.
        self.burst.reset();
        Some((snapshot.text, snapshot.cursor))
    }

    /// Mirror of `undo`, restoring the most recently undone step.
    pub fn redo(
        &mut self,
        current_text: &str,
        current_cursor: CursorState,
    ) -> Option<(String, CursorState)> {
        let snapshot = self.redo.pop()?;
        self.undo.push(Snapshot {
            text: current_text.to_string(),
            cursor: current_cursor,
        });
        self.burst.reset();
        Some((snapshot.text, snapshot.cursor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::SelectionKind;

    /// A caret with nothing selected - what most of these tests care about.
    fn caret(line: usize, column: usize) -> CursorState {
        CursorState { position: (line, column), selection: None }
    }

    /// A caret at `cursor` with a selection anchored at `anchor`.
    fn selected(anchor: (usize, usize), cursor: (usize, usize), kind: SelectionKind) -> CursorState {
        CursorState {
            position: cursor,
            selection: Some(SavedSelection { anchor, kind }),
        }
    }

    #[test]
    fn undo_on_empty_history_returns_none() {
        let mut history = History::new();
        assert!(history.undo("abc", caret(0, 3)).is_none());
    }

    #[test]
    fn redo_on_empty_history_returns_none() {
        let mut history = History::new();
        assert!(history.redo("abc", caret(0, 3)).is_none());
    }

    #[test]
    fn undo_restores_previous_text_and_cursor() {
        let mut history = History::new();
        history.record_before_edit("abc", caret(0, 3));
        let restored = history.undo("abcd", caret(0, 4));
        assert_eq!(restored, Some(("abc".to_string(), caret(0, 3))));
    }

    #[test]
    fn redo_after_undo_restores_the_undone_state() {
        let mut history = History::new();
        history.record_before_edit("abc", caret(0, 3));
        history.undo("abcd", caret(0, 4));
        let restored = history.redo("abc", caret(0, 3));
        assert_eq!(restored, Some(("abcd".to_string(), caret(0, 4))));
    }

    #[test]
    fn new_edit_after_undo_clears_redo_stack() {
        let mut history = History::new();
        history.record_before_edit("abc", caret(0, 3));
        history.undo("abcd", caret(0, 4));
        // A different edit happens instead of a redo.
        history.record_before_edit_at("abc", caret(0, 3), Instant::now());
        assert!(history.redo("abcX", caret(0, 4)).is_none());
    }

    #[test]
    fn rapid_edits_within_coalesce_window_collapse_to_one_undo_step() {
        let mut history = History::new();
        let t0 = Instant::now();
        history.record_before_edit_at("a", caret(0, 1), t0);
        history.record_before_edit_at("ab", caret(0, 2), t0 + Duration::from_millis(100));
        history.record_before_edit_at("abc", caret(0, 3), t0 + Duration::from_millis(200));

        // One undo step should skip straight back to before the whole burst.
        let restored = history.undo("abcd", caret(0, 4));
        assert_eq!(restored, Some(("a".to_string(), caret(0, 1))));
        assert!(history.undo("abcd", caret(0, 4)).is_none());
    }

    #[test]
    fn an_isolated_record_stays_its_own_step_inside_a_typing_burst() {
        let mut history = History::new();
        // All three records land well inside one coalesce window; without
        // the isolation they would collapse into a single step.
        history.record_before_edit("a", caret(0, 1));
        history.record_isolated("ab", caret(0, 2));
        history.record_before_edit("ab//", caret(0, 4));

        let restored = history.undo("ab//c", caret(0, 5));
        assert_eq!(restored, Some(("ab//".to_string(), caret(0, 4))));
        let restored = history.undo("ab//", caret(0, 4));
        assert_eq!(restored, Some(("ab".to_string(), caret(0, 2))));
        let restored = history.undo("ab", caret(0, 2));
        assert_eq!(restored, Some(("a".to_string(), caret(0, 1))));
    }

    #[test]
    fn an_isolated_record_clears_the_redo_stack() {
        let mut history = History::new();
        history.record_before_edit("abc", caret(0, 3));
        history.undo("abcd", caret(0, 4));
        history.record_isolated("abc", caret(0, 3));
        assert!(history.redo("// abc", caret(0, 6)).is_none());
    }

    #[test]
    fn edits_spaced_past_the_coalesce_window_produce_separate_undo_steps() {
        let mut history = History::new();
        let t0 = Instant::now();
        history.record_before_edit_at("a", caret(0, 1), t0);
        history.record_before_edit_at(
            "ab",
            caret(0, 2),
            t0 + COALESCE_WINDOW + Duration::from_millis(1),
        );

        let restored = history.undo("abc", caret(0, 3));
        assert_eq!(restored, Some(("ab".to_string(), caret(0, 2))));
        let restored = history.undo("ab", caret(0, 2));
        assert_eq!(restored, Some(("a".to_string(), caret(0, 1))));
    }

    #[test]
    fn undo_stack_is_capped_at_max_depth() {
        let mut history = History::new();
        let t0 = Instant::now();
        for i in 0..MAX_DEPTH + 10 {
            let gap = t0 + (COALESCE_WINDOW + Duration::from_millis(1)) * i as u32;
            history.record_before_edit_at(&i.to_string(), caret(0, 0), gap);
        }
        assert_eq!(history.undo.len(), MAX_DEPTH);
    }

    #[test]
    fn undo_restores_the_selection_that_preceded_the_edit() {
        let mut history = History::new();
        let before = selected((0, 0), (0, 5), SelectionKind::Range);
        history.record_before_edit("hello world", before);
        // The edit replaced the selection, so the caret is now collapsed.
        let restored = history.undo(" world", caret(0, 0));
        assert_eq!(restored, Some(("hello world".to_string(), before)));
    }

    #[test]
    fn undo_preserves_the_selection_kind() {
        // Guards the `SavedSelection` shape: a word/line selection carries its
        // bounds in the kind, not in an anchor-to-cursor span, so flattening
        // the snapshot back to a bare pair of positions would lose them.
        for kind in [SelectionKind::Range, SelectionKind::Word, SelectionKind::Line] {
            let mut history = History::new();
            let before = selected((0, 2), (0, 2), kind);
            history.record_before_edit("hello", before);
            let restored = history.undo("h", caret(0, 1));
            assert_eq!(restored, Some(("hello".to_string(), before)), "kind: {kind:?}");
        }
    }

    #[test]
    fn undo_restores_no_selection_when_there_was_none() {
        let mut history = History::new();
        history.record_before_edit("abc", caret(0, 3));
        let restored = history.undo("abcd", caret(0, 4));
        assert_eq!(restored.expect("something to undo").1.selection, None);
    }

    #[test]
    fn a_coalesced_burst_keeps_the_selection_from_before_its_first_edit() {
        // Typing over a selection: the first keystroke replaces it, so only
        // that first record carries one. Coalescing must keep that snapshot
        // rather than letting a later selection-less one overwrite it.
        let mut history = History::new();
        let t0 = Instant::now();
        let before = selected((0, 0), (0, 5), SelectionKind::Range);
        history.record_before_edit_at("hello world", before, t0);
        history.record_before_edit_at("X world", caret(0, 1), t0 + Duration::from_millis(100));
        history.record_before_edit_at("Xy world", caret(0, 2), t0 + Duration::from_millis(200));

        let restored = history.undo("Xyz world", caret(0, 3));
        assert_eq!(restored, Some(("hello world".to_string(), before)));
        assert!(history.undo("Xyz world", caret(0, 3)).is_none());
    }

    #[test]
    fn redo_returns_the_caret_state_as_it_stood_when_undo_ran() {
        // Deliberate divergence from VS Code, which captures the after-state
        // at edit time instead - see the note on `undo`.
        let mut history = History::new();
        history.record_before_edit("abc", caret(0, 3));
        // The caret wandered before the undo; that is what redo comes back to.
        let at_undo_time = selected((0, 0), (0, 4), SelectionKind::Range);
        history.undo("abcd", at_undo_time);
        let restored = history.redo("abc", caret(0, 3));
        assert_eq!(restored, Some(("abcd".to_string(), at_undo_time)));
    }
}
