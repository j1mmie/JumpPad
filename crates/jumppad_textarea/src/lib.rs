mod comment;
mod drag_scroll;
pub mod font;
mod history;
mod indent;
mod line_edit;
mod lines;
mod safe_area;
mod scrollbar;
mod text_delta;
pub mod text_editor;
mod word;

pub use comment::CommentStyle;
pub use indent::{Indentation, IndentationStyle};
pub use word::WordSeparators;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use editor_core::{
    EditorMessage, FindMatch, SCROLLBAR_THUMB_WASH, SavedSelection,
    SelectionKind, TextEditorWidget, scrollbar_wash,
};
use history::{CursorState, History};
use iced::advanced::text::Highlighter;
use iced::advanced::text::highlighter::Format;
use iced::{Background, Border, Color, Element, Fill, Font, Theme};
use jumppad_actions::Action;
use line_edit::EditedLines;
use syntax_registry::{
    Grammar, Handle, HighlightCategory, PollResult, SyntaxRegistry,
};
use text_delta::TextDelta;
use text_editor::{
    Binding, Content, Cursor, Direction, Motion, Position, Status, text_editor,
};

/// Re-exported so the app can write the [`KeyResolver`] it hands down without
/// naming the widget module.
pub use text_editor::KeyPress;

/// Turns a key press into the [`Action`] it asks for, if any.
///
/// Supplied by the app rather than built here: resolving a press means
/// knowing both the default chords (`jumppad_keybinds`) and the user's
/// `keybinds.toml` overrides (`jumppad_config`), and this crate depends on
/// neither - the same reason `build_editor_overrides` already lived in the
/// app. Handing down a resolver keeps that boundary and puts
/// override-beats-default precedence in one place instead of two.
pub type KeyResolver = dyn Fn(&KeyPress) -> Option<Action> + Send + Sync;

/// Editor settings the app can change after construction: a config reload
/// writes here, and every open [`TextArea`] reads through a shared handle
/// on each `view`, which is what lets a reload reach tabs that already
/// exist. One per app, created alongside [`TextArea::factory`].
pub struct SharedEditorConfig {
    /// `f32` bits - an atomic can't hold a float directly.
    background_alpha: AtomicU32,
    /// `f32` bits, same reason. Range-checked by the widget's
    /// `scroll_sensitivity` builder, not here.
    scroll_sensitivity: AtomicU32,
    /// `f32` bits, as above. Range-checked by the widget's `drag_speed`
    /// builder.
    drag_speed: AtomicU32,
    /// Clamped by `History::set_depth`, not here.
    undo_depth: AtomicUsize,
    /// The typeface documents are drawn in. Behind a lock rather than in an
    /// atomic because a [`Font`] is four fields wide; swappable because a
    /// `config.toml` reload has to reach tabs that already exist.
    font: RwLock<Font>,
    /// `f32` bits, as above. Clamped by [`font::clamp_size`] on the way in.
    font_size: AtomicU32,
    /// `Arc` inside the lock so `view` clones a refcount out per redraw,
    /// not the whole resolver. Swappable because a `keybinds.toml` reload
    /// has to reach tabs that already exist.
    resolver: RwLock<Arc<KeyResolver>>,
    /// Extension -> comment style, flattened from the config by the app.
    /// Keys are lowercase; look up lowercased.
    comment_styles: RwLock<Arc<HashMap<String, CommentStyle>>>,
    /// What Tab inserts and how wide a tab draws. Behind a lock rather than
    /// in an atomic because it is two fields, like [`Self::font`]; read once
    /// per view and once per Tab press, neither of them hot.
    indentation: RwLock<Indentation>,
    /// What ends a word, for the word motions and the double click. Behind a
    /// lock for the same reason [`Self::font`] is - it is a string, not a
    /// number - and read once per word action, which is one keystroke or one
    /// click.
    word_separators: RwLock<WordSeparators>,
}

impl SharedEditorConfig {
    pub fn new(background_alpha: f32, resolver: Arc<KeyResolver>) -> Arc<Self> {
        Arc::new(Self {
            background_alpha: AtomicU32::new(
                background_alpha.clamp(0.0, 1.0).to_bits(),
            ),
            scroll_sensitivity: AtomicU32::new(1.0f32.to_bits()),
            drag_speed: AtomicU32::new(1.0f32.to_bits()),
            undo_depth: AtomicUsize::new(history::DEFAULT_DEPTH),
            font: RwLock::new(Font::MONOSPACE),
            font_size: AtomicU32::new(font::DEFAULT_SIZE.to_bits()),
            resolver: RwLock::new(resolver),
            comment_styles: RwLock::new(Arc::new(HashMap::new())),
            indentation: RwLock::new(Indentation::default()),
            word_separators: RwLock::new(WordSeparators::default()),
        })
    }

    pub fn background_alpha(&self) -> f32 {
        f32::from_bits(self.background_alpha.load(Ordering::Relaxed))
    }

    pub fn resolver(&self) -> Arc<KeyResolver> {
        self.resolver.read().unwrap().clone()
    }

    pub fn set_background_alpha(&self, alpha: f32) {
        self.background_alpha
            .store(alpha.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The multiplier on wheel and trackpad scroll distance. `1.0` is the
    /// shipped speed; the widget clamps whatever lands here to a usable
    /// range, so a nonsense `config.toml` value can't disable scrolling.
    pub fn scroll_sensitivity(&self) -> f32 {
        f32::from_bits(self.scroll_sensitivity.load(Ordering::Relaxed))
    }

    pub fn set_scroll_sensitivity(&self, sensitivity: f32) {
        self.scroll_sensitivity
            .store(sensitivity.to_bits(), Ordering::Relaxed);
    }

    /// The multiplier on how fast a selection drag held past the top or
    /// bottom edge walks the view. `1.0` is the shipped speed; the widget
    /// clamps whatever lands here to the same usable range as the wheel's.
    pub fn drag_speed(&self) -> f32 {
        f32::from_bits(self.drag_speed.load(Ordering::Relaxed))
    }

    pub fn set_drag_speed(&self, speed: f32) {
        self.drag_speed.store(speed.to_bits(), Ordering::Relaxed);
    }

    /// Undo steps per tab. A step is a burst of typing, not a keystroke.
    pub fn undo_depth(&self) -> usize {
        self.undo_depth.load(Ordering::Relaxed)
    }

    pub fn set_undo_depth(&self, depth: usize) {
        self.undo_depth.store(depth, Ordering::Relaxed);
    }

    /// The typeface documents are drawn in. `Font::MONOSPACE` - the
    /// system's own monospace face - until a config names another.
    pub fn font(&self) -> Font {
        *self.font.read().unwrap()
    }

    pub fn set_font(&self, font: Font) {
        *self.font.write().unwrap() = font;
    }

    /// The height of the editor's text in pixels.
    pub fn font_size(&self) -> f32 {
        f32::from_bits(self.font_size.load(Ordering::Relaxed))
    }

    pub fn set_font_size(&self, size: f32) {
        self.font_size
            .store(font::clamp_size(size).to_bits(), Ordering::Relaxed);
    }

    /// Routed through here so settings have one mutation API, but stored in
    /// `FOREGROUND_ALPHA` - see that static for why it's global.
    pub fn set_foreground_alpha(&self, alpha: f32) {
        FOREGROUND_ALPHA
            .store(alpha.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_resolver(&self, resolver: Arc<KeyResolver>) {
        *self.resolver.write().unwrap() = resolver;
    }

    pub fn comment_styles(&self) -> Arc<HashMap<String, CommentStyle>> {
        self.comment_styles.read().unwrap().clone()
    }

    pub fn set_comment_styles(&self, styles: HashMap<String, CommentStyle>) {
        *self.comment_styles.write().unwrap() = Arc::new(styles);
    }

    /// What one press of Tab inserts, and the width every tab in every open
    /// document is drawn at. Tabs at four columns until a config says
    /// otherwise.
    pub fn indentation(&self) -> Indentation {
        *self.indentation.read().unwrap()
    }

    /// Range-checked by [`Indentation::new`], the way a font size is by
    /// [`font::clamp_size`] - so a nonsense `config.toml` width can't reach
    /// the buffer that draws the tabs.
    pub fn set_indentation(&self, indentation: Indentation) {
        *self.indentation.write().unwrap() = indentation;
    }

    /// The characters a word ends at, for the word motions and the double
    /// click. VS Code's list until a config names another.
    pub fn word_separators(&self) -> WordSeparators {
        self.word_separators.read().unwrap().clone()
    }

    pub fn set_word_separators(&self, separators: WordSeparators) {
        *self.word_separators.write().unwrap() = separators;
    }
}

/// A [`TextEditorWidget`] backed by this crate's forked [`text_editor`], with
/// optional tree-sitter/WASM syntax highlighting layered on via a
/// [`syntax_registry::SyntaxRegistry`].
pub struct TextArea {
    content: Content,
    /// The document's full text, rebuilt only when an edit changes it -
    /// never on a redraw. `Content::text()` reassembles the whole document
    /// line by line, and `view` runs on every redraw, including ones caused
    /// by nothing but a mouse move mid-drag: ~18ms for a 150K-line file in
    /// release, past a whole 60fps frame budget on its own.
    ///
    /// `Arc` so handing it to [`HighlighterSettings`] is a refcount bump,
    /// and so that struct's `PartialEq` can compare pointers, not bytes.
    source: Arc<String>,
    /// Where the last change starts, or `None` if it could have recolored
    /// anything. Tells the highlighter which line to resume at.
    edited_from: Option<usize>,
    highlighting: Highlighting,
    /// Read for its load revision on every `view` - an injection target
    /// resolving is otherwise invisible to iced, which re-runs the
    /// highlighter only when `HighlighterSettings` compare unequal.
    registry: Arc<SyntaxRegistry>,
    history: History,
    settings: Arc<SharedEditorConfig>,
    /// The file extension this tab was opened from, lowercased - what
    /// toggle-comment resolves its prefix by.
    extension: Option<String>,
    /// Find-palette matches to recolor, and which one is current. `Arc` so
    /// rebuilding `HighlighterSettings` on every `view` is a refcount bump
    /// rather than a copy of the whole match list.
    find_matches: Arc<Vec<FindMatch>>,
    find_current: Option<usize>,
    /// Where the double click that started a word selection landed, while
    /// one is live. It is what a drag out of that word measures from, so the
    /// selection goes on taking whole words at both ends; anything else that
    /// moves the caret clears it.
    word_drag_from: Option<Position>,
}

/// Scales every syntax-highlighted color `color_for` produces. Global
/// rather than a field on [`SharedEditorConfig`] because
/// `text_editor::highlight_with`'s `to_format` callback must be a bare `fn`
/// pointer - it can't capture state, so `color_for` has nothing else to
/// read this from. `f32` bits; written via
/// [`SharedEditorConfig::set_foreground_alpha`], including on config reload.
static FOREGROUND_ALPHA: AtomicU32 = AtomicU32::new(f32::to_bits(1.0));

fn foreground_alpha() -> f32 {
    f32::from_bits(FOREGROUND_ALPHA.load(Ordering::Relaxed))
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
    fn resync_source(&mut self) {
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
    fn cursor_state(&self) -> CursorState {
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
    const LINES_WORTH_SPLICING: usize = 500;

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
    fn replace_document(&mut self, text: &str) {
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
    fn extend_word_selection(&mut self) {
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
    fn covered_text(&self, (first, last): (usize, usize)) -> Vec<String> {
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
    fn splice_lines_in_place(
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
    fn paste_over(
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
    fn highlighter_settings(&self) -> HighlighterSettings {
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
        text_editor(&self.content)
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
            .style(move |theme, status| {
                editor_style(theme, status, background_alpha, foreground_alpha)
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

/// Clamps a saved (line, column) to the nearest valid position, in case the
/// document has since gotten shorter - `Content::move_to` does no bounds
/// checking of its own. `column` is a byte index within the line (matching
/// what `Content::cursor` reports), so it's also backed up to a `char`
/// boundary.
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

fn clamp_position(
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

/// The settings a [`TreeSitterHighlighter`] is built/updated from: the
/// grammar to highlight with (if any) and the entire current document text -
/// `highlight_line` only sees one line at a time, but tree-sitter needs the
/// whole buffer to parse.
#[derive(Clone)]
struct HighlighterSettings {
    /// Shared with the [`TextArea`] this was built from - see its `source`
    /// field. Cloning these settings (which the widget does on every layout
    /// that changes them) copies a refcount, not the document.
    source: Arc<String>,
    grammar: Option<Arc<Grammar>>,
    /// The registry's load revision. The grammar `Arc` stays the same object
    /// as its injection targets resolve, so without this the settings would
    /// still compare equal and the incomplete first parse would stay on
    /// screen until an unrelated edit changed `source`.
    revision: u64,
    matches: Arc<Vec<FindMatch>>,
    current_match: Option<usize>,
    /// Not read by the highlighter itself - `to_format` resolves colors
    /// through `FOREGROUND_ALPHA` directly. It rides along so a config
    /// reload makes the settings compare unequal (see `PartialEq` below).
    foreground_alpha: f32,
    /// Outside `PartialEq`: it describes the change, not the state.
    edited_from: Option<usize>,
}

impl HighlighterSettings {
    /// Whether the text is the only thing that moved. Anything else can
    /// recolor a line no edit went near.
    fn only_the_text_moved_since(&self, previous: &Self) -> bool {
        !Arc::ptr_eq(&self.source, &previous.source)
            && self.revision == previous.revision
            && self.foreground_alpha == previous.foreground_alpha
            && self.current_match == previous.current_match
            && Arc::ptr_eq(&self.matches, &previous.matches)
            && match (&self.grammar, &previous.grammar) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl PartialEq for HighlighterSettings {
    fn eq(&self, other: &Self) -> bool {
        // Pointer equality, not a byte compare: `TextArea` mints a new `Arc`
        // exactly when the text changes, so a shared pointer already means
        // "same text" - and this runs per redraw, where comparing a
        // multi-megabyte string is the cost the cache exists to remove. The
        // failure directions are asymmetric, which is what makes that safe:
        // two equal-but-separate allocations cost one redundant reparse,
        // where a missed change would leave stale colors on screen.
        Arc::ptr_eq(&self.source, &other.source)
            && self.revision == other.revision
            && match (&self.grammar, &other.grammar) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            // Comparing the find state matters as much as the source: iced
            // only re-runs the highlighter when these settings change, so
            // leaving it out here would freeze the match coloring.
            && self.current_match == other.current_match
            && Arc::ptr_eq(&self.matches, &other.matches)
            // Same for the foreground alpha: without it, a config reload
            // would leave every syntax-colored span at its old alpha until
            // an unrelated edit changed `source`.
            && self.foreground_alpha == other.foreground_alpha
    }
}

/// What a highlighted range is: ordinary syntax, or a find-palette match.
/// Local to this crate rather than a new `HighlightCategory` variant -
/// `syntax_registry` describes grammars and has no business knowing the
/// editor has a find feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Highlighted {
    Syntax(HighlightCategory),
    Match,
    CurrentMatch,
}

struct TreeSitterHighlighter {
    spans: Arc<Vec<syntax_registry::HighlightSpan>>,
    /// Byte offset of the start of each line within `source`, in order.
    line_starts: Vec<usize>,
    current_line: usize,
    matches: Arc<Vec<FindMatch>>,
    current_match: Option<usize>,
    /// What the last update was built from.
    previous: Option<HighlighterSettings>,
}

impl TreeSitterHighlighter {
    /// The first line iced has to recolor; everything above keeps its
    /// colors. Only an edit narrows this - see `only_the_text_moved_since`.
    fn resume_line(
        &self,
        settings: &HighlighterSettings,
        spans: &[syntax_registry::HighlightSpan],
    ) -> usize {
        let Some(previous) = &self.previous else {
            return 0;
        };
        if !settings.only_the_text_moved_since(previous) {
            return 0;
        }
        let edited_from = settings.edited_from.unwrap_or(0);
        match first_recolored_byte(&self.spans, spans) {
            Some(byte) => edited_from.min(self.line_at(byte)),
            None => edited_from,
        }
    }

    fn line_at(&self, byte: usize) -> usize {
        self.line_starts
            .partition_point(|start| *start <= byte)
            .saturating_sub(1)
    }
}

/// The first byte whose coloring could have moved. A `*/` closes a comment
/// opened far above, so this can't be read off the edit's own position.
fn first_recolored_byte(
    before: &[syntax_registry::HighlightSpan],
    after: &[syntax_registry::HighlightSpan],
) -> Option<usize> {
    for (was, now) in before.iter().zip(after) {
        if was != now {
            return Some(was.start.min(now.start));
        }
    }
    // One parse ran longer; they part at the first span past the shorter.
    before
        .get(after.len())
        .or_else(|| after.get(before.len()))
        .map(|span| span.start)
}

impl Highlighter for TreeSitterHighlighter {
    type Settings = HighlighterSettings;
    type Highlight = Highlighted;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Highlighted)>;

    fn new(settings: &Self::Settings) -> Self {
        let mut highlighter = Self {
            spans: Arc::new(Vec::new()),
            line_starts: vec![0],
            current_line: 0,
            matches: Arc::new(Vec::new()),
            current_match: None,
            previous: None,
        };
        highlighter.update(settings);
        highlighter
    }

    fn update(&mut self, settings: &Self::Settings) {
        let spans = match &settings.grammar {
            Some(grammar) => grammar.highlight(&settings.source),
            None => Arc::new(Vec::new()),
        };
        self.line_starts = line_starts(&settings.source);
        self.current_line = self.resume_line(settings, &spans);
        self.spans = spans;
        self.matches = settings.matches.clone();
        self.current_match = settings.current_match;
        self.previous = Some(settings.clone());
    }

    fn change_line(&mut self, line: usize) {
        // Lowest wins - iced knows where its edit landed, not how far up
        // the reparse reaches.
        self.current_line = self.current_line.min(line);
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        let line_index = self.current_line;
        self.current_line += 1;

        let Some(&start) = self.line_starts.get(line_index) else {
            return Vec::new().into_iter();
        };
        let end = start + line.len();

        // Spans are ordered and never overlap, so the ones on this line are
        // one contiguous run. Scanning them all per line cost lines x spans.
        let from = self
            .spans
            .partition_point(|span| span.start < start)
            .saturating_sub(1);
        let mut ranges: Vec<(Range<usize>, Highlighted)> = self.spans[from..]
            .iter()
            .take_while(|span| span.start < end)
            .filter(|span| span.end > start)
            .map(|span| {
                let range_start = span.start.max(start) - start;
                let range_end = span.end.min(end) - start;
                (range_start..range_end, Highlighted::Syntax(span.category))
            })
            .collect();

        // After the syntax spans on purpose: the last span covering a byte
        // wins, and match coloring has to outrank syntax coloring. Columns
        // are already line-relative. Matches group by line, so same search.
        let first_match = self
            .matches
            .partition_point(|found| found.line < line_index);
        ranges.extend(
            self.matches[first_match..]
                .iter()
                .enumerate()
                .take_while(|(_, found)| found.line == line_index)
                .map(|(offset, found)| {
                    let kind =
                        if Some(first_match + offset) == self.current_match {
                            Highlighted::CurrentMatch
                        } else {
                            Highlighted::Match
                        };
                    (found.start..found.end, kind)
                }),
        );

        ranges.into_iter()
    }

    fn current_line(&self) -> usize {
        self.current_line
    }
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn to_format(highlighted: &Highlighted, _theme: &Theme) -> Format<Font> {
    Format {
        color: Some(color_for(*highlighted)),
        font: None,
    }
}

fn color_for(highlighted: Highlighted) -> iced::Color {
    apply_alpha(base_color_for(highlighted), foreground_alpha())
}

/// Find matches are recolored rather than given a highlight box: iced's
/// `highlighter::Format` carries only a color and a font, with no background
/// to fill. The current match is the brighter of the two so it stands out
/// from its neighbours.
const MATCH_COLOR: iced::Color = iced::Color::from_rgb(0.85, 0.62, 0.24);
const CURRENT_MATCH_COLOR: iced::Color = iced::Color::from_rgb(1.0, 0.85, 0.35);

/// Scales `color`'s alpha by `alpha`, skipping the multiply at `1.0`.
fn apply_alpha(color: iced::Color, alpha: f32) -> iced::Color {
    if alpha >= 1.0 {
        color
    } else {
        color.scale_alpha(alpha)
    }
}

fn base_color_for(highlighted: Highlighted) -> iced::Color {
    let category = match highlighted {
        Highlighted::Match => return MATCH_COLOR,
        Highlighted::CurrentMatch => return CURRENT_MATCH_COLOR,
        Highlighted::Syntax(category) => category,
    };
    match category {
        HighlightCategory::String => iced::Color::from_rgb8(152, 195, 121),
        HighlightCategory::Comment => iced::Color::from_rgb8(140, 140, 140),
        HighlightCategory::Number => iced::Color::from_rgb8(209, 154, 102),
        HighlightCategory::Keyword => iced::Color::from_rgb8(97, 175, 239),
        HighlightCategory::Heading => iced::Color::from_rgb8(224, 108, 117),
        HighlightCategory::Emphasis => iced::Color::from_rgb8(198, 120, 221),
        HighlightCategory::Link => iced::Color::from_rgb8(86, 182, 194),
        HighlightCategory::Quote => iced::Color::from_rgb8(130, 140, 155),
        HighlightCategory::Code => iced::Color::from_rgb8(229, 192, 123),
    }
}

fn word_delete_backward() -> Binding<EditorMessage> {
    Binding::Sequence(vec![
        Binding::Select(Motion::Left.widen()),
        Binding::Backspace,
    ])
}

fn word_delete_forward() -> Binding<EditorMessage> {
    Binding::Sequence(vec![
        Binding::Select(Motion::Right.widen()),
        Binding::Delete,
    ])
}

/// How this crate performs an [`Action`], or `None` for one it doesn't own -
/// every `Action::App`, which the shell handles instead.
///
/// `jumppad`'s wiring test asserts every action is claimed by exactly one of
/// this and its own `message_for`, so an action added to the registry and
/// forgotten here fails the build rather than going quiet.
pub fn binding_for(action: Action) -> Option<Binding<EditorMessage>> {
    let custom = |message| Some(Binding::Custom(message));
    match action {
        Action::WordDeleteBackward => Some(word_delete_backward()),
        Action::WordDeleteForward => Some(word_delete_forward()),
        Action::DocumentStart => Some(Binding::Move(Motion::DocumentStart)),
        Action::SelectDocumentStart => {
            Some(Binding::Select(Motion::DocumentStart))
        }
        Action::DocumentEnd => Some(Binding::Move(Motion::DocumentEnd)),
        Action::SelectDocumentEnd => Some(Binding::Select(Motion::DocumentEnd)),
        Action::Undo => custom(EditorMessage::Undo),
        Action::Redo => custom(EditorMessage::Redo),
        Action::Indent => custom(EditorMessage::Indent),
        Action::Outdent => custom(EditorMessage::Outdent),
        Action::ToggleComment => custom(EditorMessage::ToggleComment),
        Action::DeleteLine => custom(EditorMessage::DeleteLine),
        Action::MoveLineUp => custom(EditorMessage::MoveLineUp),
        Action::MoveLineDown => custom(EditorMessage::MoveLineDown),
        Action::CopyLineUp => custom(EditorMessage::CopyLineUp),
        Action::CopyLineDown => custom(EditorMessage::CopyLineDown),
        _ => None,
    }
}

/// Turns a key press into a binding.
///
/// Two tiers, first match wins:
/// 1. `resolve` - the app's key -> [`Action`] map, which already merges the
///    user's `keybinds.toml` overrides over the default chords. An action
///    this crate doesn't perform falls through rather than swallowing the
///    press, so the shell still sees its own shortcuts.
/// 2. iced's own stock default dispatch.
fn key_binding(
    press: KeyPress,
    resolve: &KeyResolver,
) -> Option<Binding<EditorMessage>> {
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }

    if let Some(binding) = resolve(&press).and_then(binding_for) {
        return Some(binding);
    }

    // Tier 2: iced's own stock dispatch - with one correction. On macOS,
    // holding Cmd doesn't suppress character production, so an
    // unrecognized Cmd+<letter> would otherwise get typed into the
    // document *and* mark the event captured, hiding it from app-level
    // shortcuts. Discard an `Insert` produced while `command()` is held so
    // it falls through unhandled instead (a no-op on other platforms,
    // where `command()` is Ctrl and already suppresses character production).
    let command_held = press.modifiers.command();
    match Binding::from_key_press(press) {
        Some(Binding::Insert(_)) if command_held => None,
        other => other,
    }
}

/// iced's default `text_editor` style draws a border that changes color on
/// hover/focus - dropped here so there's no color-change effect to notice.
/// Also drops its background on a transparent window and scales the base text
/// color by `foreground_alpha` (both plain parameters, for testability -
/// syntax-highlighted text instead goes through `color_for`).
fn editor_style(
    theme: &Theme,
    status: text_editor::Status,
    background_alpha: f32,
    foreground_alpha: f32,
) -> text_editor::Style {
    let default = text_editor::default(theme, status);
    // The window background is already this exact color; repainting it
    // translucent would just stack a second layer (see AGENTS.md's
    // hairline-seam gotcha).
    let background = if background_alpha >= 1.0 {
        default.background
    } else {
        Background::Color(Color::TRANSPARENT)
    };
    let value = apply_alpha(default.value, foreground_alpha);
    text_editor::Style {
        border: Border {
            width: 0.0,
            ..Border::default()
        },
        background,
        value,
        scrollbar_thumb: scrollbar_thumb_style(theme),
        ..default
    }
}

/// The scrollbar thumb's fill: a wash toward white on a dark theme, toward
/// black on a light one (see `editor_core::scrollbar_wash`), so the thumb
/// always reads as a step away from the document without needing a border to
/// stay visible on dark themes.
pub fn scrollbar_thumb_style(theme: &Theme) -> Color {
    scrollbar_wash(theme, SCROLLBAR_THUMB_WASH)
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
