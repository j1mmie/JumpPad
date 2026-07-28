mod history;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use editor_core::{EditorMessage, TextEditorWidget};
use history::History;
use iced::advanced::text::Highlighter;
use iced::advanced::text::highlighter::Format;
use iced::keyboard::{self, key};
use iced::widget::text_editor;
use iced::widget::text_editor::{Binding, Content, KeyPress, Motion, Status};
use iced::{Border, Element, Fill, Font, Theme};
use syntax_registry::{Grammar, Handle, HighlightCategory, PollResult, SyntaxRegistry};

/// A named editor-level action a `keybinds.toml` override can target - see
/// `xizor_config::KeybindsConfig::overrides`'s doc comment. Deliberately a
/// small, closed set (not every possible motion): these are the commands
/// this crate already has custom logic for beyond iced's own stock
/// `text_editor` defaults (copy/cut/paste/select-all/plain motions/etc,
/// which stay non-configurable - nobody's asked to remap those, and they're
/// already identical on every OS via iced's own `command()` gating).
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
}

/// The canonical `keybinds.toml` override name for each [`EditorCommand`] -
/// the single source of truth both `key_binding`'s override lookup and
/// `xizor_config::KeybindsConfig::overrides`'s doc comment point at.
pub const EDITOR_COMMAND_NAMES: &[(&str, EditorCommand)] = &[
    ("word_delete_backward", EditorCommand::WordDeleteBackward),
    ("word_delete_forward", EditorCommand::WordDeleteForward),
    ("document_start", EditorCommand::DocumentStart),
    ("select_document_start", EditorCommand::SelectDocumentStart),
    ("document_end", EditorCommand::DocumentEnd),
    ("select_document_end", EditorCommand::SelectDocumentEnd),
    ("undo", EditorCommand::Undo),
    ("redo", EditorCommand::Redo),
];

/// A user override's resolved chord, keyed by the same `(Modifiers,
/// key::Code)` pair `key_binding` matches incoming presses against -
/// physical-key based (layout-independent), unlike this crate's other,
/// pre-existing hardcoded bindings below, which stay logical-key based
/// (layout-dependent) exactly as already shipped. See the plan doc for why
/// that inconsistency is deliberate and scoped rather than an oversight.
pub type EditorOverrides = HashMap<(keyboard::Modifiers, key::Code), EditorCommand>;

/// A [`TextEditorWidget`] backed by `iced::widget::text_editor`, with
/// optional tree-sitter/WASM syntax highlighting layered on top via a
/// [`syntax_registry::SyntaxRegistry`].
///
/// This is the default/first editor implementation. The highlighting
/// machinery lives in `syntax_registry` (which has no `iced` dependency),
/// so a future, different `TextEditorWidget` implementation could reuse it
/// without depending on this crate.
pub struct IcedTextEditor {
    content: Content,
    highlighting: Highlighting,
    history: History,
    overrides: Arc<EditorOverrides>,
}

enum Highlighting {
    /// No file extension is known for this tab - never highlighted.
    None,
    /// A grammar load was requested; still waiting on it.
    Pending(Handle),
    /// Grammar loaded and ready to parse/highlight with. The `Handle` must
    /// be kept alive here (not just while `Pending`) - dropping it releases
    /// the registry's reservation, which would evict the grammar the
    /// moment it finished loading and defeat the whole reuse/cache point.
    Ready(#[allow(dead_code)] Handle, Arc<Grammar>),
    /// Grammar search finished with nothing usable - stop polling. Still
    /// holds the `Handle` so a second tab with the same extension reuses
    /// this cached "nothing to find" result instead of re-searching.
    Unavailable(#[allow(dead_code)] Handle),
}

impl IcedTextEditor {
    pub fn new(
        text: &str,
        registry: &Arc<SyntaxRegistry>,
        extension: Option<&str>,
        overrides: Arc<EditorOverrides>,
    ) -> Self {
        let highlighting = match extension {
            None => Highlighting::None,
            Some(ext) => Highlighting::Pending(registry.acquire(ext)),
        };
        Self {
            content: Content::with_text(text),
            highlighting,
            history: History::new(),
            overrides,
        }
    }

    /// Builds an [`editor_core::EditorFactory`]-shaped closure that
    /// captures a shared syntax registry and the resolved keybind overrides
    /// once. This is the one line the app shell changes to swap editor
    /// backends.
    pub fn factory(
        registry: Arc<SyntaxRegistry>,
        overrides: Arc<EditorOverrides>,
    ) -> impl Fn(&str, Option<&str>) -> Box<dyn TextEditorWidget> {
        move |text, extension| Box::new(Self::new(text, &registry, extension, overrides.clone()))
    }

    fn grammar(&self) -> Option<Arc<Grammar>> {
        match &self.highlighting {
            Highlighting::Ready(_, grammar) => Some(grammar.clone()),
            _ => None,
        }
    }

    /// Shared by `EditorMessage::Undo`/`Redo`: hands the current text and
    /// cursor to `op` (`History::undo` or `History::redo`) and, if it
    /// returns a state to restore, replaces the content wholesale and walks
    /// the cursor back to the recorded position. Returns whether an edit
    /// actually happened (`false` when there was nothing to undo/redo), the
    /// same contract `update` has for `Action`s.
    fn apply_history(
        &mut self,
        op: impl FnOnce(&mut History, &str, (usize, usize)) -> Option<(String, (usize, usize))>,
    ) -> bool {
        let text = self.content.text();
        let cursor = self.cursor_position();
        let Some((restored_text, restored_cursor)) = op(&mut self.history, &text, cursor) else {
            return false;
        };
        self.content = Content::with_text(&restored_text);
        self.move_cursor_to(restored_cursor.0, restored_cursor.1);
        true
    }
}

impl TextEditorWidget for IcedTextEditor {
    fn view(&self) -> Element<'_, EditorMessage> {
        let settings = HighlighterSettings {
            source: self.content.text(),
            grammar: self.grammar(),
        };
        let overrides = self.overrides.clone();
        text_editor(&self.content)
            .placeholder("")
            .font(Font::MONOSPACE)
            .height(Fill)
            .style(borderless)
            .highlight_with::<TreeSitterHighlighter>(settings, to_format)
            .key_binding(move |press| key_binding(press, &overrides))
            .on_action(EditorMessage::Action)
            .into()
    }

    fn update(&mut self, message: EditorMessage) -> bool {
        match message {
            EditorMessage::Action(action) => {
                let is_edit = action.is_edit();
                if is_edit {
                    self.history
                        .record_before_edit(&self.content.text(), self.cursor_position());
                }
                self.content.perform(action);
                is_edit
            }
            EditorMessage::Undo => self.apply_history(History::undo),
            EditorMessage::Redo => self.apply_history(History::redo),
        }
    }

    fn text(&self) -> String {
        self.content.text()
    }

    fn set_text(&mut self, text: &str) {
        self.content = Content::with_text(text);
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
        matches!(self.highlighting, Highlighting::Pending(_))
    }

    fn cursor_position(&self) -> (usize, usize) {
        // `Content::cursor_position()` (iced 0.13) was replaced by
        // `cursor()` returning a `text_editor::Cursor { position, selection }`
        // - only the plain caret position is needed here, not any selection.
        let position = self.content.cursor().position;
        (position.line, position.column)
    }

    fn move_cursor_to(&mut self, line: usize, column: usize) {
        // No absolute "jump to (line, column)" action exists, so this walks
        // there from the start: document start, then down `line` times,
        // then right `column` times. Each step clamps at the nearest valid
        // position rather than erroring, so a document that's gotten
        // shorter since `line`/`column` were recorded just lands at the
        // closest place instead of panicking or doing nothing.
        self.content
            .perform(text_editor::Action::Move(text_editor::Motion::DocumentStart));
        for _ in 0..line {
            self.content
                .perform(text_editor::Action::Move(text_editor::Motion::Down));
        }
        for _ in 0..column {
            self.content
                .perform(text_editor::Action::Move(text_editor::Motion::Right));
        }
    }
}

/// The settings a [`TreeSitterHighlighter`] is built/updated from: the
/// grammar to highlight with (if any) and the *entire* current document
/// text. iced's `Highlighter::highlight_line` only ever sees one line at a
/// time, but tree-sitter needs the whole buffer to parse - so the full text
/// is threaded through here instead, and `highlight_line` slices into
/// spans computed up front in `new`/`update`.
#[derive(Clone)]
struct HighlighterSettings {
    source: String,
    grammar: Option<Arc<Grammar>>,
}

impl PartialEq for HighlighterSettings {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && match (&self.grammar, &other.grammar) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

struct TreeSitterHighlighter {
    spans: Arc<Vec<syntax_registry::HighlightSpan>>,
    /// Byte offset of the start of each line within `source`, in order.
    line_starts: Vec<usize>,
    current_line: usize,
}

impl Highlighter for TreeSitterHighlighter {
    type Settings = HighlighterSettings;
    type Highlight = HighlightCategory;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, HighlightCategory)>;

    fn new(settings: &Self::Settings) -> Self {
        let mut highlighter = Self {
            spans: Arc::new(Vec::new()),
            line_starts: vec![0],
            current_line: 0,
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

        self.spans
            .iter()
            .filter(|span| span.start < end && span.end > start)
            .map(|span| {
                let range_start = span.start.max(start) - start;
                let range_end = span.end.min(end) - start;
                (range_start..range_end, span.category)
            })
            .collect::<Vec<_>>()
            .into_iter()
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

fn to_format(category: &HighlightCategory, _theme: &Theme) -> Format<Font> {
    Format {
        color: Some(color_for(*category)),
        font: None,
    }
}

fn color_for(category: HighlightCategory) -> iced::Color {
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
/// the user-override path and this crate's own hardcoded-default path in
/// `key_binding`, so the two never drift out of sync with each other.
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
    }
}

/// Extends iced's default `text_editor` key bindings with the OS-standard
/// behaviors it doesn't already cover, then falls back to
/// [`Binding::from_key_press`] (iced's own default dispatch) for everything
/// else. Supplying a custom `key_binding` closure at all replaces default
/// dispatch entirely rather than layering on top of it, so this must
/// explicitly re-run the default for any key combination it doesn't itself
/// care about - including the focus guard `from_key_press` would otherwise
/// apply internally.
///
/// Three tiers, checked in order, first match wins - no layering/conflict
/// system beyond that fixed order:
/// 1. `overrides` - a user's `keybinds.toml` remap, matched by *physical*
///    key (layout-independent - see `EditorOverrides`'s doc comment for why
///    this deliberately differs from tier 2's matching style).
/// 2. This crate's own hardcoded extras (word-delete, undo/redo, document
///    start/end) - unchanged from before overrides existed, still keyed off
///    iced's portable `command()`/`jump()` modifier predicates (Cmd/Ctrl and
///    Option/Ctrl respectively, already resolved per-OS by iced itself)
///    rather than `#[cfg(target_os = ...)]`.
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

    // Tier 2: this repo's existing hardcoded extras.
    //
    // Option+Backspace (macOS) / Ctrl+Backspace (elsewhere): delete the
    // previous word. Option+Delete / Ctrl+Delete: delete the next word.
    // iced's own default bindings only ever delete one character - `jump()`
    // is the same modifier iced's built-in word-movement logic already uses
    // to widen `Left`/`Right` motions to `WordLeft`/`WordRight`, reused here
    // so word-delete uses the identical "select the word, then delete the
    // selection" shape as a manual select-then-backspace would.
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

    // Cmd+Up/Down: jump to the start/end of the document (the actual macOS
    // convention - iced's own `macos_command()` handling only remaps
    // Left/Right this way, not Up/Down). With Shift held, select instead of
    // just moving. `command()` is the portable helper (Cmd on macOS, Ctrl
    // elsewhere), so this is Ctrl+Up/Down on other platforms - an unclaimed,
    // harmless combo there.
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
    // elsewhere): redo. This exact iced version (0.14.0) has no undo history
    // of its own (confirmed absent from its `Binding` enum), so these are
    // routed to `EditorMessage::Undo`/`Redo`, handled by this crate's own
    // `History` in `update()`. Redo answering to both Shift+Z and Y keeps
    // this one portable `command()`-gated path instead of a macOS-only
    // special case for the Shift+Z convention.
    if press.modifiers.command() {
        if let Some(c) = press.key.to_latin(press.physical_key) {
            match c {
                'z' if press.modifiers.shift() => {
                    return Some(binding_for(EditorCommand::Redo));
                }
                'z' => return Some(binding_for(EditorCommand::Undo)),
                'y' => return Some(binding_for(EditorCommand::Redo)),
                _ => {}
            }
        }
    }

    // Tier 3: iced's own stock dispatch.
    Binding::from_key_press(press)
}

/// iced's default `text_editor` style draws a border that changes color on
/// hover/focus - drop the border in every status instead of just one, so
/// there's no color-change effect left to notice.
fn borderless(theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    text_editor::Style {
        border: Border {
            width: 0.0,
            ..Border::default()
        },
        ..text_editor::default(theme, status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(modifiers: keyboard::Modifiers, physical: key::Code, key: keyboard::Key) -> KeyPress {
        KeyPress {
            key: key.clone(),
            modified_key: key,
            physical_key: key::Physical::Code(physical),
            modifiers,
            text: None,
            status: Status::Focused { is_hovered: false },
        }
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
}
