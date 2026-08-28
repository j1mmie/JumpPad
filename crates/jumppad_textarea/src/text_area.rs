use std::ops::Range;
use std::sync::Arc;

use editor_core::{
    EditorMessage, FindMatch, SavedSelection, SelectionKind, TextEditorWidget,
};
use iced::{Element, Fill};
use syntax_registry::{Grammar, Handle, PollResult, SyntaxRegistry};

use crate::comment::{self, CommentStyle};
use crate::highlighter::{HighlighterSettings, TreeSitterHighlighter, to_format};
use crate::history::{CursorState, History};
use crate::keybindings::key_binding;
use crate::line_edit::{self, EditedLines};
use crate::lines;
use crate::shared_config::{SharedEditorConfig, foreground_alpha};
use crate::style::editor_style;
use crate::text_delta::TextDelta;
use crate::text_editor::{self, Content, Cursor, Direction, Motion, Position};

/// A [`TextEditorWidget`] backed by this crate's forked [`text_editor`], with
/// optional tree-sitter/WASM syntax highlighting layered on via a
/// [`syntax_registry::SyntaxRegistry`].
pub struct TextArea {
    pub(crate) content: Content,
    /// The document's full text, rebuilt only when an edit changes it -
    /// never on a redraw. `Content::text()` reassembles the whole document
    /// line by line, and `view` runs on every redraw, including ones caused
    /// by nothing but a mouse move mid-drag: ~18ms for a 150K-line file in
    /// release, past a whole 60fps frame budget on its own.
    ///
    /// `Arc` so handing it to [`HighlighterSettings`] is a refcount bump,
    /// and so that struct's `PartialEq` can compare pointers, not bytes.
    pub(crate) source: Arc<String>,
    /// Where the last change starts, or `None` if it could have recolored
    /// anything. Tells the highlighter which line to resume at.
    pub(crate) edited_from: Option<usize>,
    highlighting: Highlighting,
    /// Read for its load revision on every `view` - an injection target
    /// resolving is otherwise invisible to iced, which re-runs the
    /// highlighter only when `HighlighterSettings` compare unequal.
    registry: Arc<SyntaxRegistry>,
    pub(crate) history: History,
    pub(crate) settings: Arc<SharedEditorConfig>,
    /// The file extension this tab was opened from, lowercased - what
    /// toggle-comment resolves its prefix by.
    extension: Option<String>,
    /// Find-palette matches to recolor, and which one is current. `Arc` so
    /// rebuilding `HighlighterSettings` on every `view` is a refcount bump
    /// rather than a copy of the whole match list.
    pub(crate) find_matches: Arc<Vec<FindMatch>>,
    pub(crate) find_current: Option<usize>,
    /// Where the double click that started a word selection landed, while
    /// one is live. It is what a drag out of that word measures from, so the
    /// selection goes on taking whole words at both ends; anything else that
    /// moves the caret clears it.
    pub(crate) word_drag_from: Option<Position>,
}

enum Highlighting {
    /// No file extension is known for this tab - never highlighted.
    None,
    /// A grammar load was requested; still waiting on it.
    Pending(Handle),
    /// Grammar loaded and ready to use. The `Handle` must stay alive here,
    /// not just while `Pending` - dropping it would evict the grammar.
    Ready(#[allow(dead_code)] Handle, Arc<Grammar>),
    /// Grammar search finished with nothing usable. Still holds the
    /// `Handle` so a second tab with the same extension reuses this result.
    Unavailable(#[allow(dead_code)] Handle),
}

impl TextArea {
    pub fn new(
        text: &str,
        registry: &Arc<SyntaxRegistry>,
        extension: Option<&str>,
        settings: Arc<SharedEditorConfig>,
    ) -> Self {
        let highlighting = match extension {
            None => Highlighting::None,
            Some(ext) => Highlighting::Pending(registry.acquire(ext)),
        };
        let content = Content::with_text(text);
        Self {
            // Seeded from `content`, not from `text`, so the cache is
            // byte-identical to what `Content::text()` would have produced -
            // the highlighter's byte offsets are resolved against the lines
            // `Content` actually holds, and any normalization difference
            // between the two would misalign every span after it.
            source: Arc::new(content.text()),
            content,
            edited_from: None,
            highlighting,
            registry: registry.clone(),
            history: History::new(),
            settings,
            extension: extension.map(str::to_lowercase),
            find_matches: Arc::new(Vec::new()),
            find_current: None,
            word_drag_from: None,
        }
    }

    /// Builds an [`editor_core::EditorFactory`]-shaped closure that captures
    /// a shared syntax registry and the app's live [`SharedEditorConfig`].
    pub fn factory(
        registry: Arc<SyntaxRegistry>,
        settings: Arc<SharedEditorConfig>,
    ) -> impl Fn(&str, Option<&str>) -> Box<dyn TextEditorWidget> {
        move |text, extension| {
            Box::new(Self::new(text, &registry, extension, settings.clone()))
        }
    }

    /// Rebuilds the [`source`](Self::source) cache from `content`. Call after
    /// anything that changes the document's text, and only then: the fresh
    /// `Arc` is what tells the highlighter its input moved, so a needless
    /// call costs a full reparse and a missing one leaves it parsing stale
    /// text.
    pub(crate) fn resync_source(&mut self) {
        self.source = Arc::new(self.content.text());
    }

    /// A hint only - iced reports its own topmost changed line afterwards,
    /// and the lower of the two wins.
    fn topmost_touched_line(&self) -> usize {
        let caret = self.content.caret_line();
        match self.selection() {
            Some(saved) => caret.min(saved.anchor.0),
            None => caret,
        }
    }

    /// The caret state to hand `History`: position plus whatever is selected,
    /// since undoing an edit that replaced a selection has to put it back.
    pub(crate) fn cursor_state(&self) -> CursorState {
        CursorState {
            position: self.cursor_position(),
            selection: self.selection(),
        }
    }

    fn grammar(&self) -> Option<Arc<Grammar>> {
        match &self.highlighting {
            Highlighting::Ready(_, grammar) => Some(grammar.clone()),
            _ => None,
        }
    }

    /// Shared by `EditorMessage::Undo`/`Redo`. Returns whether an edit
    /// happened, same contract `update` has for `Action`s.
    fn apply_history(
        &mut self,
        op: impl FnOnce(
            &mut History,
            &Arc<String>,
            CursorState,
        ) -> Option<(TextDelta, CursorState)>,
    ) -> bool {
        let current = self.cursor_state();
        let Some((delta, restored)) =
            op(&mut self.history, &self.source, current)
        else {
            return false;
        };
        let caret_was = self.apply_delta(&delta);
        // `move_cursor_to` clears a selection, so a restored one needs the
        // other branch. Neither changes text.
        match restored.selection {
            Some(selection) => {
                self.restore_selection(selection, restored.position)
            }
            None => {
                self.move_cursor_to(restored.position.0, restored.position.1)
            }
        }
        // Last, so nothing overwrites the record the next layout reads.
        if let Some(caret_was) = caret_was {
            self.content.reveal_caret_from(caret_was);
        }
        true
    }

    /// Past this many lines a rebuild is cheaper: pasting is quadratic in
    /// what it pastes. Measured - see AGENTS.md.
    pub(crate) const LINES_WORTH_SPLICING: usize = 500;

    /// Replays a delta in place. Returns the caret's starting line for the
    /// reveal that follows, or `None` if it rebuilt instead.
    fn apply_delta(&mut self, delta: &TextDelta) -> Option<usize> {
        if delta.line_count() > Self::LINES_WORTH_SPLICING {
            let replaced = self.source_with(delta);
            self.replace_document(&replaced);
            return None;
        }
        let caret_was = self.content.caret_line(); // the splice moves it
        let span = delta.source_range();
        self.paste_over(
            position_of(&self.source, span.start),
            position_of(&self.source, span.end),
            delta.replacement().to_owned(),
        );
        self.splice_source(delta);
        self.edited_from = Some(delta.first_line());
        Some(caret_was)
    }

    /// One copy of the document, against the per-line reassembly
    /// `Content::text()` would cost.
    fn source_with(&self, delta: &TextDelta) -> String {
        let mut replaced = self.source.as_str().to_owned();
        replaced.replace_range(delta.source_range(), delta.replacement());
        replaced
    }

    /// `resync_source` for paths that already know what moved.
    fn splice_source(&mut self, delta: &TextDelta) {
        self.source = Arc::new(self.source_with(delta));
    }

    /// Replaces the whole document, carrying the view across - a rebuilt
    /// `Content` starts at the top. Never SelectAll+Paste (see AGENTS.md).
    pub(crate) fn replace_document(&mut self, text: &str) {
        let before = self.content.capture_view();
        self.content = Content::with_text(text);
        self.content.restore_view(before);
        self.resync_source();
        self.edited_from = None;
    }

    /// The comment style configured for this tab's file type.
    fn comment_style(&self) -> Option<CommentStyle> {
        let extension = self.extension.as_deref()?;
        self.settings.comment_styles().get(extension).cloned()
    }

    /// The ending this document separates lines with - the stand-in wherever
    /// a spliced-in line needs one and the line it displaced had none. Only
    /// a single-line document has no separator to copy.
    fn document_line_ending(&self) -> text_editor::LineEnding {
        match self.content.line_ending() {
            Some(text_editor::LineEnding::None) | None => {
                text_editor::LineEnding::default()
            }
            Some(ending) => ending,
        }
    }

    /// Performs one editor action, keeping the source cache, the
    /// highlighter's resume line and the undo history in step with it.
    ///
    /// Only an `Edit` changes the document's text. `Move`, `Select`, `Click`,
    /// `Drag` and `Scroll` just move the cursor or the viewport, so the
    /// `source` cache stays valid across all of them - which is what keeps a
    /// selection drag, the whole reason the cache exists, off the rebuild
    /// path.
    fn perform_action(&mut self, action: text_editor::Action) -> bool {
        let is_edit = action.is_edit();
        let ends_step = ends_undo_step(&action);
        // Before the action moves the caret, and not on a drag.
        let touched_before = is_edit.then(|| self.topmost_touched_line());
        if is_edit {
            // The pre-edit text is already sitting in the cache. The caret
            // goes with it so undo can re-select whatever this edit is about
            // to replace.
            self.history
                .record_before_edit(&self.source, self.cursor_state());
        }
        self.apply_action(action);
        if let Some(touched_before) = touched_before {
            self.resync_source();
            self.edited_from =
                Some(touched_before.min(self.topmost_touched_line()));
        }
        if ends_step {
            self.history.end_burst();
        }
        is_edit
    }

    /// Hands one action to the document, answering the word-boundary ones
    /// here rather than letting them reach the buffer.
    ///
    /// cosmic-text decides where a word ends by Unicode segmentation and
    /// takes no argument on the subject, so `[words] separators` can only
    /// apply from outside it: what a word motion or a double click asks for
    /// is worked out in `word.rs` and arrives as a cursor position.
    fn apply_action(&mut self, action: text_editor::Action) {
        use text_editor::Action;

        // A word drag lives from the double click that starts it until
        // anything else moves the caret. A shift+click arrives as a `Drag`
        // of its own, so it extends the selection by whole words - which is
        // what VS Code and a browser both do after a double click.
        if !matches!(action, Action::SelectWord | Action::Drag(_)) {
            self.word_drag_from = None;
        }
        match action {
            Action::SelectWord => self.select_word(),
            Action::Drag(at) if self.word_drag_from.is_some() => {
                self.content.perform(Action::Drag(at));
                self.extend_word_selection();
            }
            Action::Move(motion @ (Motion::WordLeft | Motion::WordRight)) => {
                // A motion over a selection only collapses it onto the edge
                // it is heading for, whatever the motion was - so the
                // buffer's own answer is right here, and its idea of a word
                // never comes into it.
                if self.content.cursor().selection.is_some() {
                    self.content.perform(Action::Move(motion));
                } else {
                    let target = self.word_target(motion.direction());
                    self.set_cursor(target, None);
                }
            }
            Action::Select(motion @ (Motion::WordLeft | Motion::WordRight)) => {
                let cursor = self.content.cursor();
                let anchor = cursor.selection.unwrap_or(cursor.position);
                let target = self.word_target(motion.direction());
                self.set_cursor(target, Some(anchor));
            }
            action => self.content.perform(action),
        }
    }

    /// The word around the caret, selected outright - a double click.
    ///
    /// The click that placed the caret has already been performed by the
    /// time this arrives, so the caret is standing exactly where the pointer
    /// is and the run around it is what the pointer picked out.
    fn select_word(&mut self) {
        let position = self.content.cursor().position;
        self.word_drag_from = Some(position);
        let (start, end) = self.run_around(position);
        self.set_cursor(end, Some(start));
    }

    /// Takes a word drag out to whole runs at both ends, from wherever the
    /// drag has just left the caret. The end the pointer is on moves; the
    /// other stays wrapped around the word the double click landed in.
    pub(crate) fn extend_word_selection(&mut self) {
        let Some(origin) = self.word_drag_from else {
            return;
        };
        // The document can have been rebuilt since the double click, and a
        // position past the end of it would slice a line in half.
        let origin =
            clamp_position(&self.content, (origin.line, origin.column));
        let position = self.content.cursor().position;
        let (origin_start, origin_end) = self.run_around(origin);
        let (start, end) = self.run_around(position);
        if (position.line, position.column) < (origin.line, origin.column) {
            self.set_cursor(start, Some(origin_end));
        } else {
            self.set_cursor(end, Some(origin_start));
        }
    }

    /// The run of like characters around a position: the word a double click
    /// takes, or the punctuation or the whitespace it landed in instead.
    fn run_around(&self, position: Position) -> (Position, Position) {
        let text = self.line_text(position.line).unwrap_or_default();
        let run = self
            .settings
            .word_separators()
            .run_around(&text, position.column);
        let at = |column| Position {
            line: position.line,
            column,
        };
        (at(run.start), at(run.end))
    }

    /// Where one word motion lands from wherever the caret is: the start of
    /// the word behind it, or the end of the word ahead of it - and the line
    /// above or below once this one has run out, which is the only way
    /// either motion leaves the line it started on.
    fn word_target(&self, direction: Direction) -> Position {
        let position = self.content.cursor().position;
        let text = self.line_text(position.line).unwrap_or_default();
        let separators = self.settings.word_separators();
        match direction {
            Direction::Left if position.column == 0 => {
                match position.line.checked_sub(1) {
                    Some(line) => Position {
                        line,
                        column: self.line_text(line).unwrap_or_default().len(),
                    },
                    // The start of the document: nowhere left to go.
                    None => position,
                }
            }
            Direction::Left => Position {
                column: separators.start_before(&text, position.column),
                ..position
            },
            Direction::Right if position.column >= text.len() => {
                if position.line + 1 < self.content.line_count() {
                    Position {
                        line: position.line + 1,
                        column: 0,
                    }
                } else {
                    position
                }
            }
            Direction::Right => Position {
                column: separators.end_after(&text, position.column),
                ..position
            },
        }
    }

    /// Puts the caret at `position`, with `anchor` as the other end of a
    /// selection, or with nothing selected at all.
    ///
    /// The `Move`s do two things `Content::move_to` cannot, and where they
    /// land doesn't matter since `move_to` overwrites it. They drop a
    /// selection still standing - `move_to` can set one but never clear one.
    /// And the one that actually *moves* clears the column an Up or Down is
    /// aiming for, which a caret that has just been moved sideways has no
    /// business keeping: without it, word-left followed by Down lands on the
    /// column before the word motion rather than after it. A `Move` over a
    /// selection only collapses it, so reaching the motion can take two.
    fn set_cursor(&mut self, position: Position, anchor: Option<Position>) {
        let anchor = anchor.filter(|anchor| *anchor != position);
        if self.content.cursor().selection.is_some() {
            self.content
                .perform(text_editor::Action::Move(Motion::Right));
        }
        self.content
            .perform(text_editor::Action::Move(Motion::Right));
        self.content.move_to(Cursor {
            position,
            selection: anchor,
        });
    }

    /// Indents: one more level on every line of a selection that reaches
    /// across a line ending, and otherwise one indent typed at the caret.
    fn indent(&mut self) -> bool {
        let Some((first, last)) = self.block_to_indent() else {
            return self.type_indent();
        };
        let indentation = self.settings.indentation();
        self.transform_lines(first, last, |covered| {
            indentation.indent_lines(covered)
        })
    }

    /// Takes one indent level back off every covered line. A block already
    /// flush against the left margin is a clean no-op.
    fn outdent(&mut self) -> bool {
        let (first, last) = self.covered();
        let indentation = self.settings.indentation();
        self.transform_lines(first, last, |covered| {
            indentation.outdent_lines(covered)
        })
    }

    /// The lines Tab indents as a block, or `None` when it types an indent at
    /// the caret instead.
    ///
    /// The dividing line is whether the selection holds a line ending: one
    /// that does cannot be replaced by an indent without joining the lines it
    /// spans, which is nobody's idea of what Tab does.
    fn block_to_indent(&self) -> Option<(usize, usize)> {
        let spans_lines = match self.selection()? {
            SavedSelection {
                kind: SelectionKind::Line,
                ..
            } => true,
            selection => selection.anchor.0 != self.cursor_position().0,
        };
        spans_lines.then(|| self.covered())
    }

    /// Inserts one indent over any selection: a tab character, or the spaces
    /// that reach the next stop from where the caret is drawn. Always an
    /// edit - the narrowest indent is still one character - so there is no
    /// no-op case to guard.
    fn type_indent(&mut self) -> bool {
        let indentation = self.settings.indentation();
        let (line, column) = self.indent_origin();
        let line_text = self.line_text(line).unwrap_or_default();
        let text =
            indentation.text_at(indentation.visual_column(&line_text, column));
        let changed = self.perform_action(text_editor::Action::Edit(
            text_editor::Edit::Paste(Arc::new(text)),
        ));
        // An indent closes a typing burst, the way inserting any other
        // whitespace already does. `Edit::Paste` doesn't on its own, and
        // teaching it to would change what the clipboard does too.
        self.history.end_burst();
        changed
    }

    /// Where an indent lands: the start of the selection it replaces, or the
    /// caret. A word or line selection reports its anchor rather than its
    /// start, so this reads a column or two late for one - worth a space or
    /// two in spaces mode, and nothing at all in tabs mode.
    fn indent_origin(&self) -> (usize, usize) {
        let cursor = self.cursor_position();
        match self.selection() {
            Some(selection) => selection.anchor.min(cursor),
            None => cursor,
        }
    }

    /// Comments or uncomments the covered lines with the file type's
    /// configured prefix; no style or all-blank coverage is a clean no-op.
    fn toggle_comment(&mut self) -> bool {
        let Some(style) = self.comment_style() else {
            return false;
        };
        let (first, last) = self.covered();
        self.transform_lines(first, last, |covered| {
            comment::toggle_comment(covered, &style)
        })
    }

    /// Rewrites lines `first..=last` with whatever `transform` makes of them,
    /// carrying the caret and any selection across the columns it moved. A
    /// transform with nothing to do is a clean no-op.
    fn transform_lines(
        &mut self,
        first: usize,
        last: usize,
        transform: impl FnOnce(&[&str]) -> Option<EditedLines>,
    ) -> bool {
        let covered = self.covered_text((first, last));
        let covered: Vec<&str> = covered.iter().map(String::as_str).collect();
        let Some(edited) = transform(&covered) else {
            return false;
        };

        let cursor = self.cursor_position();
        let selection = self.selection();
        // Its own undo step - a line transform shouldn't fold into a typing
        // burst - recorded only now that an edit is certain to happen.
        self.history
            .record_isolated(&self.source, self.cursor_state());
        let caret_was = self.content.caret_line(); // the splice moves it
        self.splice_lines_in_place(first..last + 1, &edited.lines);
        self.resync_source();
        self.edited_from = Some(first);

        let shift = |pos| line_edit::shift_position(pos, first, &edited.edits);
        match selection {
            Some(saved) => self.restore_selection(
                SavedSelection {
                    anchor: shift(saved.anchor),
                    ..saved
                },
                shift(cursor),
            ),
            None => {
                let (line, column) = shift(cursor);
                self.move_cursor_to(line, column);
            }
        }
        // Last, so nothing overwrites the record the next layout reads.
        self.content.reveal_caret_from(caret_was);
        true
    }

    /// The covered lines' text, endings dropped - what the line transforms take.
    pub(crate) fn covered_text(&self, (first, last): (usize, usize)) -> Vec<String> {
        (first..=last)
            .filter_map(|index| {
                self.content.line(index).map(|line| line.text.into_owned())
            })
            .collect()
    }

    /// The inclusive line range the caret or selection covers right now.
    fn covered(&self) -> (usize, usize) {
        lines::covered_lines(self.cursor_position(), self.selection())
    }

    /// Replaces the lines in `replaced` with `replacements`, in place, by
    /// selecting exactly that span and pasting over it.
    ///
    /// The alternative - splice the text, rebuild the whole `Content` - costs
    /// a full re-shape of the document on every keystroke: 2.16s against
    /// 1.09ms for this, measured on 20K lines, because a fresh buffer is laid
    /// out from placeholder metrics and re-shaped end to end. A line command
    /// touches two or three lines and should cost two or three lines.
    ///
    /// The endings are the fiddly part, and the rule matches what the rebuild
    /// did: each replacement inherits the ending of the line it stands in for,
    /// falling back to the document's own. The span reaches to the *start* of
    /// the line past the block, so it swallows one ending per line replaced -
    /// which is why every replacement gets one back. A block running to the
    /// end of the document has no ending to swallow on its last line, and
    /// deleting one takes the newline in front of it instead, or it would
    /// leave a blank line behind.
    pub(crate) fn splice_lines_in_place(
        &mut self,
        replaced: Range<usize>,
        replacements: &[String],
    ) {
        let line_count = self.content.line_count();
        let start = replaced.start.min(line_count);
        let end = replaced.end.clamp(start, line_count);
        let at_document_end = end >= line_count;

        let displaced: Vec<text_editor::LineEnding> = (start..end)
            .filter_map(|index| {
                self.content.line(index).map(|line| line.ending)
            })
            .collect();
        let fallback = self.document_line_ending();

        let mut text = String::new();
        for (offset, replacement) in replacements.iter().enumerate() {
            text.push_str(replacement);
            if at_document_end && offset + 1 == replacements.len() {
                continue;
            }
            let ending = match displaced.get(offset).copied() {
                Some(text_editor::LineEnding::None) | None => fallback,
                Some(ending) => ending,
            };
            text.push_str(ending.as_str());
        }

        let (anchor, caret) = match (at_document_end, replacements.is_empty()) {
            (false, _) => ((start, 0), (end, 0)),
            // Nothing is going back in and there is no ending to the right to
            // absorb, so the one to the left goes with it.
            (true, true) if start > 0 => {
                ((start - 1, self.line_end(start - 1)), self.document_end())
            }
            (true, _) => ((start, 0), self.document_end()),
        };

        self.paste_over(anchor, caret, text);
    }

    /// Selects a span and pastes over it. A change touching three lines
    /// should cost three lines, not a rebuild of the document.
    pub(crate) fn paste_over(
        &mut self,
        anchor: (usize, usize),
        caret: (usize, usize),
        text: String,
    ) {
        self.content.move_to(Cursor {
            position: clamp_position(&self.content, caret),
            selection: Some(clamp_position(&self.content, anchor)),
        });
        self.content.perform(text_editor::Action::Edit(
            text_editor::Edit::Paste(Arc::new(text)),
        ));
    }

    /// Byte length of a line's text, its ending excluded.
    fn line_end(&self, line: usize) -> usize {
        self.content.line(line).map_or(0, |line| line.text.len())
    }

    /// The last position in the document.
    fn document_end(&self) -> (usize, usize) {
        let last = self.content.line_count().saturating_sub(1);
        (last, self.line_end(last))
    }

    /// Writes a line command out: one isolated undo step - a line command
    /// joins neither the typing burst before it nor the keystroke after -
    /// then the lines spliced in place and the caret put where the command
    /// left it. Callers bail before here when there is nothing to do.
    fn apply_line_splice(&mut self, splice: lines::LineSplice) -> bool {
        let before = self.cursor_state();
        // Where the caret starts, for the reveal at the bottom - read before
        // the splice selects anything, since that moves the caret itself.
        let caret_was = self.content.caret_line();
        self.history.record_isolated(&self.source, before);
        let touched_from = splice.replaced.start;
        self.splice_lines_in_place(splice.replaced, &splice.lines);
        self.resync_source();
        self.edited_from = Some(touched_from);

        match splice.caret {
            lines::Caret::Collapsed(line) => {
                self.move_cursor_to(line, before.position.1)
            }
            lines::Caret::Shifted(rows) => {
                let shift = |(line, column): (usize, usize)| {
                    (line.saturating_add_signed(rows), column)
                };
                match before.selection {
                    Some(saved) => self.restore_selection(
                        SavedSelection {
                            anchor: shift(saved.anchor),
                            ..saved
                        },
                        shift(before.position),
                    ),
                    None => {
                        let (line, column) = shift(before.position);
                        self.move_cursor_to(line, column);
                    }
                }
            }
        }
        // Last: the caret is where the command leaves it, and nothing after
        // this overwrites the record the next layout reads.
        self.content.reveal_caret_from(caret_was);
        true
    }

    /// Removes the covered lines. Only an already-empty document has nothing
    /// to remove - dropping any line elsewhere always drops bytes.
    fn delete_line(&mut self) -> bool {
        if self.source.is_empty() {
            return false;
        }
        self.apply_line_splice(lines::delete(self.covered()))
    }

    /// Swaps the covered lines with the line above them; a clean no-op at the
    /// top of the document, where there is nothing to swap with.
    fn move_line_up(&mut self) -> bool {
        let covered = self.covered();
        let Some(above) = covered
            .0
            .checked_sub(1)
            .and_then(|index| self.line_text(index))
        else {
            return false;
        };
        self.apply_line_splice(lines::move_up(
            covered,
            above,
            self.covered_text(covered),
        ))
    }

    /// Mirror of [`Self::move_line_up`], no-op at the end of the document.
    fn move_line_down(&mut self) -> bool {
        let covered = self.covered();
        let Some(below) = self.line_text(covered.1 + 1) else {
            return false;
        };
        self.apply_line_splice(lines::move_down(
            covered,
            below,
            self.covered_text(covered),
        ))
    }

    /// Duplicates the covered lines. Always an edit - a copy adds at least a
    /// line separator - so there is no no-op case to guard.
    fn copy_line(&mut self, downward: bool) -> bool {
        let covered = self.covered();
        self.apply_line_splice(lines::copy(
            covered,
            self.covered_text(covered),
            downward,
        ))
    }

    fn line_text(&self, index: usize) -> Option<String> {
        self.content.line(index).map(|line| line.text.into_owned())
    }

    /// The [`HighlighterSettings`] describing this editor's current state.
    /// Built fresh on every `view` and compared against the previous frame's
    /// copy in the widget's `layout`, so this and its `PartialEq` both sit on
    /// the per-redraw path.
    pub(crate) fn highlighter_settings(&self) -> HighlighterSettings {
        HighlighterSettings {
            source: self.source.clone(),
            grammar: self.grammar(),
            revision: self.registry.revision(),
            matches: self.find_matches.clone(),
            current_match: self.find_current,
            foreground_alpha: foreground_alpha(),
            edited_from: self.edited_from,
        }
    }
}

impl TextEditorWidget for TextArea {
    fn view(&self) -> Element<'_, EditorMessage> {
        let settings = self.highlighter_settings();
        let resolve = self.settings.resolver();
        let background_alpha = self.settings.background_alpha();
        let foreground_alpha = foreground_alpha();
        let line_numbers_alpha = self.settings.line_numbers_alpha();
        text_editor::text_editor(&self.content)
            .id(iced::advanced::widget::Id::new(
                editor_core::EDITOR_WIDGET_ID,
            ))
            .placeholder("")
            .font(self.settings.font())
            .size(self.settings.font_size())
            .height(Fill)
            .scroll_sensitivity(self.settings.scroll_sensitivity())
            .drag_speed(self.settings.drag_speed())
            .tab_width(self.settings.indentation().width())
            .line_numbers(self.settings.line_numbers())
            .style(move |theme, status| {
                editor_style(
                    theme,
                    status,
                    background_alpha,
                    foreground_alpha,
                    line_numbers_alpha,
                )
            })
            .highlight_with::<TreeSitterHighlighter>(settings, to_format)
            .key_binding(move |press| key_binding(press, resolve.as_ref()))
            .on_action(EditorMessage::Action)
            .on_scroll(EditorMessage::Scroll)
            .into()
    }

    fn update(&mut self, message: EditorMessage) -> bool {
        // Live, so a config reload reaches tabs that already exist.
        self.history.set_depth(self.settings.undo_depth());
        match message {
            EditorMessage::Action(action) => self.perform_action(action),
            EditorMessage::Scroll(pixels) => {
                // Moves the view, never the text - so no `source` rebuild,
                // for the same reason `Scroll`'s action counterpart needs
                // none.
                self.content.scroll_by(pixels);
                false
            }
            EditorMessage::Undo => self.apply_history(History::undo),
            EditorMessage::Redo => self.apply_history(History::redo),
            EditorMessage::Indent => self.indent(),
            EditorMessage::Outdent => self.outdent(),
            EditorMessage::ToggleComment => self.toggle_comment(),
            EditorMessage::DeleteLine => self.delete_line(),
            EditorMessage::MoveLineUp => self.move_line_up(),
            EditorMessage::MoveLineDown => self.move_line_down(),
            EditorMessage::CopyLineUp => self.copy_line(false),
            EditorMessage::CopyLineDown => self.copy_line(true),
        }
    }

    fn text(&self) -> String {
        self.source.as_str().to_owned()
    }

    fn set_text(&mut self, text: &str) {
        self.content = Content::with_text(text);
        self.resync_source();
    }

    fn reload_text(&mut self, text: &str) {
        // A stamp can move without the bytes moving - `touch`, a checkout
        // that restores identical content. Recording that would push an undo
        // step that undoes nothing and, worse, clear the redo stack.
        if text == self.source.as_str() {
            return;
        }
        // Isolated for the same reason a comment toggle is: the reload joins
        // neither the typing burst before it nor the keystroke after it.
        self.history.set_depth(self.settings.undo_depth());
        self.history
            .record_isolated(&self.source, self.cursor_state());
        let cursor = self.cursor_position();
        self.replace_document(text);
        self.move_cursor_to(cursor.0, cursor.1); // clamps if the file shrank
    }

    fn poll_highlighting(&mut self) {
        if !matches!(self.highlighting, Highlighting::Pending(_)) {
            return;
        }
        let Highlighting::Pending(handle) =
            std::mem::replace(&mut self.highlighting, Highlighting::None)
        else {
            unreachable!("just checked self.highlighting is Pending");
        };
        self.highlighting = match handle.poll() {
            PollResult::Ready(grammar) => Highlighting::Ready(handle, grammar),
            PollResult::Unavailable => Highlighting::Unavailable(handle),
            PollResult::Loading => Highlighting::Pending(handle),
        };
    }

    fn has_pending_highlighting(&self) -> bool {
        match &self.highlighting {
            Highlighting::Pending(_) => true,
            // A loaded grammar is not necessarily done: its injection
            // targets (markdown's inline grammar, say) load separately and
            // later, and the spans it yields until then are missing
            // everything they would have colored.
            Highlighting::Ready(_, grammar) => grammar.injections_unresolved(),
            Highlighting::None | Highlighting::Unavailable(_) => false,
        }
    }

    fn cursor_position(&self) -> (usize, usize) {
        let position = self.content.cursor().position;
        (position.line, position.column)
    }

    fn move_cursor_to(&mut self, line: usize, column: usize) {
        let position = clamp_position(&self.content, (line, column));
        self.set_cursor(position, None);
    }

    fn set_find_matches(
        &mut self,
        matches: Vec<FindMatch>,
        current: Option<usize>,
    ) {
        // A fresh `Arc` tells the highlighter its input changed, and the app
        // pushes an empty list here after every edit while find is closed.
        if self.find_matches.as_slice() != matches.as_slice() {
            self.find_matches = Arc::new(matches);
        }
        self.find_current = current;
    }

    fn selection(&self) -> Option<SavedSelection> {
        let cursor = self.content.cursor();
        let anchor = cursor.selection?;
        let anchor_position = (anchor.line, anchor.column);
        if anchor != cursor.position {
            return Some(SavedSelection {
                anchor: anchor_position,
                kind: SelectionKind::Range,
            });
        }
        // Anchor == cursor: either a leftover collapsed range (not a real
        // selection) or the line a triple click took, whose bounds live in
        // the kind rather than in the cursor pair. Whether anything is
        // actually selected tells the two apart. A double click needs no
        // case of its own - it makes a plain range like any other, since the
        // word it takes is measured here rather than by the buffer.
        if self.content.selection().is_none_or(|text| text.is_empty()) {
            return None;
        }
        Some(SavedSelection {
            anchor: anchor_position,
            kind: SelectionKind::Line,
        })
    }

    fn restore_selection(
        &mut self,
        selection: SavedSelection,
        cursor: (usize, usize),
    ) {
        let anchor = clamp_position(&self.content, selection.anchor);
        match selection.kind {
            SelectionKind::Range => {
                let position = clamp_position(&self.content, cursor);
                self.set_cursor(position, Some(anchor));
            }
            SelectionKind::Line => {
                self.set_cursor(anchor, None);
                self.content.perform(text_editor::Action::SelectLine);
            }
        }
    }
}

/// What closes an undo step on top of the coalescing timer: finishing a
/// word, and moving the caret somewhere else. The whitespace itself rides
/// with the word it follows, so undo takes back `hello ` rather than `hello`.
fn ends_undo_step(action: &text_editor::Action) -> bool {
    use text_editor::{Action, Edit};
    match action {
        Action::Edit(Edit::Enter) => true,
        Action::Edit(Edit::Insert(character)) => character.is_whitespace(),
        Action::Move(_)
        | Action::Select(_)
        | Action::SelectWord
        | Action::SelectLine
        | Action::SelectAll
        | Action::Click(_)
        | Action::Drag(_) => true,
        _ => false,
    }
}

/// The `(line, column)` a byte offset falls on. Columns are byte offsets
/// within their own line, which is what `Content` positions take.
fn position_of(source: &str, offset: usize) -> (usize, usize) {
    let head = &source.as_bytes()[..offset.min(source.len())];
    let line = head.iter().filter(|byte| **byte == b'\n').count();
    let line_start = head
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |n| n + 1);
    (line, offset - line_start)
}

pub(crate) fn clamp_position(
    content: &Content,
    (line, column): (usize, usize),
) -> Position {
    let line = line.min(content.line_count().saturating_sub(1));
    let text = content
        .line(line)
        .map(|l| l.text.into_owned())
        .unwrap_or_default();
    let mut column = column.min(text.len());
    while !text.is_char_boundary(column) {
        column -= 1;
    }
    Position { line, column }
}
