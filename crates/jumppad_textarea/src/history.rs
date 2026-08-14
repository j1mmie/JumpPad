use std::sync::Arc;
use std::time::{Duration, Instant};

use editor_core::{Debounce, SavedSelection};

use crate::text_delta::TextDelta;

/// Where the caret was and what was selected. Undoing an edit that replaced
/// a selection has to put the selection back too, not just the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorState {
    pub position: (usize, usize),
    pub selection: Option<SavedSelection>,
}

/// Fallback for a `History` no config has reached yet. Mirrors
/// `HistoryConfig`'s default.
pub const DEFAULT_DEPTH: usize = 200;

/// The fallback step boundary, for edits with no word to end on - a held
/// backspace would otherwise become one enormous step.
const COALESCE_WINDOW: Duration = Duration::from_millis(750);

/// A delta-based undo/redo stack, standing in for the one
/// `text_editor::Content` doesn't provide.
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    burst: Debounce,
    open: Option<BurstInProgress>,
    depth: usize,
}

/// One entry on either stack.
struct Step {
    delta: TextDelta,
    cursor: CursorState,
}

/// A burst still being typed, held as the document from before it began.
/// An `Arc` clone of the source cache, so it costs a refcount, not a copy.
struct BurstInProgress {
    before: Arc<String>,
    cursor: CursorState,
}

impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            burst: Debounce::new(COALESCE_WINDOW),
            open: None,
            depth: DEFAULT_DEPTH,
        }
    }

    /// Clamped to at least one, so `depth = 0` can't turn undo off.
    pub fn set_depth(&mut self, depth: usize) {
        self.depth = depth.max(1);
        self.trim();
    }

    /// Call before an edit, with the document as it stood before it. Only
    /// the first edit of a burst is kept, so the step's cursor is the state
    /// from before the whole burst.
    pub fn record_before_edit(
        &mut self,
        text: &Arc<String>,
        cursor: CursorState,
    ) {
        self.record_before_edit_at(text, cursor, Instant::now());
    }

    fn record_before_edit_at(
        &mut self,
        text: &Arc<String>,
        cursor: CursorState,
        now: Instant,
    ) {
        if self.burst.poke(now) {
            // This text is both the last burst's result and this one's start.
            self.close_open_burst(text);
            self.open = Some(BurstInProgress {
                before: text.clone(),
                cursor,
            });
        }
        self.redo.clear();
    }

    /// Ends the open burst, so the next edit starts a new step. What word
    /// boundaries and caret moves call.
    pub fn end_burst(&mut self) {
        self.burst.reset();
    }

    /// Records an undo step that stands alone: it neither joins the typing
    /// burst before it nor absorbs the edit after it.
    pub fn record_isolated(&mut self, text: &Arc<String>, cursor: CursorState) {
        self.burst.reset();
        self.record_before_edit(text, cursor);
        self.burst.reset();
    }

    /// The delta that walks the most recent step back, and the caret to put
    /// back with it. The redo entry's caret is the one live at undo time.
    pub fn undo(
        &mut self,
        current_text: &Arc<String>,
        current_cursor: CursorState,
    ) -> Option<(TextDelta, CursorState)> {
        self.close_open_burst(current_text);
        let step = self.undo.pop()?;
        let undoing = step.delta.inverted();
        self.redo.push(Step {
            delta: step.delta,
            cursor: current_cursor,
        });
        self.burst.reset(); // the next edit starts a fresh step
        Some((undoing, step.cursor))
    }

    /// Mirror of `undo`, replaying the most recently undone step.
    pub fn redo(
        &mut self,
        current_text: &Arc<String>,
        current_cursor: CursorState,
    ) -> Option<(TextDelta, CursorState)> {
        self.close_open_burst(current_text);
        let step = self.redo.pop()?;
        let redoing = step.delta.clone();
        self.undo.push(Step {
            delta: step.delta,
            cursor: current_cursor,
        });
        self.burst.reset();
        Some((redoing, step.cursor))
    }

    /// Folds the open burst into one step. A burst that ended where it
    /// began leaves no step.
    fn close_open_burst(&mut self, current_text: &str) {
        let Some(open) = self.open.take() else {
            return;
        };
        let Some(delta) = TextDelta::between(&open.before, current_text) else {
            return;
        };
        self.undo.push(Step {
            delta,
            cursor: open.cursor,
        });
        self.trim();
    }

    fn trim(&mut self) {
        if self.undo.len() > self.depth {
            self.undo.drain(..self.undo.len() - self.depth);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::SelectionKind;

    /// A caret with nothing selected - what most of these tests care about.
    fn caret(line: usize, column: usize) -> CursorState {
        CursorState {
            position: (line, column),
            selection: None,
        }
    }

    /// A caret at `cursor` with a selection anchored at `anchor`.
    fn selected(
        anchor: (usize, usize),
        cursor: (usize, usize),
        kind: SelectionKind,
    ) -> CursorState {
        CursorState {
            position: cursor,
            selection: Some(SavedSelection { anchor, kind }),
        }
    }

    fn doc(text: &str) -> Arc<String> {
        Arc::new(text.to_owned())
    }

    fn replayed(text: &str, delta: &TextDelta) -> String {
        let mut applied = text.to_owned();
        applied.replace_range(delta.source_range(), delta.replacement());
        applied
    }

    /// Undo, reporting the document it produces rather than the change that
    /// produces it - what these tests want to assert about.
    fn undone(
        history: &mut History,
        text: &str,
        cursor: CursorState,
    ) -> Option<(String, CursorState)> {
        let (delta, restored) = history.undo(&doc(text), cursor)?;
        Some((replayed(text, &delta), restored))
    }

    /// Mirror of [`undone`].
    fn redone(
        history: &mut History,
        text: &str,
        cursor: CursorState,
    ) -> Option<(String, CursorState)> {
        let (delta, restored) = history.redo(&doc(text), cursor)?;
        Some((replayed(text, &delta), restored))
    }

    #[test]
    fn undo_on_empty_history_returns_none() {
        let mut history = History::new();
        assert!(undone(&mut history, "abc", caret(0, 3)).is_none());
    }

    #[test]
    fn redo_on_empty_history_returns_none() {
        let mut history = History::new();
        assert!(redone(&mut history, "abc", caret(0, 3)).is_none());
    }

    #[test]
    fn undo_restores_previous_text_and_cursor() {
        let mut history = History::new();
        history.record_before_edit(&doc("abc"), caret(0, 3));
        let restored = undone(&mut history, "abcd", caret(0, 4));
        assert_eq!(restored, Some(("abc".to_string(), caret(0, 3))));
    }

    #[test]
    fn redo_after_undo_restores_the_undone_state() {
        let mut history = History::new();
        history.record_before_edit(&doc("abc"), caret(0, 3));
        undone(&mut history, "abcd", caret(0, 4));
        let restored = redone(&mut history, "abc", caret(0, 3));
        assert_eq!(restored, Some(("abcd".to_string(), caret(0, 4))));
    }

    #[test]
    fn new_edit_after_undo_clears_redo_stack() {
        let mut history = History::new();
        history.record_before_edit(&doc("abc"), caret(0, 3));
        undone(&mut history, "abcd", caret(0, 4));
        // A different edit happens instead of a redo.
        history.record_before_edit_at(&doc("abc"), caret(0, 3), Instant::now());
        assert!(redone(&mut history, "abcX", caret(0, 4)).is_none());
    }

    #[test]
    fn rapid_edits_within_coalesce_window_collapse_to_one_undo_step() {
        let mut history = History::new();
        let t0 = Instant::now();
        history.record_before_edit_at(&doc("a"), caret(0, 1), t0);
        history.record_before_edit_at(
            &doc("ab"),
            caret(0, 2),
            t0 + Duration::from_millis(100),
        );
        history.record_before_edit_at(
            &doc("abc"),
            caret(0, 3),
            t0 + Duration::from_millis(200),
        );

        // One undo step should skip straight back to before the whole burst.
        let restored = undone(&mut history, "abcd", caret(0, 4));
        assert_eq!(restored, Some(("a".to_string(), caret(0, 1))));
        assert!(undone(&mut history, "abcd", caret(0, 4)).is_none());
    }

    #[test]
    fn an_isolated_record_stays_its_own_step_inside_a_typing_burst() {
        let mut history = History::new();
        // All three records land well inside one coalesce window; without
        // the isolation they would collapse into a single step.
        history.record_before_edit(&doc("a"), caret(0, 1));
        history.record_isolated(&doc("ab"), caret(0, 2));
        history.record_before_edit(&doc("ab//"), caret(0, 4));

        let restored = undone(&mut history, "ab//c", caret(0, 5));
        assert_eq!(restored, Some(("ab//".to_string(), caret(0, 4))));
        let restored = undone(&mut history, "ab//", caret(0, 4));
        assert_eq!(restored, Some(("ab".to_string(), caret(0, 2))));
        let restored = undone(&mut history, "ab", caret(0, 2));
        assert_eq!(restored, Some(("a".to_string(), caret(0, 1))));
    }

    #[test]
    fn an_isolated_record_clears_the_redo_stack() {
        let mut history = History::new();
        history.record_before_edit(&doc("abc"), caret(0, 3));
        undone(&mut history, "abcd", caret(0, 4));
        history.record_isolated(&doc("abc"), caret(0, 3));
        assert!(redone(&mut history, "// abc", caret(0, 6)).is_none());
    }

    #[test]
    fn edits_spaced_past_the_coalesce_window_produce_separate_undo_steps() {
        let mut history = History::new();
        let t0 = Instant::now();
        history.record_before_edit_at(&doc("a"), caret(0, 1), t0);
        history.record_before_edit_at(
            &doc("ab"),
            caret(0, 2),
            t0 + COALESCE_WINDOW + Duration::from_millis(1),
        );

        let restored = undone(&mut history, "abc", caret(0, 3));
        assert_eq!(restored, Some(("ab".to_string(), caret(0, 2))));
        let restored = undone(&mut history, "ab", caret(0, 2));
        assert_eq!(restored, Some(("a".to_string(), caret(0, 1))));
    }

    #[test]
    fn undo_stack_is_capped_at_the_configured_depth() {
        let mut history = History::new();
        let t0 = Instant::now();
        for i in 0..DEFAULT_DEPTH + 10 {
            let gap =
                t0 + (COALESCE_WINDOW + Duration::from_millis(1)) * i as u32;
            history.record_before_edit_at(
                &doc(&i.to_string()),
                caret(0, 0),
                gap,
            );
        }
        assert_eq!(history.undo.len(), DEFAULT_DEPTH);
    }

    #[test]
    fn lowering_the_depth_trims_a_stack_that_is_already_deeper() {
        let mut history = History::new();
        let t0 = Instant::now();
        for i in 0..10 {
            let gap =
                t0 + (COALESCE_WINDOW + Duration::from_millis(1)) * i as u32;
            history.record_before_edit_at(
                &doc(&i.to_string()),
                caret(0, 0),
                gap,
            );
        }
        history.set_depth(3);
        assert_eq!(history.undo.len(), 3);
        // The oldest steps are what go: the newest is still on top.
        let restored = undone(&mut history, "9", caret(0, 0));
        assert_eq!(restored, Some(("8".to_string(), caret(0, 0))));
    }

    #[test]
    fn a_depth_of_zero_still_keeps_one_step() {
        let mut history = History::new();
        history.set_depth(0);
        history.record_before_edit(&doc("abc"), caret(0, 3));
        let restored = undone(&mut history, "abcd", caret(0, 4));
        assert_eq!(restored, Some(("abc".to_string(), caret(0, 3))));
    }

    #[test]
    fn a_burst_that_ends_where_it_began_records_no_step() {
        // Typed and then deleted back inside one coalesce window. The old
        // snapshot stack pushed a step here that restored identical text.
        let mut history = History::new();
        let t0 = Instant::now();
        history.record_before_edit_at(&doc("abc"), caret(0, 3), t0);
        history.record_before_edit_at(
            &doc("abcd"),
            caret(0, 4),
            t0 + Duration::from_millis(100),
        );
        assert!(undone(&mut history, "abc", caret(0, 3)).is_none());
    }

    #[test]
    fn a_step_holds_only_the_characters_its_edit_disturbed() {
        // The point of the whole exercise: undoing a small change in a long
        // document carries the change, not the line and not the document.
        let before: String =
            (0..500).map(|i| format!("line {i}\n")).collect::<String>();
        let after = before.replace("line 200\n", "line 200 edited\n");

        let mut history = History::new();
        history.record_before_edit(&Arc::new(before.clone()), caret(200, 8));
        let (delta, _) = history
            .undo(&Arc::new(after.clone()), caret(200, 15))
            .expect("something to undo");

        // Undoing takes the added text back out and puts nothing in.
        assert_eq!(delta.replacement(), "");
        assert_eq!(delta.source_range().len(), " edited".len());
        assert_eq!(delta.first_line(), 200);
        assert_eq!(replayed(&after, &delta), before);
    }

    #[test]
    fn undo_restores_the_selection_that_preceded_the_edit() {
        let mut history = History::new();
        let before = selected((0, 0), (0, 5), SelectionKind::Range);
        history.record_before_edit(&doc("hello world"), before);
        // The edit replaced the selection, so the caret is now collapsed.
        let restored = undone(&mut history, " world", caret(0, 0));
        assert_eq!(restored, Some(("hello world".to_string(), before)));
    }

    #[test]
    fn undo_preserves_the_selection_kind() {
        // Guards the `SavedSelection` shape: a word/line selection carries its
        // bounds in the kind, not in an anchor-to-cursor span, so flattening
        // the step back to a bare pair of positions would lose them.
        for kind in [
            SelectionKind::Range,
            SelectionKind::Word,
            SelectionKind::Line,
        ] {
            let mut history = History::new();
            let before = selected((0, 2), (0, 2), kind);
            history.record_before_edit(&doc("hello"), before);
            let restored = undone(&mut history, "h", caret(0, 1));
            assert_eq!(
                restored,
                Some(("hello".to_string(), before)),
                "kind: {kind:?}"
            );
        }
    }

    #[test]
    fn undo_restores_no_selection_when_there_was_none() {
        let mut history = History::new();
        history.record_before_edit(&doc("abc"), caret(0, 3));
        let restored = undone(&mut history, "abcd", caret(0, 4));
        assert_eq!(restored.expect("something to undo").1.selection, None);
    }

    #[test]
    fn a_coalesced_burst_keeps_the_selection_from_before_its_first_edit() {
        // Typing over a selection: the first keystroke replaces it, so only
        // that first record carries one. Coalescing must keep that state
        // rather than letting a later selection-less one overwrite it.
        let mut history = History::new();
        let t0 = Instant::now();
        let before = selected((0, 0), (0, 5), SelectionKind::Range);
        history.record_before_edit_at(&doc("hello world"), before, t0);
        history.record_before_edit_at(
            &doc("X world"),
            caret(0, 1),
            t0 + Duration::from_millis(100),
        );
        history.record_before_edit_at(
            &doc("Xy world"),
            caret(0, 2),
            t0 + Duration::from_millis(200),
        );

        let restored = undone(&mut history, "Xyz world", caret(0, 3));
        assert_eq!(restored, Some(("hello world".to_string(), before)));
        assert!(undone(&mut history, "Xyz world", caret(0, 3)).is_none());
    }

    #[test]
    fn redo_returns_the_caret_state_as_it_stood_when_undo_ran() {
        // Deliberate divergence from VS Code, which captures the after-state
        // at edit time instead - see the note on `undo`.
        let mut history = History::new();
        history.record_before_edit(&doc("abc"), caret(0, 3));
        // The caret wandered before the undo; that is what redo comes back to.
        let at_undo_time = selected((0, 0), (0, 4), SelectionKind::Range);
        undone(&mut history, "abcd", at_undo_time);
        let restored = redone(&mut history, "abc", caret(0, 3));
        assert_eq!(restored, Some(("abcd".to_string(), at_undo_time)));
    }

    #[test]
    fn undo_and_redo_walk_a_multi_step_stack_in_order() {
        let mut history = History::new();
        let t0 = Instant::now();
        let apart =
            |i: u32| t0 + (COALESCE_WINDOW + Duration::from_millis(1)) * i;
        history.record_before_edit_at(&doc("one"), caret(0, 3), apart(0));
        history.record_before_edit_at(&doc("one two"), caret(0, 7), apart(1));
        history.record_before_edit_at(
            &doc("one two three"),
            caret(0, 13),
            apart(2),
        );

        let (text, _) =
            undone(&mut history, "one two three four", caret(0, 18))
                .expect("three steps in");
        assert_eq!(text, "one two three");
        let (text, _) =
            undone(&mut history, &text, caret(0, 13)).expect("two steps in");
        assert_eq!(text, "one two");
        let (text, _) =
            undone(&mut history, &text, caret(0, 7)).expect("one step in");
        assert_eq!(text, "one");
        assert!(undone(&mut history, &text, caret(0, 3)).is_none());

        let (text, _) = redone(&mut history, &text, caret(0, 3)).expect("redo");
        assert_eq!(text, "one two");
        let (text, _) = redone(&mut history, &text, caret(0, 7)).expect("redo");
        assert_eq!(text, "one two three");
        let (text, _) =
            redone(&mut history, &text, caret(0, 13)).expect("redo");
        assert_eq!(text, "one two three four");
        assert!(redone(&mut history, &text, caret(0, 18)).is_none());
    }
}
