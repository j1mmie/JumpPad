mod comment;
mod history;
mod scrollbar;
pub mod text_editor;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use editor_core::{
    EditorMessage, FindMatch, SCROLLBAR_THUMB_WASH, SavedSelection, SelectionKind,
    TextEditorWidget, scrollbar_wash,
};
use history::{CursorState, History};
use iced::advanced::text::Highlighter;
use iced::advanced::text::highlighter::Format;
use iced::keyboard::{self, key};
use iced::{Background, Border, Color, Element, Fill, Font, Theme};
use syntax_registry::{Grammar, Handle, HighlightCategory, PollResult, SyntaxRegistry};
use text_editor::{Binding, Content, Cursor, KeyPress, Motion, Position, Status, text_editor};

/// A named editor-level action a `keybinds.toml` override can target - a
/// small, closed set: the commands this crate has custom logic for beyond
/// iced's own stock `text_editor` defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorCommand {
    WordDeleteBackward,
    WordDeleteForward,
    DocumentStart,
    SelectDocumentStart,
    DocumentEnd,
    SelectDocumentEnd,
    Undo,
    Redo,
    ToggleComment,
}

/// The canonical `keybinds.toml` override name for each [`EditorCommand`].
pub const EDITOR_COMMAND_NAMES: &[(&str, EditorCommand)] = &[
    ("word_delete_backward", EditorCommand::WordDeleteBackward),
    ("word_delete_forward", EditorCommand::WordDeleteForward),
    ("document_start", EditorCommand::DocumentStart),
    ("select_document_start", EditorCommand::SelectDocumentStart),
    ("document_end", EditorCommand::DocumentEnd),
    ("select_document_end", EditorCommand::SelectDocumentEnd),
    ("undo", EditorCommand::Undo),
    ("redo", EditorCommand::Redo),
    ("toggle_comment", EditorCommand::ToggleComment),
];

/// A user override's resolved chord, keyed by physical key (layout-
/// independent) - unlike this crate's pre-existing hardcoded bindings
/// below, which stay logical-key based.
pub type EditorOverrides = HashMap<(keyboard::Modifiers, key::Code), EditorCommand>;

/// Editor settings the app can change after construction: a config reload
/// writes here, and every open [`TextArea`] reads through a shared handle
/// on each `view`, which is what lets a reload reach tabs that already
/// exist. One per app, created alongside [`TextArea::factory`].
pub struct SharedEditorConfig {
    /// `f32` bits - an atomic can't hold a float directly.
    background_alpha: AtomicU32,
    /// `Arc` inside the lock so `view` clones a refcount out per redraw,
    /// not the whole map.
    overrides: RwLock<Arc<EditorOverrides>>,
    /// Extension -> single-line comment prefix, flattened from config's
    /// `[[comment_styles]]`. Keys are lowercase; look up lowercased.
    comment_prefixes: RwLock<Arc<HashMap<String, String>>>,
}

impl SharedEditorConfig {
    pub fn new(background_alpha: f32, overrides: EditorOverrides) -> Arc<Self> {
        Arc::new(Self {
            background_alpha: AtomicU32::new(background_alpha.clamp(0.0, 1.0).to_bits()),
            overrides: RwLock::new(Arc::new(overrides)),
            comment_prefixes: RwLock::new(Arc::new(HashMap::new())),
        })
    }

    pub fn background_alpha(&self) -> f32 {
        f32::from_bits(self.background_alpha.load(Ordering::Relaxed))
    }

    pub fn overrides(&self) -> Arc<EditorOverrides> {
        self.overrides.read().unwrap().clone()
    }

    pub fn set_background_alpha(&self, alpha: f32) {
        self.background_alpha
            .store(alpha.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Routed through here so settings have one mutation API, but stored in
    /// `FOREGROUND_ALPHA` - see that static for why it's global.
    pub fn set_foreground_alpha(&self, alpha: f32) {
        FOREGROUND_ALPHA.store(alpha.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_overrides(&self, overrides: EditorOverrides) {
        *self.overrides.write().unwrap() = Arc::new(overrides);
    }

    pub fn comment_prefixes(&self) -> Arc<HashMap<String, String>> {
        self.comment_prefixes.read().unwrap().clone()
    }

    pub fn set_comment_prefixes(&self, prefixes: HashMap<String, String>) {
        *self.comment_prefixes.write().unwrap() = Arc::new(prefixes);
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

    /// The caret state to hand `History`: position plus whatever is selected,
    /// since undoing an edit that replaced a selection has to put it back.
    fn cursor_state(&self) -> CursorState {
        CursorState { position: self.cursor_position(), selection: self.selection() }
    }

    fn grammar(&self) -> Option<Arc<Grammar>> {
        match &self.highlighting {
            Highlighting::Ready(_, grammar) => Some(grammar.clone()),
            _ => None,
        }
    }

    /// Shared by `EditorMessage::Undo`/`Redo`: hands the current text and
    /// caret to `op` and, if it returns a state to restore, replaces the
    /// content wholesale and puts the caret back. Returns whether an edit
    /// actually happened, same contract `update` has for `Action`s.
    fn apply_history(
        &mut self,
        op: impl FnOnce(&mut History, &str, CursorState) -> Option<(String, CursorState)>,
    ) -> bool {
        let current = self.cursor_state();
        let Some((restored_text, restored)) =
            op(&mut self.history, self.source.as_str(), current)
        else {
            return false;
        };
        self.replace_document(&restored_text);
        // Replaying the selection is the point: `move_cursor_to` clears one,
        // so an undo that only moved the cursor would drop the selection the
        // undone edit had replaced. Neither branch changes text, so neither
        // needs a second `resync_source`.
        match restored.selection {
            Some(selection) => self.restore_selection(selection, restored.position),
            None => self.move_cursor_to(restored.position.0, restored.position.1),
        }
        true
    }

    /// Replaces the whole document, carrying the view across - the rebuilt
    /// content starts at the top of the document, so without the restore an
    /// edit already on screen would still jump the document around. Shared
    /// by undo/redo and toggle-comment; always `Content::with_text`, never
    /// SelectAll+Paste, which is quadratic (see AGENTS.md).
    fn replace_document(&mut self, text: &str) {
        let view = self.content.scrolled_to();
        self.content = Content::with_text(text);
        self.content.restore_view(view);
        self.resync_source();
    }

    /// The single-line comment prefix configured for this tab's file type.
    fn comment_prefix(&self) -> Option<String> {
        let extension = self.extension.as_deref()?;
        self.settings.comment_prefixes().get(extension).cloned()
    }

    /// The document with lines `first..first + replacements.len()` swapped
    /// out, joined the way `Content::text()` joins (separators between
    /// lines, never after the last) - so everything outside the replaced
    /// range, line endings included, round-trips byte-identically.
    fn text_with_lines_replaced(&self, first: usize, replacements: &[String]) -> String {
        let mut text = String::with_capacity(self.source.len());
        let mut lines = self.content.lines().enumerate().peekable();
        while let Some((index, line)) = lines.next() {
            match index.checked_sub(first).and_then(|i| replacements.get(i)) {
                Some(replacement) => text.push_str(replacement),
                None => text.push_str(&line.text),
            }
            if lines.peek().is_some() {
                text.push_str(if line.ending == text_editor::LineEnding::None {
                    text_editor::LineEnding::default().as_str()
                } else {
                    line.ending.as_str()
                });
            }
        }
        text
    }

    /// Comments or uncomments the covered lines with the file type's
    /// configured prefix. A file with no style, or all-blank coverage, is a
    /// silent no-op that leaves the tab clean.
    fn toggle_comment(&mut self) -> bool {
        let Some(prefix) = self.comment_prefix() else {
            return false;
        };
        let cursor = self.cursor_position();
        let selection = self.selection();
        let (first, last) = comment::covered_lines(cursor, selection);
        let covered: Vec<String> = (first..=last)
            .filter_map(|index| self.content.line(index).map(|line| line.text.into_owned()))
            .collect();
        let covered: Vec<&str> = covered.iter().map(String::as_str).collect();
        let Some(toggled) = comment::toggle_comment(&covered, &prefix) else {
            return false;
        };

        // Its own undo step - a toggle shouldn't fold into a typing burst -
        // recorded only now that an edit is certain to happen.
        self.history.record_isolated(&self.source, self.cursor_state());
        let new_text = self.text_with_lines_replaced(first, &toggled.lines);
        self.replace_document(&new_text);

        let shift = |pos| comment::shift_position(pos, first, &toggled.edits);
        match selection {
            Some(saved) => self.restore_selection(
                SavedSelection { anchor: shift(saved.anchor), ..saved },
                shift(cursor),
            ),
            None => {
                let (line, column) = shift(cursor);
                self.move_cursor_to(line, column);
            }
        }
        true
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
        }
    }
}

impl TextEditorWidget for TextArea {
    fn view(&self) -> Element<'_, EditorMessage> {
        let settings = self.highlighter_settings();
        let overrides = self.settings.overrides();
        let background_alpha = self.settings.background_alpha();
        let foreground_alpha = foreground_alpha();
        text_editor(&self.content)
            .id(iced::advanced::widget::Id::new(editor_core::EDITOR_WIDGET_ID))
            .placeholder("")
            .font(Font::MONOSPACE)
            .height(Fill)
            .style(move |theme, status| editor_style(theme, status, background_alpha, foreground_alpha))
            .highlight_with::<TreeSitterHighlighter>(settings, to_format)
            .key_binding(move |press| key_binding(press, &overrides))
            .on_action(EditorMessage::Action)
            .into()
    }

    fn update(&mut self, message: EditorMessage) -> bool {
        match message {
            EditorMessage::Action(action) => {
                // Only an `Edit` changes the document's text. `Move`,
                // `Select`, `Click`, `Drag` and `Scroll` just move the cursor
                // or the viewport, so the `source` cache stays valid across
                // all of them - which is what keeps a selection drag, the
                // whole reason the cache exists, off the rebuild path.
                let is_edit = action.is_edit();
                if is_edit {
                    // The pre-edit text is already sitting in the cache. The
                    // caret goes with it so undo can re-select whatever this
                    // edit is about to replace.
                    self.history.record_before_edit(&self.source, self.cursor_state());
                }
                self.content.perform(action);
                if is_edit {
                    self.resync_source();
                }
                is_edit
            }
            EditorMessage::Undo => self.apply_history(History::undo),
            EditorMessage::Redo => self.apply_history(History::redo),
            EditorMessage::ToggleComment => self.toggle_comment(),
        }
    }

    fn text(&self) -> String {
        self.source.as_str().to_owned()
    }

    fn set_text(&mut self, text: &str) {
        self.content = Content::with_text(text);
        self.resync_source();
    }

    fn poll_highlighting(&mut self) {
        if !matches!(self.highlighting, Highlighting::Pending(_)) {
            return;
        }
        let Highlighting::Pending(handle) = std::mem::replace(&mut self.highlighting, Highlighting::None)
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
            self.content.perform(text_editor::Action::Move(Motion::Right));
        }
        let position = clamp_position(&self.content, (line, column));
        self.content.move_to(Cursor { position, selection: None });
    }

    fn set_find_matches(&mut self, matches: Vec<FindMatch>, current: Option<usize>) {
        self.find_matches = Arc::new(matches);
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
        let selected = self.content.selection().filter(|text| !text.is_empty())?;
        let line = self.content.line(anchor.line)?;
        let kind = if selected.trim_end_matches(['\r', '\n']) == line.text.as_ref() {
            SelectionKind::Line
        } else {
            SelectionKind::Word
        };
        Some(SavedSelection { anchor: anchor_position, kind })
    }

    fn restore_selection(&mut self, selection: SavedSelection, cursor: (usize, usize)) {
        let anchor = clamp_position(&self.content, selection.anchor);
        match selection.kind {
            SelectionKind::Range => {
                self.content.move_to(Cursor {
                    position: clamp_position(&self.content, cursor),
                    selection: Some(anchor),
                });
            }
            SelectionKind::Word => {
                self.content.move_to(Cursor { position: anchor, selection: None });
                self.content.perform(text_editor::Action::SelectWord);
            }
            SelectionKind::Line => {
                self.content.move_to(Cursor { position: anchor, selection: None });
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
fn clamp_position(content: &Content, (line, column): (usize, usize)) -> Position {
    let line = line.min(content.line_count().saturating_sub(1));
    let text = content.line(line).map(|l| l.text.into_owned()).unwrap_or_default();
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
        };
        highlighter.update(settings);
        highlighter
    }

    fn update(&mut self, settings: &Self::Settings) {
        self.spans = match &settings.grammar {
            Some(grammar) => grammar.highlight(&settings.source),
            None => Arc::new(Vec::new()),
        };
        self.line_starts = line_starts(&settings.source);
        self.current_line = 0;
        self.matches = settings.matches.clone();
        self.current_match = settings.current_match;
    }

    fn change_line(&mut self, line: usize) {
        self.current_line = line;
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        let line_index = self.current_line;
        self.current_line += 1;

        let Some(&start) = self.line_starts.get(line_index) else {
            return Vec::new().into_iter();
        };
        let end = start + line.len();

        let mut ranges: Vec<(Range<usize>, Highlighted)> = self
            .spans
            .iter()
            .filter(|span| span.start < end && span.end > start)
            .map(|span| {
                let range_start = span.start.max(start) - start;
                let range_end = span.end.min(end) - start;
                (range_start..range_end, Highlighted::Syntax(span.category))
            })
            .collect();

        // Appended *after* the syntax spans on purpose: iced feeds these to
        // `AttrsList::add_span`, whose range map overwrites on overlap, so
        // the last span covering a byte wins. Match coloring has to outrank
        // syntax coloring to be visible at all.
        //
        // Match columns are already line-relative, so unlike the syntax
        // spans above they need no offset arithmetic.
        ranges.extend(
            self.matches
                .iter()
                .enumerate()
                .filter(|(_, found)| found.line == line_index)
                .map(|(index, found)| {
                    let kind = if Some(index) == self.current_match {
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
    Binding::Sequence(vec![Binding::Select(Motion::Left.widen()), Binding::Backspace])
}

fn word_delete_forward() -> Binding<EditorMessage> {
    Binding::Sequence(vec![Binding::Select(Motion::Right.widen()), Binding::Delete])
}

/// Constructs the `Binding` for a named [`EditorCommand`] - shared by both
/// the user-override and hardcoded-default paths in `key_binding`.
fn binding_for(command: EditorCommand) -> Binding<EditorMessage> {
    match command {
        EditorCommand::WordDeleteBackward => word_delete_backward(),
        EditorCommand::WordDeleteForward => word_delete_forward(),
        EditorCommand::DocumentStart => Binding::Move(Motion::DocumentStart),
        EditorCommand::SelectDocumentStart => Binding::Select(Motion::DocumentStart),
        EditorCommand::DocumentEnd => Binding::Move(Motion::DocumentEnd),
        EditorCommand::SelectDocumentEnd => Binding::Select(Motion::DocumentEnd),
        EditorCommand::Undo => Binding::Custom(EditorMessage::Undo),
        EditorCommand::Redo => Binding::Custom(EditorMessage::Redo),
        EditorCommand::ToggleComment => Binding::Custom(EditorMessage::ToggleComment),
    }
}

/// Extends iced's default `text_editor` key bindings with the OS-standard
/// behaviors it doesn't already cover, then falls back to
/// [`Binding::from_key_press`] for everything else.
///
/// Three tiers, checked in order, first match wins:
/// 1. `overrides` - a user's `keybinds.toml` remap, matched by physical key.
/// 2. This crate's own hardcoded extras (word-delete, undo/redo, document start/end).
/// 3. iced's own stock default dispatch.
fn key_binding(press: KeyPress, overrides: &EditorOverrides) -> Option<Binding<EditorMessage>> {
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }

    // Tier 1: user override.
    if let key::Physical::Code(code) = press.physical_key {
        if let Some(&command) = overrides.get(&(press.modifiers, code)) {
            return Some(binding_for(command));
        }
    }

    // Tier 2: hardcoded extras.
    //
    // Option+Backspace (macOS) / Ctrl+Backspace (elsewhere): delete the
    // previous word. Option+Delete / Ctrl+Delete: delete the next word.
    if press.modifiers.jump() {
        match press.modified_key.as_ref() {
            keyboard::Key::Named(key::Named::Backspace) => {
                return Some(word_delete_backward());
            }
            keyboard::Key::Named(key::Named::Delete) => {
                return Some(word_delete_forward());
            }
            _ => {}
        }
    }

    // Cmd+Up/Down (Ctrl+Up/Down elsewhere): jump to document start/end;
    // with Shift held, select instead of just moving.
    if press.modifiers.command() {
        let motion = match press.modified_key.as_ref() {
            keyboard::Key::Named(key::Named::ArrowUp) => Some(Motion::DocumentStart),
            keyboard::Key::Named(key::Named::ArrowDown) => Some(Motion::DocumentEnd),
            _ => None,
        };
        if let Some(motion) = motion {
            return Some(if press.modifiers.shift() {
                Binding::Select(motion)
            } else {
                Binding::Move(motion)
            });
        }
    }

    // Cmd+Z / Ctrl+Z: undo. Cmd+Shift+Z or Cmd+Y (Ctrl+Shift+Z / Ctrl+Y
    // elsewhere): redo. iced has no undo history of its own, so these route
    // to this crate's own `History` via `EditorMessage::Undo`/`Redo`.
    if press.modifiers.command() {
        if let Some(c) = press.key.to_latin(press.physical_key) {
            match c {
                'z' if press.modifiers.shift() => {
                    return Some(binding_for(EditorCommand::Redo));
                }
                'z' => return Some(binding_for(EditorCommand::Undo)),
                'y' => return Some(binding_for(EditorCommand::Redo)),
                // On layouts where `/` needs a modifier (German: Shift+7)
                // this never fires - keybinds.toml overrides by physical
                // key and covers those.
                '/' => return Some(binding_for(EditorCommand::ToggleComment)),
                _ => {}
            }
        }
    }

    // Tier 3: iced's own stock dispatch - with one correction. On macOS,
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
    use super::*;

    fn press(modifiers: keyboard::Modifiers, physical: key::Code, key: keyboard::Key) -> KeyPress {
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
            SharedEditorConfig::new(1.0, EditorOverrides::new()),
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
        for doc in ["", "one line", "trailing\n", "a\nb\nc", "crlf\r\nlines\r\n"] {
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
        assert!(first == second, "unchanged settings must not re-run the highlighter");
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
    fn an_edit_rebuilds_the_cached_source() {
        let mut editor = plain_editor("hello");
        editor.move_cursor_to(0, 5);
        let before = editor.source.clone();

        let edited = editor.update(EditorMessage::Action(text_editor::Action::Edit(
            text_editor::Edit::Insert('!'),
        )));

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

    /// Selects `hello` in "hello world" as a plain drag-style range, cursor
    /// at the far end - the starting point for the undo tests below.
    fn editor_with_hello_selected() -> TextArea {
        let mut editor = plain_editor("hello world");
        editor.restore_selection(
            SavedSelection { anchor: (0, 0), kind: SelectionKind::Range },
            (0, 5),
        );
        assert_eq!(editor.content.selection().as_deref(), Some("hello"));
        editor
    }

    fn edit(edit: text_editor::Edit) -> EditorMessage {
        EditorMessage::Action(text_editor::Action::Edit(edit))
    }

    /// An editor opened as a `.rs` file with `// ` configured - what the
    /// toggle-comment tests run against.
    fn rust_editor(text: &str) -> TextArea {
        let registry = SyntaxRegistry::new(Vec::new(), HashMap::new(), || {});
        let settings = SharedEditorConfig::new(1.0, EditorOverrides::new());
        settings.set_comment_prefixes([("rs".to_string(), "// ".to_string())].into());
        TextArea::new(text, &registry, Some("rs"), settings)
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
            SavedSelection { anchor: (0, 1), kind: SelectionKind::Range },
            (1, 2),
        );

        assert!(editor.update(EditorMessage::ToggleComment));
        assert_eq!(editor.text(), "// aaa\n// bbb");
        assert_eq!(
            editor.selection(),
            Some(SavedSelection { anchor: (0, 4), kind: SelectionKind::Range })
        );
        assert_eq!(editor.cursor_position(), (1, 5));
        assert_source_is_synced(&editor);
    }

    #[test]
    fn a_selection_ending_at_column_zero_leaves_that_line_alone() {
        let mut editor = rust_editor("aaa\nbbb\nccc");
        editor.restore_selection(
            SavedSelection { anchor: (0, 0), kind: SelectionKind::Range },
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
        assert!(!editor.update(EditorMessage::Undo), "no phantom history entry");
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
            SavedSelection { anchor: (0, 1), kind: SelectionKind::Range },
            (1, 2),
        );
        assert!(editor.update(EditorMessage::ToggleComment));

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "aaa\nbbb");
        assert_eq!(
            editor.selection(),
            Some(SavedSelection { anchor: (0, 1), kind: SelectionKind::Range })
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
            Some(SavedSelection { anchor: (0, 0), kind: SelectionKind::Range })
        );
        assert_eq!(editor.cursor_position(), (0, 5));
    }

    #[test]
    fn undo_of_a_paste_over_a_selection_reselects_the_replaced_text() {
        let mut editor = editor_with_hello_selected();
        assert!(editor.update(edit(text_editor::Edit::Paste(Arc::new("bye".to_string())))));
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
        assert_eq!(editor.selection().map(|s| s.kind), Some(SelectionKind::Word));
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
        assert_eq!(editor.selection().map(|s| s.kind), Some(SelectionKind::Line));
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
        assert!(!editor.update(EditorMessage::Action(text_editor::Action::Select(
            Motion::Left.widen()
        ))));
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
        assert!(!Arc::ptr_eq(&after_edit, &editor.source), "undo changed the text");
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
        };
        assert!(settings("ab") != settings("ab"));
    }

    #[test]
    fn range_selection_round_trips_through_save_and_restore() {
        let mut editor = plain_editor("hello\nworld");
        let saved = SavedSelection { anchor: (0, 2), kind: SelectionKind::Range };
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
        assert_eq!(saved, SavedSelection { anchor: (0, 8), kind: SelectionKind::Word });

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
            SavedSelection { anchor: (0, 0), kind: SelectionKind::Range },
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
        assert_eq!(clamp_position(&content, (9, 9)), Position { line: 1, column: 2 });
        assert_eq!(clamp_position(&content, (0, 3)), Position { line: 0, column: 3 });
    }

    #[test]
    fn clamp_position_backs_up_to_a_char_boundary() {
        // 'é' occupies bytes 1..3, so byte offset 2 is mid-character.
        let content = Content::with_text("héllo");
        assert_eq!(clamp_position(&content, (0, 2)), Position { line: 0, column: 1 });
    }

    #[test]
    fn binding_for_covers_every_command() {
        assert!(matches!(
            binding_for(EditorCommand::WordDeleteBackward),
            Binding::Sequence(_)
        ));
        assert!(matches!(
            binding_for(EditorCommand::DocumentStart),
            Binding::Move(Motion::DocumentStart)
        ));
        assert!(matches!(
            binding_for(EditorCommand::SelectDocumentEnd),
            Binding::Select(Motion::DocumentEnd)
        ));
        assert!(matches!(binding_for(EditorCommand::Undo), Binding::Custom(EditorMessage::Undo)));
        assert!(matches!(binding_for(EditorCommand::Redo), Binding::Custom(EditorMessage::Redo)));
        assert!(matches!(
            binding_for(EditorCommand::ToggleComment),
            Binding::Custom(EditorMessage::ToggleComment)
        ));
    }

    #[test]
    fn command_slash_binds_toggle_comment_by_default() {
        // CTRL stands in for `Modifiers::command()` on non-mac test runners,
        // same as the undo/redo chord tests.
        let event = press(
            keyboard::Modifiers::CTRL,
            key::Code::Slash,
            keyboard::Key::Character("/".into()),
        );
        assert!(matches!(
            key_binding(event, &EditorOverrides::new()),
            Some(Binding::Custom(EditorMessage::ToggleComment))
        ));
    }

    #[test]
    fn an_override_on_command_slash_wins_over_toggle_comment() {
        let mut overrides = EditorOverrides::new();
        overrides.insert((keyboard::Modifiers::CTRL, key::Code::Slash), EditorCommand::Undo);
        let event = press(
            keyboard::Modifiers::CTRL,
            key::Code::Slash,
            keyboard::Key::Character("/".into()),
        );
        assert!(matches!(
            key_binding(event, &overrides),
            Some(Binding::Custom(EditorMessage::Undo))
        ));
    }

    #[test]
    fn override_wins_over_a_conflicting_hardcoded_default() {
        // Ctrl+Z would otherwise hit the hardcoded default's Undo binding -
        // override it onto Redo instead, and confirm the override wins.
        let mut overrides = EditorOverrides::new();
        overrides.insert(
            (keyboard::Modifiers::CTRL, key::Code::KeyZ),
            EditorCommand::Redo,
        );
        let event = press(
            keyboard::Modifiers::CTRL,
            key::Code::KeyZ,
            keyboard::Key::Character("z".into()),
        );
        assert!(matches!(
            key_binding(event, &overrides),
            Some(Binding::Custom(EditorMessage::Redo))
        ));
    }

    #[test]
    fn hardcoded_default_still_fires_with_no_overrides() {
        let overrides = EditorOverrides::new();
        let event = press(
            keyboard::Modifiers::CTRL,
            key::Code::KeyZ,
            keyboard::Key::Character("z".into()),
        );
        assert!(matches!(
            key_binding(event, &overrides),
            Some(Binding::Custom(EditorMessage::Undo))
        ));
    }

    #[test]
    fn command_held_unrecognized_character_does_not_fall_through_to_insert() {
        // `Modifiers::CTRL` stands in for `command()`, which resolves per-OS
        // at compile time - same convention the Undo/Redo tests use.
        let overrides = EditorOverrides::new();
        let event = press_with_text(
            keyboard::Modifiers::CTRL,
            key::Code::KeyN,
            keyboard::Key::Character("n".into()),
            Some("n"),
        );
        assert!(key_binding(event, &overrides).is_none());
    }

    #[test]
    fn plain_character_without_command_still_inserts_normally() {
        // Regression guard for the fix above: ordinary typing (no command
        // modifier held) must still work.
        let overrides = EditorOverrides::new();
        let event = press_with_text(
            keyboard::Modifiers::empty(),
            key::Code::KeyN,
            keyboard::Key::Character("n".into()),
            Some("n"),
        );
        assert!(matches!(key_binding(event, &overrides), Some(Binding::Insert('n'))));
    }

    #[test]
    fn unfocused_status_returns_none_regardless_of_overrides() {
        let mut overrides = EditorOverrides::new();
        overrides.insert(
            (keyboard::Modifiers::CTRL, key::Code::KeyZ),
            EditorCommand::Redo,
        );
        let mut event = press(
            keyboard::Modifiers::CTRL,
            key::Code::KeyZ,
            keyboard::Key::Character("z".into()),
        );
        event.status = Status::Active;
        assert!(key_binding(event, &overrides).is_none());
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
    fn editor_style_drops_its_background_when_translucent_but_keeps_the_value() {
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
    fn set_foreground_alpha_reaches_color_for() {
        // The only test that writes the global, restored at the end so the
        // parallel test threads never see a scaled alpha.
        let settings = SharedEditorConfig::new(1.0, EditorOverrides::new());
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
        let settings = SharedEditorConfig::new(0.8, EditorOverrides::new());
        assert_eq!(settings.background_alpha(), 0.8);
        settings.set_background_alpha(0.4);
        assert_eq!(settings.background_alpha(), 0.4);
        // Out-of-range input clamps rather than propagating.
        settings.set_background_alpha(2.0);
        assert_eq!(settings.background_alpha(), 1.0);

        assert!(settings.overrides().is_empty());
        let mut overrides = EditorOverrides::new();
        overrides.insert(
            (keyboard::Modifiers::CTRL, key::Code::KeyZ),
            EditorCommand::Undo,
        );
        settings.set_overrides(overrides);
        assert_eq!(
            settings.overrides().get(&(keyboard::Modifiers::CTRL, key::Code::KeyZ)),
            Some(&EditorCommand::Undo)
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
        })
    }

    #[test]
    fn highlight_line_emits_a_span_per_match_on_that_line() {
        let source = "find me\nand me";
        let matches = vec![
            FindMatch { line: 0, start: 5, end: 7 },
            FindMatch { line: 1, start: 4, end: 6 },
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
            vec![FindMatch { line: 0, start: 0, end: 7 }],
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
        let matches = Arc::new(vec![FindMatch { line: 0, start: 0, end: 2 }]);
        let base = HighlighterSettings {
            source: Arc::new("ab".to_string()),
            grammar: None,
            revision: 0,
            matches: matches.clone(),
            current_match: None,
            foreground_alpha: 1.0,
        };

        let same = HighlighterSettings { ..base.clone() };
        assert!(base == same);

        let moved_current = HighlighterSettings {
            current_match: Some(0),
            ..base.clone()
        };
        assert!(base != moved_current, "a new current match must invalidate");

        let other_matches = HighlighterSettings {
            matches: Arc::new(vec![FindMatch { line: 9, start: 1, end: 2 }]),
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
        };
        let loaded_something = HighlighterSettings { revision: 1, ..base.clone() };
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
        };
        let faded = HighlighterSettings { foreground_alpha: 0.5, ..base.clone() };
        assert!(base != faded);
    }

    #[test]
    fn set_find_matches_reaches_the_highlighter_settings() {
        let mut editor = plain_editor("find me");
        editor.set_find_matches(vec![FindMatch { line: 0, start: 5, end: 7 }], Some(0));
        assert_eq!(editor.find_matches.len(), 1);
        assert_eq!(editor.find_current, Some(0));

        editor.set_find_matches(Vec::new(), None);
        assert!(editor.find_matches.is_empty());
        assert_eq!(editor.find_current, None);
    }
}
