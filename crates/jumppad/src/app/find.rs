use super::*;

impl JumpPadApp {
    /// Whether the active tab's find palette is currently showing.
    pub(super) fn find_is_open(&self) -> bool {
        self.tabs
            .get(self.active)
            .and_then(|tab| self.find.get(&tab.id))
            .is_some_and(|state| state.open)
    }

    /// Re-points the counter at whichever match the cursor now touches,
    /// without re-searching - for cursor moves that changed no text.
    pub(super) fn sync_find_counter(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(state) = self.find.get_mut(&tab.id) else {
            return;
        };
        if !state.open {
            return;
        }
        let cursor = tab.editor.cursor_position();
        if let Some(touched) = state.index_at(cursor) {
            state.current = Some(touched);
            tab.editor
                .set_find_matches(state.matches.clone(), state.current);
        }
    }

    /// Re-runs the active tab's search against its current text and pushes
    /// the result to its editor for coloring. Called whenever either side
    /// can have moved: the query, the document, or which tab is active.
    pub(super) fn refresh_find(&mut self) {
        self.refresh_find_for(self.active);
    }

    /// Same, for a tab that isn't the active one - a background tab whose
    /// buffer was replaced by an external reload still holds ranges into the
    /// text that just went away.
    pub(super) fn refresh_find_for(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(state) = self.find.get_mut(&tab.id) else {
            return;
        };
        if state.open {
            state.search(&tab.editor.text());
            tab.editor
                .set_find_matches(state.matches.clone(), state.current);
        } else {
            tab.editor.set_find_matches(Vec::new(), None);
        }
    }

    /// Selects the active tab's current match in the editor, and repaints
    /// the match coloring so the newly-current one stands out.
    pub(super) fn select_current_match(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(state) = self.find.get(&tab.id) else {
            return;
        };
        // Tint only while the palette is showing. With it closed - a bare
        // find-again - the editor holds focus, so the ordinary selection
        // marks the match on its own.
        if state.open {
            tab.editor
                .set_find_matches(state.matches.clone(), state.current);
        } else {
            tab.editor.set_find_matches(Vec::new(), None);
        }
        let Some(found) = state.current_match() else {
            return;
        };
        // Cursor at the end of the match so typing continues past it, the
        // same shape a drag-selection leaves. `move_to` underneath marks the
        // cursor moved, which scrolls an off-screen match into view.
        tab.editor.restore_selection(
            SavedSelection {
                anchor: (found.line, found.start),
                kind: SelectionKind::Range,
            },
            (found.line, found.end),
        );
    }

    /// The active tab's find state, if it has any.
    pub(super) fn active_find(&self) -> Option<&FindState> {
        self.find.get(&self.tabs.get(self.active)?.id)
    }
}
