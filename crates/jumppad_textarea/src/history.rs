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
#[path = "history_tests.rs"]
mod tests;
