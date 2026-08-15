mod comment;
pub mod font;
mod history;
mod lines;
mod safe_area;
mod scrollbar;
mod text_delta;
pub mod text_editor;

pub use comment::CommentStyle;

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
use syntax_registry::{
    Grammar, Handle, HighlightCategory, PollResult, SyntaxRegistry,
};
use text_delta::TextDelta;
use text_editor::{
    Binding, Content, Cursor, Motion, Position, Status, text_editor,
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
}

impl SharedEditorConfig {
    pub fn new(background_alpha: f32, resolver: Arc<KeyResolver>) -> Arc<Self> {
        Arc::new(Self {
            background_alpha: AtomicU32::new(
                background_alpha.clamp(0.0, 1.0).to_bits(),
            ),
            scroll_sensitivity: AtomicU32::new(1.0f32.to_bits()),
            undo_depth: AtomicUsize::new(history::DEFAULT_DEPTH),
            font: RwLock::new(Font::MONOSPACE),
            font_size: AtomicU32::new(font::DEFAULT_SIZE.to_bits()),
            resolver: RwLock::new(resolver),
            comment_styles: RwLock::new(Arc::new(HashMap::new())),
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

    /// Comments or uncomments the covered lines with the file type's
    /// configured prefix; no style or all-blank coverage is a clean no-op.
    fn toggle_comment(&mut self) -> bool {
        let Some(style) = self.comment_style() else {
            return false;
        };
        let cursor = self.cursor_position();
        let selection = self.selection();
        let (first, last) = lines::covered_lines(cursor, selection);
        let covered = self.covered_text((first, last));
        let covered: Vec<&str> = covered.iter().map(String::as_str).collect();
        let Some(toggled) = comment::toggle_comment(&covered, &style) else {
            return false;
        };

        // Its own undo step - a toggle shouldn't fold into a typing burst -
        // recorded only now that an edit is certain to happen.
        self.history
            .record_isolated(&self.source, self.cursor_state());
        let caret_was = self.content.caret_line(); // the splice moves it
        self.splice_lines_in_place(first..last + 1, &toggled.lines);
        self.resync_source();
        self.edited_from = Some(first);

        let shift = |pos| comment::shift_position(pos, first, &toggled.edits);
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
            EditorMessage::Action(action) => {
                // Only an `Edit` changes the document's text. `Move`,
                // `Select`, `Click`, `Drag` and `Scroll` just move the cursor
                // or the viewport, so the `source` cache stays valid across
                // all of them - which is what keeps a selection drag, the
                // whole reason the cache exists, off the rebuild path.
                let is_edit = action.is_edit();
                let ends_step = ends_undo_step(&action);
                // Before the action moves the caret, and not on a drag.
                let touched_before =
                    is_edit.then(|| self.topmost_touched_line());
                if is_edit {
                    // The pre-edit text is already sitting in the cache. The
                    // caret goes with it so undo can re-select whatever this
                    // edit is about to replace.
                    self.history
                        .record_before_edit(&self.source, self.cursor_state());
                }
                self.content.perform(action);
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
            EditorMessage::Scroll(pixels) => {
                // Moves the view, never the text - so no `source` rebuild,
                // for the same reason `Scroll`'s action counterpart needs
                // none.
                self.content.scroll_by(pixels);
                false
            }
            EditorMessage::Undo => self.apply_history(History::undo),
            EditorMessage::Redo => self.apply_history(History::redo),
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
        // `move_to` with no selection leaves an existing one in place, so a
        // stale selection has to be dropped explicitly first. Any non-edge
        // `Move` collapses it (the cursor lands wherever `move_to` says next).
        if self.content.cursor().selection.is_some() {
            self.content
                .perform(text_editor::Action::Move(Motion::Right));
        }
        let position = clamp_position(&self.content, (line, column));
        self.content.move_to(Cursor {
            position,
            selection: None,
        });
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
        // selection) or a word/line selection from a double/triple click,
        // whose bounds live in the selection kind rather than the cursor
        // pair. The selected text tells the cases apart - and which kind.
        let selected =
            self.content.selection().filter(|text| !text.is_empty())?;
        let line = self.content.line(anchor.line)?;
        let kind =
            if selected.trim_end_matches(['\r', '\n']) == line.text.as_ref() {
                SelectionKind::Line
            } else {
                SelectionKind::Word
            };
        Some(SavedSelection {
            anchor: anchor_position,
            kind,
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
                self.content.move_to(Cursor {
                    position: clamp_position(&self.content, cursor),
                    selection: Some(anchor),
                });
            }
            SelectionKind::Word => {
                self.content.move_to(Cursor {
                    position: anchor,
                    selection: None,
                });
                self.content.perform(text_editor::Action::SelectWord);
            }
            SelectionKind::Line => {
                self.content.move_to(Cursor {
                    position: anchor,
                    selection: None,
                });
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
mod tests {
    use iced::keyboard::{self, key};

    use super::*;

    fn press(
        modifiers: keyboard::Modifiers,
        physical: key::Code,
        key: keyboard::Key,
    ) -> KeyPress {
        press_with_text(modifiers, physical, key, None)
    }

    fn press_with_text(
        modifiers: keyboard::Modifiers,
        physical: key::Code,
        key: keyboard::Key,
        text: Option<&str>,
    ) -> KeyPress {
        KeyPress {
            key: key.clone(),
            modified_key: key,
            physical_key: key::Physical::Code(physical),
            modifiers,
            text: text.map(Into::into),
            status: Status::Focused { is_hovered: false },
        }
    }

    /// An editor with no highlighting and default alpha, for cursor/selection tests.
    fn plain_editor(text: &str) -> TextArea {
        let registry = SyntaxRegistry::new(Vec::new(), HashMap::new(), || {});
        TextArea::new(
            text,
            &registry,
            None,
            SharedEditorConfig::new(1.0, Arc::new(|_: &KeyPress| None)),
        )
    }

    /// The invariant the `source` cache exists to maintain: it holds exactly
    /// what rebuilding from `Content` would have produced. Everything below
    /// that mutates an editor ends by checking this - a cache that drifts
    /// feeds the highlighter stale text, which misaligns every span after
    /// the point where it diverged.
    fn assert_source_is_synced(editor: &TextArea) {
        assert_eq!(
            editor.source.as_str(),
            editor.content.text(),
            "the cached source drifted from the document"
        );
    }

    #[test]
    fn a_new_editor_starts_with_a_synced_source() {
        // Covers the line-ending shapes where seeding the cache from the
        // input `&str` rather than from `Content` could have diverged.
        for doc in
            ["", "one line", "trailing\n", "a\nb\nc", "crlf\r\nlines\r\n"]
        {
            let editor = plain_editor(doc);
            assert_source_is_synced(&editor);
            assert_eq!(editor.text(), editor.content.text(), "doc: {doc:?}");
        }
    }

    #[test]
    fn redraws_reuse_the_cached_source_instead_of_rebuilding_it() {
        // `view` runs per redraw; rebuilding the document string there is
        // the cost this cache removes, so consecutive builds must hand out
        // the same allocation and compare equal (no highlighter re-run).
        let editor = plain_editor("fn main() {}\n");
        let first = editor.highlighter_settings();
        let second = editor.highlighter_settings();
        assert!(
            Arc::ptr_eq(&first.source, &second.source),
            "a redraw must not rebuild the document string"
        );
        assert!(
            first == second,
            "unchanged settings must not re-run the highlighter"
        );
    }

    #[test]
    fn a_selection_drag_does_not_invalidate_the_cached_source() {
        // The regression this cache exists for: dragging a selection emits a
        // stream of non-edit actions, each one causing a redraw. None of
        // them change the text, so none may rebuild the source.
        let mut editor = plain_editor("hello world\nsecond line");
        let before = editor.source.clone();

        for action in [
            text_editor::Action::Click(iced::Point::new(4.0, 2.0)),
            text_editor::Action::Drag(iced::Point::new(30.0, 2.0)),
            text_editor::Action::Drag(iced::Point::new(60.0, 14.0)),
            text_editor::Action::Move(Motion::Right),
            text_editor::Action::Select(Motion::Down),
            text_editor::Action::SelectWord,
            text_editor::Action::SelectLine,
            text_editor::Action::SelectAll,
            text_editor::Action::Scroll { lines: 3 },
        ] {
            let edited = editor.update(EditorMessage::Action(action.clone()));
            assert!(!edited, "{action:?} is not an edit");
            assert!(
                Arc::ptr_eq(&before, &editor.source),
                "{action:?} must not rebuild the cached source"
            );
        }
        assert_source_is_synced(&editor);
    }

    #[test]
    fn a_pixel_scroll_is_not_an_edit_and_does_not_rebuild_the_source() {
        // Same contract as the non-edit actions above: `Scroll` moves the
        // view, never the text, so the highlighter must not be re-run for it.
        // It arrives on its own message rather than as a `text_editor::Action`
        // because `Action::Scroll` can't carry a fractional distance.
        let mut editor = plain_editor("hello world\nsecond line");
        let before = editor.source.clone();

        let edited = editor.update(EditorMessage::Scroll(7.5));

        assert!(!edited, "a scroll is not an edit");
        assert!(
            Arc::ptr_eq(&before, &editor.source),
            "a scroll must not rebuild the cached source"
        );
        assert_source_is_synced(&editor);
    }

    #[test]
    fn an_edit_rebuilds_the_cached_source() {
        let mut editor = plain_editor("hello");
        editor.move_cursor_to(0, 5);
        let before = editor.source.clone();

        let edited = editor.update(EditorMessage::Action(
            text_editor::Action::Edit(text_editor::Edit::Insert('!')),
        ));

        assert!(edited);
        assert!(
            !Arc::ptr_eq(&before, &editor.source),
            "an edit must mint a new source so the highlighter re-runs"
        );
        assert_eq!(editor.source.as_str(), "hello!");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn set_text_rebuilds_the_cached_source() {
        let mut editor = plain_editor("original");
        editor.set_text("replaced\nwith more");
        assert_eq!(editor.text(), "replaced\nwith more");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn undo_and_redo_keep_the_cached_source_in_sync() {
        let mut editor = plain_editor("hello");
        editor.move_cursor_to(0, 5);
        editor.update(EditorMessage::Action(text_editor::Action::Edit(
            text_editor::Edit::Insert('!'),
        )));
        assert_eq!(editor.text(), "hello!");

        // Undo restores the pre-edit text, which `update` read out of the
        // cache - so a stale cache would record the wrong snapshot here.
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "hello");
        assert_source_is_synced(&editor);

        assert!(editor.update(EditorMessage::Redo));
        assert_eq!(editor.text(), "hello!");
        assert_source_is_synced(&editor);
    }

    /// Types `text` a character at a time, the way a person would.
    fn type_out(editor: &mut TextArea, text: &str) {
        for character in text.chars() {
            let action = if character == '\n' {
                text_editor::Edit::Enter
            } else {
                text_editor::Edit::Insert(character)
            };
            editor.update(edit(action));
        }
    }

    #[test]
    fn each_typed_word_is_its_own_undo_step() {
        let mut editor = plain_editor("");
        type_out(&mut editor, "Hello my name is Jimmie");

        // The space rides with the word it follows.
        for expected in [
            "Hello my name is ",
            "Hello my name ",
            "Hello my ",
            "Hello ",
            "",
        ] {
            assert!(editor.update(EditorMessage::Undo), "{expected:?}");
            assert_eq!(editor.text(), expected);
            assert_source_is_synced(&editor);
        }
        assert!(!editor.update(EditorMessage::Undo));
    }

    #[test]
    fn redo_walks_the_words_back_in() {
        let mut editor = plain_editor("");
        type_out(&mut editor, "one two three");
        while editor.update(EditorMessage::Undo) {}
        assert_eq!(editor.text(), "");

        for expected in ["one ", "one two ", "one two three"] {
            assert!(editor.update(EditorMessage::Redo), "{expected:?}");
            assert_eq!(editor.text(), expected);
            assert_source_is_synced(&editor);
        }
    }

    #[test]
    fn enter_ends_an_undo_step() {
        let mut editor = plain_editor("");
        type_out(&mut editor, "first\nsecond");
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "first\n");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn moving_the_caret_ends_an_undo_step() {
        // Typing here, clicking there, then typing again is two edits, not
        // one - without this they fold into a single step.
        let mut editor = plain_editor("ab");
        editor.move_cursor_to(0, 2);
        editor.update(edit(text_editor::Edit::Insert('X')));
        editor.update(EditorMessage::Action(text_editor::Action::Move(
            text_editor::Motion::Home,
        )));
        editor.update(edit(text_editor::Edit::Insert('Y')));
        assert_eq!(editor.text(), "YabX");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "abX");
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "ab");
    }

    #[test]
    fn a_run_of_deletes_still_coalesces_on_the_timer() {
        // Backspace has no word boundary to break on, so the timer is what
        // keeps a held key from becoming one enormous step.
        let mut editor = plain_editor("abcdef");
        editor.move_cursor_to(0, 6);
        for _ in 0..3 {
            editor.update(edit(text_editor::Edit::Backspace));
        }
        assert_eq!(editor.text(), "abc");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "abcdef");
        assert!(!editor.update(EditorMessage::Undo));
    }

    #[test]
    fn a_typed_word_stores_only_the_word() {
        // With word-sized steps, a delta that rounded out to whole lines
        // would carry the growing line on every keystroke.
        let mut editor = plain_editor("Hello ");
        editor.move_cursor_to(0, 6);
        type_out(&mut editor, "my ");

        let (delta, _) = editor
            .history
            .undo(&editor.source, editor.cursor_state())
            .expect("a word to undo");
        assert_eq!(delta.replacement(), "");
        assert_eq!(delta.source_range(), 6..9);
    }

    #[test]
    fn undo_and_redo_round_trip_byte_exactly() {
        // Every shape where endings are load-bearing: the last line, which
        // carries none, and CRLF, which is two bytes.
        let cases: &[(&str, (usize, usize), text_editor::Edit)] = &[
            ("a\nb\nc", (1, 1), text_editor::Edit::Insert('X')),
            ("a\nb\nc", (0, 0), text_editor::Edit::Insert('X')),
            ("a\nb\nc", (2, 1), text_editor::Edit::Insert('X')),
            ("a\nb\nc", (1, 1), text_editor::Edit::Enter),
            ("a\nb\nc", (2, 1), text_editor::Edit::Enter),
            ("a\nb\nc", (2, 1), text_editor::Edit::Backspace),
            ("a\nb\nc", (2, 0), text_editor::Edit::Backspace),
            ("a\r\nb\r\nc", (1, 1), text_editor::Edit::Insert('X')),
            ("a\r\nb\r\nc", (2, 1), text_editor::Edit::Enter),
            ("a\r\nb\r\nc", (1, 0), text_editor::Edit::Backspace),
            ("a\r\nb\r\nc", (2, 1), text_editor::Edit::Backspace),
            ("", (0, 0), text_editor::Edit::Insert('X')),
            ("trailing\n", (1, 0), text_editor::Edit::Insert('X')),
            ("one line", (0, 3), text_editor::Edit::Insert('X')),
        ];

        for (doc, cursor, action) in cases {
            let what = format!("{doc:?} at {cursor:?} {action:?}");
            let mut editor = plain_editor(doc);
            editor.move_cursor_to(cursor.0, cursor.1);
            assert!(editor.update(edit(action.clone())), "no edit: {what}");
            let edited = editor.text();
            assert_source_is_synced(&editor);

            assert!(editor.update(EditorMessage::Undo), "no undo: {what}");
            assert_eq!(editor.text(), *doc, "undo: {what}");
            assert_source_is_synced(&editor);

            assert!(editor.update(EditorMessage::Redo), "no redo: {what}");
            assert_eq!(editor.text(), edited, "redo: {what}");
            assert_source_is_synced(&editor);
        }
    }

    #[test]
    fn undo_carries_only_the_lines_the_edit_touched() {
        // The whole point: an undo costs the lines it changes.
        let doc: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut editor = plain_editor(&doc);
        editor.move_cursor_to(30, 4);
        editor.update(edit(text_editor::Edit::Insert('X')));
        assert!(editor.update(EditorMessage::Undo));

        assert_eq!(editor.text(), doc);
        assert_source_is_synced(&editor);
        // The highlighter resumes at the change, not at the top.
        assert_eq!(editor.highlighter_settings().edited_from, Some(30));
    }

    #[test]
    fn an_edit_reports_the_topmost_line_it_touched() {
        // Recoloring starts at the selection's anchor, not the caret.
        let mut editor = plain_editor("a\nb\nc\nd\ne");
        editor.restore_selection(
            SavedSelection {
                anchor: (1, 0),
                kind: SelectionKind::Range,
            },
            (3, 1),
        );
        editor.update(edit(text_editor::Edit::Insert('X')));
        assert_eq!(editor.highlighter_settings().edited_from, Some(1));
    }

    #[test]
    fn a_whole_document_undo_gives_way_to_a_rebuild() {
        // Past `LINES_WORTH_SPLICING` the splice stands down, but the text
        // still has to come back.
        let long: String = (0..TextArea::LINES_WORTH_SPLICING + 10)
            .map(|i| format!("line {i}\n"))
            .collect();
        let mut editor = plain_editor("short");
        editor.reload_text(&long);
        assert_eq!(editor.text(), long);

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "short");
        assert_source_is_synced(&editor);
        assert_eq!(editor.highlighter_settings().edited_from, None);

        assert!(editor.update(EditorMessage::Redo));
        assert_eq!(editor.text(), long);
        assert_source_is_synced(&editor);
    }

    #[test]
    fn a_comment_toggle_undoes_byte_exactly() {
        // Toggle-comment splices in place, so endings survive it too.
        for doc in ["a\nb\nc", "a\r\nb\r\nc", "last"] {
            let mut editor = rust_editor(doc);
            editor.move_cursor_to(0, 0);
            assert!(editor.update(EditorMessage::ToggleComment), "{doc:?}");
            let commented = editor.text();
            assert!(commented.contains("// "), "{doc:?}");
            assert_source_is_synced(&editor);

            assert!(editor.update(EditorMessage::Undo), "{doc:?}");
            assert_eq!(editor.text(), doc, "{doc:?}");
            assert_source_is_synced(&editor);

            assert!(editor.update(EditorMessage::Redo), "{doc:?}");
            assert_eq!(editor.text(), commented, "{doc:?}");
            assert_source_is_synced(&editor);
        }
    }

    #[test]
    fn the_undo_depth_follows_the_shared_setting() {
        let registry = SyntaxRegistry::new(Vec::new(), HashMap::new(), || {});
        let settings =
            SharedEditorConfig::new(1.0, Arc::new(|_: &KeyPress| None));
        settings.set_undo_depth(1);
        let mut editor = TextArea::new("a", &registry, None, settings.clone());

        // Two isolated steps, of which only the most recent may survive.
        editor.move_cursor_to(0, 1);
        editor.update(EditorMessage::CopyLineDown);
        editor.update(EditorMessage::CopyLineDown);
        let before_undo = editor.text();

        assert!(editor.update(EditorMessage::Undo));
        assert_ne!(editor.text(), before_undo);
        assert!(!editor.update(EditorMessage::Undo), "depth of 1 keeps one");
    }

    /// The measurement behind `LINES_WORTH_SPLICING`. Run by hand:
    /// `cargo test --release -- --ignored --nocapture`. Sees buffer work
    /// only, not shaping, so every rebuild number it prints is a floor.
    #[test]
    #[ignore = "a measurement, not an assertion"]
    fn what_an_undo_costs_on_a_long_document() {
        use std::time::Instant;

        const LINES: usize = 20_000;
        let doc: String = (0..LINES).map(|i| format!("line {i}\n")).collect();

        let mut editor = plain_editor(&doc);
        editor.move_cursor_to(LINES / 2, 4);
        editor.update(edit(text_editor::Edit::Insert('X')));
        let started = Instant::now();
        assert!(editor.update(EditorMessage::Undo));
        println!("undo of one line in {LINES}: {:?}", started.elapsed());

        let mut editor = plain_editor(&doc);
        let started = Instant::now();
        editor.replace_document(&doc);
        println!("rebuild of {LINES} (unshaped): {:?}", started.elapsed());

        for reach in [1, 10, 100, 500, 1_000, 2_000, 5_000] {
            let replacement: String =
                (0..reach).map(|i| format!("other {i}\n")).collect();
            let mut editor = plain_editor(&doc);
            let started = Instant::now();
            editor.paste_over((0, 0), (reach, 0), replacement);
            println!(
                "splice of {reach} lines into {LINES}: {:?}",
                started.elapsed()
            );
        }
    }

    #[test]
    fn reload_text_replaces_the_document_and_is_undoable() {
        let mut editor = plain_editor("on disk");
        editor.reload_text("changed underneath");
        assert_eq!(editor.text(), "changed underneath");

        // An external reload is a change like any other, so Ctrl+Z brings
        // the pre-reload text back.
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "on disk");
    }

    #[test]
    fn reload_text_keeps_the_cached_source_in_sync() {
        let mut editor = plain_editor("one\ntwo");
        editor.reload_text("one\ntwo\nthree");
        assert_source_is_synced(&editor);

        editor.update(EditorMessage::Undo);
        assert_source_is_synced(&editor);
    }

    #[test]
    fn reloading_identical_text_records_nothing() {
        let mut editor = plain_editor("unchanged");
        editor.move_cursor_to(0, 4);
        editor.update(edit(text_editor::Edit::Insert('!')));
        editor.update(EditorMessage::Undo);
        assert_eq!(editor.text(), "unchanged");

        // A stamp that moved without the bytes moving must not cost the
        // redo the user just set up.
        editor.reload_text("unchanged");

        assert!(editor.update(EditorMessage::Redo), "the redo survived");
        assert_eq!(editor.text(), "unch!anged");
    }

    #[test]
    fn reload_text_clamps_a_caret_past_the_end_of_a_shortened_file() {
        let mut editor = plain_editor("first line\nsecond line\nthird line");
        editor.move_cursor_to(2, 10);

        editor.reload_text("first line");

        assert_eq!(editor.cursor_position(), (0, 10));
    }

    /// Selects `hello` in "hello world" as a plain drag-style range, cursor
    /// at the far end - the starting point for the undo tests below.
    fn editor_with_hello_selected() -> TextArea {
        let mut editor = plain_editor("hello world");
        editor.restore_selection(
            SavedSelection {
                anchor: (0, 0),
                kind: SelectionKind::Range,
            },
            (0, 5),
        );
        assert_eq!(editor.content.selection().as_deref(), Some("hello"));
        editor
    }

    fn edit(edit: text_editor::Edit) -> EditorMessage {
        EditorMessage::Action(text_editor::Action::Edit(edit))
    }

    fn editor_with_style(
        text: &str,
        extension: &str,
        style: CommentStyle,
    ) -> TextArea {
        let registry = SyntaxRegistry::new(Vec::new(), HashMap::new(), || {});
        let settings =
            SharedEditorConfig::new(1.0, Arc::new(|_: &KeyPress| None));
        settings.set_comment_styles([(extension.to_string(), style)].into());
        TextArea::new(text, &registry, Some(extension), settings)
    }

    /// An editor opened as a `.rs` file with `// ` configured.
    fn rust_editor(text: &str) -> TextArea {
        editor_with_style(text, "rs", CommentStyle::Single("// ".to_string()))
    }

    /// An editor opened as an `.html` file with `<!--` / `-->` configured.
    fn html_editor(text: &str) -> TextArea {
        editor_with_style(
            text,
            "html",
            CommentStyle::Multi {
                left: "<!--".to_string(),
                right: "-->".to_string(),
            },
        )
    }

    #[test]
    fn toggle_comment_round_trips_text_and_cursor() {
        let mut editor = rust_editor("    let x = 1;");
        editor.move_cursor_to(0, 8);

        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "    // let x = 1;");
        assert_eq!(editor.cursor_position(), (0, 11));
        assert_source_is_synced(&editor);

        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "    let x = 1;");
        assert_eq!(editor.cursor_position(), (0, 8));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn toggle_comment_covers_a_multi_line_selection_and_keeps_it() {
        let mut editor = rust_editor("aaa\nbbb");
        editor.restore_selection(
            SavedSelection {
                anchor: (0, 1),
                kind: SelectionKind::Range,
            },
            (1, 2),
        );

        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "// aaa\n// bbb");
        assert_eq!(
            editor.selection(),
            Some(SavedSelection {
                anchor: (0, 4),
                kind: SelectionKind::Range
            })
        );
        assert_eq!(editor.cursor_position(), (1, 5));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn a_selection_ending_at_column_zero_leaves_that_line_alone() {
        let mut editor = rust_editor("aaa\nbbb\nccc");
        editor.restore_selection(
            SavedSelection {
                anchor: (0, 0),
                kind: SelectionKind::Range,
            },
            (2, 0),
        );
        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "// aaa\n// bbb\nccc");
    }

    #[test]
    fn toggle_without_a_configured_style_is_a_clean_no_op() {
        let mut editor = plain_editor("text");
        assert!(!editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "text");
        assert!(
            !editor.update(EditorMessage::Undo),
            "no phantom history entry"
        );
    }

    #[test]
    fn a_toggle_between_keystrokes_stays_its_own_undo_step() {
        // All three edits land inside one coalesce window; the toggle must
        // not fold into either typing burst.
        let mut editor = rust_editor("fn main() {}");
        let _ = editor.update(edit(text_editor::Edit::Insert('a')));
        assert!(editor.update(EditorMessage::ToggleComment));
        let _ = editor.update(edit(text_editor::Edit::Insert('b')));
        assert_eq!(editor.text(), "// abfn main() {}");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "// afn main() {}");
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "afn main() {}");
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "fn main() {}");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn undo_of_a_selection_toggle_restores_the_selection() {
        let mut editor = rust_editor("aaa\nbbb");
        editor.restore_selection(
            SavedSelection {
                anchor: (0, 1),
                kind: SelectionKind::Range,
            },
            (1, 2),
        );
        assert!(editor.update(EditorMessage::ToggleComment));

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "aaa\nbbb");
        assert_eq!(
            editor.selection(),
            Some(SavedSelection {
                anchor: (0, 1),
                kind: SelectionKind::Range
            })
        );
        assert_eq!(editor.cursor_position(), (1, 2));
    }

    #[test]
    fn crlf_line_endings_survive_a_toggle_round_trip() {
        let mut editor = rust_editor("aaa\r\nbbb");
        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "// aaa\r\nbbb");
        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "aaa\r\nbbb");
        assert_source_is_synced(&editor);
    }

    /// The user-facing acceptance example for multi-line styles: wrap two
    /// list items, keeping the same characters selected.
    #[test]
    fn multi_toggle_keeps_the_selected_characters() {
        let text = "    <ul>\n\
                    \x20       <li>Do you have an Internet connection? </li>\n\
                    \x20       <li>Is anti-virus software or a firewall preventing ROBLOX from accessing the Internet?</li>     \n\
                    \x20   </ul>";
        let mut editor = html_editor(text);
        editor.restore_selection(
            SavedSelection {
                anchor: (1, 24),
                kind: SelectionKind::Range,
            },
            (2, 25),
        );
        let selected_before = editor.content.selection();

        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(
            editor.text(),
            "    <ul>\n\
             \x20       <!--<li>Do you have an Internet connection? </li>\n\
             \x20       <li>Is anti-virus software or a firewall preventing ROBLOX from accessing the Internet?</li>     -->\n\
             \x20   </ul>"
        );
        assert_eq!(
            editor.selection(),
            Some(SavedSelection {
                anchor: (1, 28),
                kind: SelectionKind::Range
            })
        );
        assert_eq!(editor.cursor_position(), (2, 25));
        assert_eq!(
            editor.content.selection(),
            selected_before,
            "same characters selected"
        );
        assert_source_is_synced(&editor);

        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), text);
        assert_eq!(
            editor.selection(),
            Some(SavedSelection {
                anchor: (1, 24),
                kind: SelectionKind::Range
            })
        );
        assert_eq!(editor.cursor_position(), (2, 25));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn multi_toggle_on_a_caret_line_round_trips() {
        let mut editor = html_editor("    foo");
        editor.move_cursor_to(0, 7);
        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "    <!--foo-->");
        assert_eq!(editor.cursor_position(), (0, 11), "caret stays before -->");

        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "    foo");
        assert_eq!(editor.cursor_position(), (0, 7));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn undo_of_a_multi_toggle_restores_text_and_selection() {
        let mut editor = html_editor("aaa\nbbb");
        editor.restore_selection(
            SavedSelection {
                anchor: (0, 1),
                kind: SelectionKind::Range,
            },
            (1, 2),
        );
        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "<!--aaa\nbbb-->");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "aaa\nbbb");
        assert_eq!(
            editor.selection(),
            Some(SavedSelection {
                anchor: (0, 1),
                kind: SelectionKind::Range
            })
        );
        assert_eq!(editor.cursor_position(), (1, 2));
    }

    #[test]
    fn crlf_survives_a_multi_toggle_round_trip() {
        let mut editor = html_editor("aaa\r\nbbb\r\nccc");
        editor.restore_selection(
            SavedSelection {
                anchor: (0, 0),
                kind: SelectionKind::Range,
            },
            (1, 3),
        );
        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "<!--aaa\r\nbbb-->\r\nccc");
        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "aaa\r\nbbb\r\nccc");
        assert_source_is_synced(&editor);
    }

    /// Selects `anchor` through `cursor` as a plain range, the shape a
    /// shift+arrow or a mouse drag leaves behind.
    fn select_range(
        editor: &mut TextArea,
        anchor: (usize, usize),
        cursor: (usize, usize),
    ) {
        editor.restore_selection(
            SavedSelection {
                anchor,
                kind: SelectionKind::Range,
            },
            cursor,
        );
    }

    #[test]
    fn a_no_op_splice_reproduces_the_document() {
        // The splice helper's contract, over the same line-ending shapes
        // `a_new_editor_starts_with_a_synced_source` covers: swapping lines
        // for copies of themselves has to be byte-identical.
        for doc in
            ["", "one line", "trailing\n", "a\nb\nc", "crlf\r\nlines\r\n"]
        {
            let mut editor = plain_editor(doc);
            let before = editor.text();
            let count = editor.content.line_count();
            let same = editor.covered_text((0, count - 1));
            editor.splice_lines_in_place(0..count, &same);
            editor.resync_source();
            assert_eq!(editor.text(), before, "{doc:?}");
            assert_source_is_synced(&editor);
        }
    }

    #[test]
    fn a_spliced_in_last_line_borrows_the_documents_ending() {
        // The last line carries no ending of its own, so a copy landing past
        // it has nothing to inherit - without the fallback this splices a
        // lone LF into a CRLF document.
        let mut editor = plain_editor("aaa\r\nbbb");
        let doubled = vec!["bbb".to_string(), "bbb".to_string()];
        editor.splice_lines_in_place(1..2, &doubled);
        editor.resync_source();
        assert_eq!(editor.text(), "aaa\r\nbbb\r\nbbb");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn delete_line_removes_the_caret_line_and_keeps_the_column() {
        let mut editor = plain_editor("aaa\nbbb\nccc");
        editor.move_cursor_to(1, 2);
        assert!(editor.update(EditorMessage::DeleteLine));
        assert_eq!(editor.text(), "aaa\nccc");
        assert_eq!(editor.cursor_position(), (1, 2));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn delete_line_at_the_end_clamps_the_caret_into_the_document() {
        let mut editor = plain_editor("aaa\nbbbb");
        editor.move_cursor_to(1, 4);
        assert!(editor.update(EditorMessage::DeleteLine));
        assert_eq!(editor.text(), "aaa");
        assert_eq!(editor.cursor_position(), (0, 3));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn delete_line_covers_a_multi_line_selection_and_collapses_it() {
        let mut editor = plain_editor("aaa\nbbb\nccc\nddd");
        select_range(&mut editor, (1, 1), (2, 2));
        assert!(editor.update(EditorMessage::DeleteLine));
        assert_eq!(editor.text(), "aaa\nddd");
        assert_eq!(editor.selection(), None);
        assert_eq!(editor.cursor_position(), (1, 2));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn delete_line_can_empty_the_document() {
        let mut editor = plain_editor("only");
        assert!(editor.update(EditorMessage::DeleteLine));
        assert_eq!(editor.text(), "");
        assert_eq!(editor.cursor_position(), (0, 0));
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "only");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn delete_line_on_an_empty_document_records_nothing() {
        let mut editor = plain_editor("");
        assert!(!editor.update(EditorMessage::DeleteLine));
        assert!(
            !editor.update(EditorMessage::Undo),
            "no phantom history entry"
        );
    }

    #[test]
    fn crlf_survives_a_delete_line() {
        let mut editor = plain_editor("aaa\r\nbbb\r\nccc");
        editor.move_cursor_to(1, 0);
        assert!(editor.update(EditorMessage::DeleteLine));
        assert_eq!(editor.text(), "aaa\r\nccc");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn move_line_up_swaps_with_the_line_above_and_carries_the_caret() {
        let mut editor = plain_editor("aaa\nbbb\nccc");
        editor.move_cursor_to(1, 2);
        assert!(editor.update(EditorMessage::MoveLineUp));
        assert_eq!(editor.text(), "bbb\naaa\nccc");
        assert_eq!(editor.cursor_position(), (0, 2));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn move_line_down_swaps_with_the_line_below_and_carries_the_caret() {
        let mut editor = plain_editor("aaa\nbbb\nccc");
        editor.move_cursor_to(1, 2);
        assert!(editor.update(EditorMessage::MoveLineDown));
        assert_eq!(editor.text(), "aaa\nccc\nbbb");
        assert_eq!(editor.cursor_position(), (2, 2));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn move_line_up_at_the_top_is_a_clean_no_op() {
        let mut editor = plain_editor("aaa\nbbb");
        editor.move_cursor_to(0, 1);
        assert!(!editor.update(EditorMessage::MoveLineUp));
        assert_eq!(editor.text(), "aaa\nbbb");
        assert!(
            !editor.update(EditorMessage::Undo),
            "no phantom history entry"
        );
    }

    #[test]
    fn move_line_down_at_the_bottom_is_a_clean_no_op() {
        let mut editor = plain_editor("aaa\nbbb");
        editor.move_cursor_to(1, 1);
        assert!(!editor.update(EditorMessage::MoveLineDown));
        assert_eq!(editor.text(), "aaa\nbbb");
        assert!(
            !editor.update(EditorMessage::Undo),
            "no phantom history entry"
        );
    }

    #[test]
    fn an_edge_no_op_leaves_the_redo_stack_alone() {
        // `record_isolated` clears redo unconditionally, so a no-op that
        // recorded anyway would silently throw away a redo.
        let mut editor = plain_editor("aaa\nbbb");
        editor.move_cursor_to(0, 1);
        assert!(editor.update(EditorMessage::DeleteLine));
        assert!(editor.update(EditorMessage::Undo));
        assert!(!editor.update(EditorMessage::MoveLineUp));
        assert!(editor.update(EditorMessage::Redo));
        assert_eq!(editor.text(), "bbb");
    }

    #[test]
    fn move_line_up_keeps_a_multi_line_selection_on_the_moved_block() {
        let mut editor = plain_editor("aaa\nbbb\nccc\nddd");
        select_range(&mut editor, (1, 1), (2, 2));
        assert!(editor.update(EditorMessage::MoveLineUp));
        assert_eq!(editor.text(), "bbb\nccc\naaa\nddd");
        assert_eq!(
            editor.selection(),
            Some(SavedSelection {
                anchor: (0, 1),
                kind: SelectionKind::Range
            })
        );
        assert_eq!(editor.cursor_position(), (1, 2));
        assert_eq!(editor.content.selection().as_deref(), Some("bb\ncc"));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn moving_the_last_line_up_keeps_the_crlf_endings() {
        // The last line carries no ending; moving it up promotes it to a
        // separator position, where it has to borrow the document's.
        let mut editor = plain_editor("aaa\r\nbbb");
        editor.move_cursor_to(1, 0);
        assert!(editor.update(EditorMessage::MoveLineUp));
        assert_eq!(editor.text(), "bbb\r\naaa");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn move_line_down_swaps_with_a_trailing_empty_line() {
        // "a\nb\n" is three lines, the last one empty - it swaps like any other.
        let mut editor = plain_editor("aaa\nbbb\n");
        editor.move_cursor_to(1, 0);
        assert!(editor.update(EditorMessage::MoveLineDown));
        assert_eq!(editor.text(), "aaa\n\nbbb");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn move_line_up_then_down_round_trips() {
        let mut editor = plain_editor("aaa\nbbb\nccc");
        editor.move_cursor_to(1, 1);
        assert!(editor.update(EditorMessage::MoveLineUp));
        assert!(editor.update(EditorMessage::MoveLineDown));
        assert_eq!(editor.text(), "aaa\nbbb\nccc");
        assert_eq!(editor.cursor_position(), (1, 1));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn undo_of_a_move_restores_the_text_and_the_selection() {
        let mut editor = plain_editor("aaa\nbbb\nccc");
        select_range(&mut editor, (1, 0), (1, 3));
        assert!(editor.update(EditorMessage::MoveLineDown));
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "aaa\nbbb\nccc");
        assert_eq!(
            editor.selection(),
            Some(SavedSelection {
                anchor: (1, 0),
                kind: SelectionKind::Range
            })
        );
        assert_eq!(editor.cursor_position(), (1, 3));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn copy_line_down_duplicates_the_line_and_lands_on_the_copy() {
        let mut editor = plain_editor("aaa\nbbb");
        editor.move_cursor_to(0, 2);
        assert!(editor.update(EditorMessage::CopyLineDown));
        assert_eq!(editor.text(), "aaa\naaa\nbbb");
        assert_eq!(editor.cursor_position(), (1, 2));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn copy_line_up_writes_the_same_text_but_stays_on_the_upper_copy() {
        let mut editor = plain_editor("aaa\nbbb");
        editor.move_cursor_to(0, 2);
        assert!(editor.update(EditorMessage::CopyLineUp));
        assert_eq!(editor.text(), "aaa\naaa\nbbb");
        assert_eq!(editor.cursor_position(), (0, 2));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn copy_line_down_shifts_a_selection_by_the_block_height() {
        let mut editor = plain_editor("aaa\nbbb\nccc");
        select_range(&mut editor, (0, 1), (1, 2));
        assert!(editor.update(EditorMessage::CopyLineDown));
        assert_eq!(editor.text(), "aaa\nbbb\naaa\nbbb\nccc");
        assert_eq!(
            editor.selection(),
            Some(SavedSelection {
                anchor: (2, 1),
                kind: SelectionKind::Range
            })
        );
        assert_eq!(editor.cursor_position(), (3, 2));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn duplicating_the_last_line_keeps_the_crlf_endings() {
        let mut editor = plain_editor("aaa\r\nbbb");
        editor.move_cursor_to(1, 0);
        assert!(editor.update(EditorMessage::CopyLineDown));
        assert_eq!(editor.text(), "aaa\r\nbbb\r\nbbb");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn copy_line_down_on_a_single_line_document() {
        let mut editor = plain_editor("only");
        assert!(editor.update(EditorMessage::CopyLineDown));
        assert_eq!(editor.text(), "only\nonly");
        assert_eq!(editor.cursor_position(), (1, 0));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn a_line_command_stays_its_own_undo_step() {
        // Same rule as toggle-comment: it neither joins the typing burst
        // before it nor absorbs the keystroke after.
        let mut editor = plain_editor("aaa\nbbb");
        editor.move_cursor_to(0, 3);
        assert!(editor.update(edit(text_editor::Edit::Insert('x'))));
        assert!(editor.update(EditorMessage::MoveLineDown));
        assert!(editor.update(edit(text_editor::Edit::Insert('y'))));
        assert_eq!(editor.text(), "bbb\naaaxy");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "bbb\naaax");
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "aaax\nbbb");
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "aaa\nbbb");
        assert_source_is_synced(&editor);
    }

    #[test]
    fn undo_of_a_cut_reselects_the_cut_text() {
        // The reported bug. A cut publishes exactly one `Edit::Delete`, so
        // this is the whole cut path as the widget produces it.
        let mut editor = editor_with_hello_selected();
        assert!(editor.update(edit(text_editor::Edit::Delete)));
        assert_eq!(editor.text(), " world");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "hello world");
        assert_eq!(editor.content.selection().as_deref(), Some("hello"));
        assert_eq!(
            editor.selection(),
            Some(SavedSelection {
                anchor: (0, 0),
                kind: SelectionKind::Range
            })
        );
        assert_eq!(editor.cursor_position(), (0, 5));
    }

    #[test]
    fn undo_of_a_paste_over_a_selection_reselects_the_replaced_text() {
        let mut editor = editor_with_hello_selected();
        assert!(editor.update(edit(text_editor::Edit::Paste(Arc::new(
            "bye".to_string()
        )))));
        assert_eq!(editor.text(), "bye world");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "hello world");
        assert_eq!(editor.content.selection().as_deref(), Some("hello"));
    }

    #[test]
    fn undo_of_typing_over_a_selection_reselects_the_replaced_text() {
        // Matches VS Code, which restores `beforeCursorState` uniformly for
        // every edit - typing included, not just cut and paste.
        let mut editor = editor_with_hello_selected();
        assert!(editor.update(edit(text_editor::Edit::Insert('X'))));
        assert_eq!(editor.text(), "X world");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "hello world");
        assert_eq!(editor.content.selection().as_deref(), Some("hello"));
    }

    #[test]
    fn redo_after_undoing_a_cut_leaves_no_selection() {
        let mut editor = editor_with_hello_selected();
        editor.update(edit(text_editor::Edit::Delete));
        editor.update(EditorMessage::Undo);

        assert!(editor.update(EditorMessage::Redo));
        assert_eq!(editor.text(), " world");
        assert_eq!(editor.selection(), None);
    }

    #[test]
    fn undo_restores_a_word_selection_as_a_word_selection() {
        // The kind matters, not just the text: a word selection anchors at
        // the click position with its bounds implied, so restoring it as a
        // plain anchor-to-cursor range would collapse it to nothing.
        let mut editor = plain_editor("hello world");
        editor.move_cursor_to(0, 8); // inside "world"
        editor.content.perform(text_editor::Action::SelectWord);
        assert_eq!(editor.content.selection().as_deref(), Some("world"));

        editor.update(edit(text_editor::Edit::Insert('X')));
        assert_eq!(editor.text(), "hello X");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "hello world");
        assert_eq!(editor.content.selection().as_deref(), Some("world"));
        assert_eq!(
            editor.selection().map(|s| s.kind),
            Some(SelectionKind::Word)
        );
    }

    #[test]
    fn undo_restores_a_line_selection_as_a_line_selection() {
        let mut editor = plain_editor("first line\nsecond line");
        editor.move_cursor_to(1, 3);
        editor.content.perform(text_editor::Action::SelectLine);

        editor.update(edit(text_editor::Edit::Insert('X')));
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "first line\nsecond line");
        assert_eq!(editor.content.selection().as_deref(), Some("second line"));
        assert_eq!(
            editor.selection().map(|s| s.kind),
            Some(SelectionKind::Line)
        );
    }

    #[test]
    fn undo_of_plain_typing_leaves_a_collapsed_caret() {
        // Nothing was selected before the edit, so nothing may be selected
        // after the undo - in particular not a degenerate zero-width range.
        let mut editor = plain_editor("hello");
        editor.move_cursor_to(0, 5);
        editor.update(edit(text_editor::Edit::Insert('!')));

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "hello");
        assert_eq!(editor.selection(), None);
        assert_eq!(editor.cursor_position(), (0, 5));
    }

    #[test]
    fn undo_of_a_word_delete_reselects_the_deleted_word() {
        // `word_delete_backward` is a `Binding::Sequence`, and a sequence
        // publishes each element as its own message - so the `Select` lands
        // first and the `Backspace` records a caret that already has the word
        // selected. Undo therefore brings it back selected. VS Code collapses
        // the caret here instead; accepted, since it reads the same as undoing
        // a cut and the alternative is an atomic word-delete command.
        let mut editor = plain_editor("hello world");
        editor.move_cursor_to(0, 11);
        assert!(!editor.update(EditorMessage::Action(
            text_editor::Action::Select(Motion::Left.widen())
        )));
        assert!(editor.update(edit(text_editor::Edit::Backspace)));
        assert_eq!(editor.text(), "hello ");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "hello world");
        assert_eq!(editor.content.selection().as_deref(), Some("world"));
    }

    #[test]
    fn a_selection_restoring_undo_keeps_the_cached_source_in_sync() {
        // Restoring a selection replays `SelectWord`/`SelectLine`/`move_to`,
        // none of which are edits - so the cache must be resynced exactly
        // once, by the content swap, and not again by the restore.
        let mut editor = editor_with_hello_selected();
        editor.update(edit(text_editor::Edit::Delete));
        let after_edit = editor.source.clone();

        editor.update(EditorMessage::Undo);
        assert_source_is_synced(&editor);
        assert!(
            !Arc::ptr_eq(&after_edit, &editor.source),
            "undo changed the text"
        );
    }

    #[test]
    fn settings_over_separately_allocated_equal_sources_compare_unequal() {
        // Documents the deliberate trade-off in `HighlighterSettings::eq`:
        // it compares source pointers, not bytes. Two identical but
        // separately allocated strings therefore compare unequal, costing
        // one redundant reparse. That direction is harmless; the reverse -
        // missing a real change - would leave stale colors on screen, and
        // only `resync_source` ever mints a new pointer.
        let settings = |source: &str| HighlighterSettings {
            source: Arc::new(source.to_string()),
            grammar: None,
            revision: 0,
            matches: Arc::new(Vec::new()),
            current_match: None,
            foreground_alpha: 1.0,
            edited_from: None,
        };
        assert!(settings("ab") != settings("ab"));
    }

    #[test]
    fn range_selection_round_trips_through_save_and_restore() {
        let mut editor = plain_editor("hello\nworld");
        let saved = SavedSelection {
            anchor: (0, 2),
            kind: SelectionKind::Range,
        };
        editor.restore_selection(saved, (1, 4));
        assert_eq!(editor.selection(), Some(saved));
        assert_eq!(editor.cursor_position(), (1, 4));
    }

    #[test]
    fn word_selection_round_trips_through_save_and_restore() {
        let mut editor = plain_editor("hello world");
        // A double click: Click places the cursor, SelectWord selects around it.
        editor.move_cursor_to(0, 8); // inside "world"
        editor.content.perform(text_editor::Action::SelectWord);
        assert_eq!(editor.content.selection().as_deref(), Some("world"));

        let saved = editor.selection().expect("word selection should save");
        assert_eq!(
            saved,
            SavedSelection {
                anchor: (0, 8),
                kind: SelectionKind::Word
            }
        );

        // Simulate the tab going away and coming back: clear, then restore.
        editor.move_cursor_to(0, 0);
        assert_eq!(editor.selection(), None);
        editor.restore_selection(saved, (0, 8));
        assert_eq!(editor.content.selection().as_deref(), Some("world"));
        assert_eq!(editor.selection(), Some(saved));
    }

    #[test]
    fn line_selection_round_trips_through_save_and_restore() {
        let mut editor = plain_editor("first line\nsecond line");
        editor.move_cursor_to(1, 3);
        editor.content.perform(text_editor::Action::SelectLine);
        let saved = editor.selection().expect("line selection should save");
        assert_eq!(saved.kind, SelectionKind::Line);

        editor.move_cursor_to(0, 0);
        editor.restore_selection(saved, (1, 3));
        assert_eq!(editor.content.selection().as_deref(), Some("second line"));
    }

    #[test]
    fn selection_is_none_without_a_selection() {
        let mut editor = plain_editor("hello");
        editor.move_cursor_to(0, 3);
        assert_eq!(editor.selection(), None);
        assert_eq!(editor.cursor_position(), (0, 3));
    }

    #[test]
    fn move_cursor_to_clears_a_leftover_selection() {
        let mut editor = plain_editor("hello world");
        editor.restore_selection(
            SavedSelection {
                anchor: (0, 0),
                kind: SelectionKind::Range,
            },
            (0, 5),
        );
        assert!(editor.selection().is_some());
        editor.move_cursor_to(0, 2);
        assert_eq!(editor.selection(), None);
        assert_eq!(editor.cursor_position(), (0, 2));
    }

    #[test]
    fn clamp_position_clamps_line_and_column_to_the_document() {
        let content = Content::with_text("hello\nhi");
        assert_eq!(
            clamp_position(&content, (9, 9)),
            Position { line: 1, column: 2 }
        );
        assert_eq!(
            clamp_position(&content, (0, 3)),
            Position { line: 0, column: 3 }
        );
    }

    #[test]
    fn clamp_position_backs_up_to_a_char_boundary() {
        // 'é' occupies bytes 1..3, so byte offset 2 is mid-character.
        let content = Content::with_text("héllo");
        assert_eq!(
            clamp_position(&content, (0, 2)),
            Position { line: 0, column: 1 }
        );
    }

    #[test]
    fn binding_for_covers_every_editor_action() {
        use jumppad_actions::Category;

        for action in Action::in_category(Category::Editor) {
            assert!(
                binding_for(action).is_some(),
                "{action} is an editor action with no binding"
            );
        }
        // And nothing else: an app action must fall through to the shell
        // rather than be swallowed here.
        for action in Action::in_category(Category::App) {
            assert!(binding_for(action).is_none(), "{action} is not ours");
        }

        assert!(matches!(
            binding_for(Action::WordDeleteBackward),
            Some(Binding::Sequence(_))
        ));
        assert!(matches!(
            binding_for(Action::DocumentStart),
            Some(Binding::Move(Motion::DocumentStart))
        ));
        assert!(matches!(
            binding_for(Action::SelectDocumentEnd),
            Some(Binding::Select(Motion::DocumentEnd))
        ));
        assert!(matches!(
            binding_for(Action::MoveLineUp),
            Some(Binding::Custom(EditorMessage::MoveLineUp))
        ));
    }

    /// Stands in for the resolver the app supplies. Which *chords* map to
    /// which actions is `jumppad_keybinds`' business and is tested there;
    /// what this crate owns is turning a resolved action into a binding, so
    /// these hand the answer over directly.
    fn resolving(
        action: Option<Action>,
    ) -> impl Fn(&KeyPress) -> Option<Action> {
        move |_| action
    }

    #[test]
    fn a_resolved_editor_action_becomes_its_binding() {
        let event = press(
            keyboard::Modifiers::ALT,
            key::Code::ArrowUp,
            keyboard::Key::Named(key::Named::ArrowUp),
        );
        assert!(matches!(
            key_binding(event, &resolving(Some(Action::MoveLineUp))),
            Some(Binding::Custom(EditorMessage::MoveLineUp))
        ));
    }

    #[test]
    fn a_resolved_app_action_falls_through_instead_of_being_swallowed() {
        // `binding_for` returns `None` for an action the shell owns, and the
        // press has to stay unclaimed so the shell still sees it.
        let event = press(
            keyboard::Modifiers::CTRL,
            key::Code::KeyN,
            keyboard::Key::Character("n".into()),
        );
        assert!(matches!(
            key_binding(event, &resolving(Some(Action::NewTab))),
            None | Some(Binding::Unfocus)
        ));
    }

    #[test]
    fn an_unresolved_press_reaches_iceds_own_dispatch() {
        let event = press_with_text(
            keyboard::Modifiers::empty(),
            key::Code::KeyN,
            keyboard::Key::Character("n".into()),
            Some("n"),
        );
        assert!(matches!(
            key_binding(event, &resolving(None)),
            Some(Binding::Insert('n'))
        ));
    }

    #[test]
    fn command_held_unrecognized_character_does_not_fall_through_to_insert() {
        // `Modifiers::CTRL` stands in for `command()`, which resolves per-OS
        // at compile time.
        let event = press_with_text(
            keyboard::Modifiers::CTRL,
            key::Code::KeyN,
            keyboard::Key::Character("n".into()),
            Some("n"),
        );
        assert!(key_binding(event, &resolving(None)).is_none());
    }

    #[test]
    fn unfocused_status_returns_none_even_for_a_resolved_action() {
        let mut event = press(
            keyboard::Modifiers::CTRL,
            key::Code::KeyZ,
            keyboard::Key::Character("z".into()),
        );
        event.status = Status::Active;
        assert!(key_binding(event, &resolving(Some(Action::Redo))).is_none());
    }

    #[test]
    fn apply_alpha_at_full_solid_returns_the_color_unchanged() {
        let color = iced::Color::from_rgba(0.2, 0.4, 0.6, 0.9);
        assert_eq!(apply_alpha(color, 1.0), color);
    }

    #[test]
    fn apply_alpha_scales_the_alpha_channel_only() {
        let color = iced::Color::from_rgba(0.2, 0.4, 0.6, 0.8);
        let scaled = apply_alpha(color, 0.5);
        assert_eq!((scaled.r, scaled.g, scaled.b), (0.2, 0.4, 0.6));
        assert!((scaled.a - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn editor_style_leaves_background_and_value_alone_at_full_solid() {
        let theme = Theme::ALL[0].clone();
        let default = text_editor::default(&theme, text_editor::Status::Active);
        let style = editor_style(&theme, text_editor::Status::Active, 1.0, 1.0);
        assert_eq!(style.background, default.background);
        assert_eq!(style.value, default.value);
    }

    #[test]
    fn editor_style_drops_its_background_when_translucent_but_keeps_the_value()
    {
        let theme = Theme::ALL[0].clone();
        let default = text_editor::default(&theme, text_editor::Status::Active);
        let style = editor_style(&theme, text_editor::Status::Active, 0.5, 1.0);
        assert_eq!(style.background, Background::Color(Color::TRANSPARENT));
        assert_eq!(style.value, default.value); // foreground untouched

        let style = editor_style(&theme, text_editor::Status::Active, 1.0, 0.3);
        assert_eq!(style.background, default.background); // background untouched
        assert_ne!(style.value, default.value);
    }

    #[test]
    fn an_ordinary_edit_reports_only_the_text_moving() {
        // An empty list arrives after every edit while find is closed;
        // minting a new `Arc` would read as a find change.
        let mut editor = plain_editor("a\nb\nc\nd");
        editor.set_find_matches(Vec::new(), None);
        let before = editor.highlighter_settings();

        editor.move_cursor_to(2, 1);
        editor.update(edit(text_editor::Edit::Insert('X')));
        editor.set_find_matches(Vec::new(), None);
        let after = editor.highlighter_settings();

        assert!(after.only_the_text_moved_since(&before));
        assert_eq!(after.edited_from, Some(2));
    }

    #[test]
    fn an_edit_that_also_moves_the_find_matches_recolors_from_the_top() {
        // Match coloring can land on a line no edit went near.
        let mut editor = plain_editor("a\nb\nc\nd");
        editor.set_find_matches(Vec::new(), None);
        let before = editor.highlighter_settings();

        editor.move_cursor_to(2, 1);
        editor.update(edit(text_editor::Edit::Insert('X')));
        editor.set_find_matches(
            vec![FindMatch {
                line: 2,
                start: 0,
                end: 1,
            }],
            Some(0),
        );
        let after = editor.highlighter_settings();

        assert!(!after.only_the_text_moved_since(&before));
    }

    #[test]
    fn the_highlighter_resumes_at_the_edit_and_rewinds_for_anything_else() {
        let mut editor = plain_editor("a\nb\nc\nd\ne\nf");
        editor.set_find_matches(Vec::new(), None);
        let mut highlighter =
            TreeSitterHighlighter::new(&editor.highlighter_settings());
        assert_eq!(highlighter.current_line(), 0);

        editor.move_cursor_to(4, 1);
        editor.update(edit(text_editor::Edit::Insert('X')));
        editor.set_find_matches(Vec::new(), None);
        highlighter.update(&editor.highlighter_settings());
        assert_eq!(
            highlighter.current_line(),
            4,
            "an edit resumes where it is"
        );

        // iced reports its own topmost changed line afterwards; the lower of
        // the two has to win.
        highlighter.change_line(2);
        assert_eq!(highlighter.current_line(), 2);
        highlighter.change_line(5);
        assert_eq!(highlighter.current_line(), 2, "the lower one stands");

        // A find change is not an edit, so it goes back to the top.
        editor.set_find_matches(
            vec![FindMatch {
                line: 4,
                start: 0,
                end: 1,
            }],
            Some(0),
        );
        highlighter.update(&editor.highlighter_settings());
        assert_eq!(highlighter.current_line(), 0);
    }

    #[test]
    fn set_foreground_alpha_reaches_color_for() {
        // The only test that writes the global, restored at the end so the
        // parallel test threads never see a scaled alpha.
        let settings =
            SharedEditorConfig::new(1.0, Arc::new(|_: &KeyPress| None));
        settings.set_foreground_alpha(0.25);
        let keyword = Highlighted::Syntax(HighlightCategory::Keyword);
        let color = color_for(keyword);
        let base = base_color_for(keyword);
        settings.set_foreground_alpha(1.0);

        assert_eq!((color.r, color.g, color.b), (base.r, base.g, base.b));
        assert!((color.a - base.a * 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn shared_editor_config_round_trips_its_settings() {
        let settings =
            SharedEditorConfig::new(0.8, Arc::new(|_: &KeyPress| None));
        assert_eq!(settings.background_alpha(), 0.8);
        settings.set_background_alpha(0.4);
        assert_eq!(settings.background_alpha(), 0.4);
        // Out-of-range input clamps rather than propagating.
        settings.set_background_alpha(2.0);
        assert_eq!(settings.background_alpha(), 1.0);

        // The resolver is swappable so a `keybinds.toml` reload reaches tabs
        // that already exist.
        assert_eq!(
            (settings.resolver())(&press(
                keyboard::Modifiers::CTRL,
                key::Code::KeyZ,
                keyboard::Key::Character("z".into()),
            )),
            None
        );
        settings.set_resolver(Arc::new(|_: &KeyPress| Some(Action::Undo)));
        assert_eq!(
            (settings.resolver())(&press(
                keyboard::Modifiers::CTRL,
                key::Code::KeyZ,
                keyboard::Key::Character("z".into()),
            )),
            Some(Action::Undo)
        );
    }

    /// Builds a highlighter over `source` with `matches` already applied.
    fn highlighter_with(
        source: &str,
        matches: Vec<FindMatch>,
        current: Option<usize>,
    ) -> TreeSitterHighlighter {
        TreeSitterHighlighter::new(&HighlighterSettings {
            source: Arc::new(source.to_string()),
            grammar: None,
            revision: 0,
            matches: Arc::new(matches),
            current_match: current,
            foreground_alpha: 1.0,
            edited_from: None,
        })
    }

    #[test]
    fn highlight_line_emits_a_span_per_match_on_that_line() {
        let source = "find me\nand me";
        let matches = vec![
            FindMatch {
                line: 0,
                start: 5,
                end: 7,
            },
            FindMatch {
                line: 1,
                start: 4,
                end: 6,
            },
        ];
        let mut highlighter = highlighter_with(source, matches, Some(1));

        let first: Vec<_> = highlighter.highlight_line("find me").collect();
        assert_eq!(first, vec![(5..7, Highlighted::Match)]);

        // The second is the current match, so it gets the distinct color.
        let second: Vec<_> = highlighter.highlight_line("and me").collect();
        assert_eq!(second, vec![(4..6, Highlighted::CurrentMatch)]);
    }

    #[test]
    fn match_spans_come_after_syntax_spans_so_they_win_on_overlap() {
        // iced feeds these to `AttrsList::add_span`, where the last span
        // covering a byte wins - so a match must be emitted last to be seen.
        let mut highlighter = highlighter_with(
            "keyword",
            vec![FindMatch {
                line: 0,
                start: 0,
                end: 7,
            }],
            None,
        );
        // Stand in for a grammar by injecting a syntax span directly.
        highlighter.spans = Arc::new(vec![syntax_registry::HighlightSpan {
            start: 0,
            end: 7,
            category: HighlightCategory::Keyword,
        }]);

        let spans: Vec<_> = highlighter.highlight_line("keyword").collect();
        assert_eq!(
            spans,
            vec![
                (0..7, Highlighted::Syntax(HighlightCategory::Keyword)),
                (0..7, Highlighted::Match),
            ],
            "the match span must be last"
        );
    }

    #[test]
    fn settings_differing_only_in_find_state_are_not_equal() {
        // Guards the hand-written `PartialEq`: iced re-runs the highlighter
        // only when settings compare unequal, so dropping the find fields
        // there would leave match coloring frozen on screen.
        let matches = Arc::new(vec![FindMatch {
            line: 0,
            start: 0,
            end: 2,
        }]);
        let base = HighlighterSettings {
            source: Arc::new("ab".to_string()),
            grammar: None,
            revision: 0,
            matches: matches.clone(),
            current_match: None,
            foreground_alpha: 1.0,
            edited_from: None,
        };

        let same = HighlighterSettings { ..base.clone() };
        assert!(base == same);

        let moved_current = HighlighterSettings {
            current_match: Some(0),
            ..base.clone()
        };
        assert!(base != moved_current, "a new current match must invalidate");

        let other_matches = HighlighterSettings {
            matches: Arc::new(vec![FindMatch {
                line: 9,
                start: 1,
                end: 2,
            }]),
            ..base.clone()
        };
        assert!(base != other_matches, "a new match list must invalidate");
    }

    #[test]
    fn settings_differing_only_in_registry_revision_are_not_equal() {
        // The whole mechanism for picking up a late-loading injection
        // target: nothing else about the settings moves when one resolves.
        let base = HighlighterSettings {
            source: Arc::new("ab".to_string()),
            grammar: None,
            revision: 0,
            matches: Arc::new(Vec::new()),
            current_match: None,
            foreground_alpha: 1.0,
            edited_from: None,
        };
        let loaded_something = HighlighterSettings {
            revision: 1,
            ..base.clone()
        };
        assert!(base != loaded_something);
    }

    #[test]
    fn settings_differing_only_in_foreground_alpha_are_not_equal() {
        // How a config reload's new alpha reaches text already on screen:
        // nothing else about the settings moves when only alpha changes.
        let base = HighlighterSettings {
            source: Arc::new("ab".to_string()),
            grammar: None,
            revision: 0,
            matches: Arc::new(Vec::new()),
            current_match: None,
            foreground_alpha: 1.0,
            edited_from: None,
        };
        let faded = HighlighterSettings {
            foreground_alpha: 0.5,
            ..base.clone()
        };
        assert!(base != faded);
    }

    #[test]
    fn set_find_matches_reaches_the_highlighter_settings() {
        let mut editor = plain_editor("find me");
        editor.set_find_matches(
            vec![FindMatch {
                line: 0,
                start: 5,
                end: 7,
            }],
            Some(0),
        );
        assert_eq!(editor.find_matches.len(), 1);
        assert_eq!(editor.find_current, Some(0));

        editor.set_find_matches(Vec::new(), None);
        assert!(editor.find_matches.is_empty());
        assert_eq!(editor.find_current, None);
    }
}
