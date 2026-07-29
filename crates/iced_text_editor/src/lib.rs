mod history;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use editor_core::{EditorMessage, TextEditorWidget};
use history::History;
use iced::advanced::text::Highlighter;
use iced::advanced::text::highlighter::Format;
use iced::keyboard::{self, key};
use iced::widget::text_editor;
use iced::widget::text_editor::{Binding, Content, KeyPress, Motion, Status};
use iced::{Border, Element, Fill, Font, Theme};
use syntax_registry::{Grammar, Handle, HighlightCategory, PollResult, SyntaxRegistry};

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
];

/// A user override's resolved chord, keyed by physical key (layout-
/// independent) - unlike this crate's pre-existing hardcoded bindings
/// below, which stay logical-key based.
pub type EditorOverrides = HashMap<(keyboard::Modifiers, key::Code), EditorCommand>;

/// A [`TextEditorWidget`] backed by `iced::widget::text_editor`, with
/// optional tree-sitter/WASM syntax highlighting layered on via a
/// [`syntax_registry::SyntaxRegistry`].
pub struct IcedTextEditor {
    content: Content,
    highlighting: Highlighting,
    history: History,
    overrides: Arc<EditorOverrides>,
    /// Scales the editor's own background color (see `editor_style`).
    /// Captured per-instance, unlike `foreground_alpha`, since it's only
    /// used inside a `.style()` closure that's allowed to capture state.
    background_alpha: f32,
}

/// Scales every syntax-highlighted color `color_for` produces. Global
/// rather than a field like `background_alpha` because
/// `text_editor::highlight_with`'s `to_format` callback must be a bare `fn`
/// pointer - it can't capture state, so `color_for` has nothing else to
/// read this from. Set once in `factory`, before any editor is constructed.
static FOREGROUND_ALPHA: OnceLock<f32> = OnceLock::new();

fn foreground_alpha() -> f32 {
    *FOREGROUND_ALPHA.get().unwrap_or(&1.0)
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

impl IcedTextEditor {
    pub fn new(
        text: &str,
        registry: &Arc<SyntaxRegistry>,
        extension: Option<&str>,
        overrides: Arc<EditorOverrides>,
        background_alpha: f32,
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
            background_alpha: background_alpha.clamp(0.0, 1.0),
        }
    }

    /// Builds an [`editor_core::EditorFactory`]-shaped closure that
    /// captures a shared syntax registry and the resolved keybind overrides
    /// once, and sets `FOREGROUND_ALPHA`.
    pub fn factory(
        registry: Arc<SyntaxRegistry>,
        overrides: Arc<EditorOverrides>,
        background_alpha: f32,
        foreground_alpha: f32,
    ) -> impl Fn(&str, Option<&str>) -> Box<dyn TextEditorWidget> {
        let _ = FOREGROUND_ALPHA.set(foreground_alpha.clamp(0.0, 1.0));
        move |text, extension| {
            Box::new(Self::new(text, &registry, extension, overrides.clone(), background_alpha))
        }
    }

    fn grammar(&self) -> Option<Arc<Grammar>> {
        match &self.highlighting {
            Highlighting::Ready(_, grammar) => Some(grammar.clone()),
            _ => None,
        }
    }

    /// Shared by `EditorMessage::Undo`/`Redo`: hands the current text and
    /// cursor to `op` and, if it returns a state to restore, replaces the
    /// content wholesale and moves the cursor back. Returns whether an edit
    /// actually happened, same contract `update` has for `Action`s.
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
        let background_alpha = self.background_alpha;
        let foreground_alpha = foreground_alpha();
        text_editor(&self.content)
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
        let position = self.content.cursor().position;
        (position.line, position.column)
    }

    fn move_cursor_to(&mut self, line: usize, column: usize) {
        // No absolute "jump to (line, column)" action exists, so this walks
        // there from document start - each step clamps to the nearest
        // valid position if the document's since gotten shorter.
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
/// grammar to highlight with (if any) and the entire current document text -
/// `highlight_line` only sees one line at a time, but tree-sitter needs the
/// whole buffer to parse.
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
    apply_alpha(base_color_for(category), foreground_alpha())
}

/// Scales `color`'s alpha by `alpha`, skipping the multiply at `1.0`.
fn apply_alpha(color: iced::Color, alpha: f32) -> iced::Color {
    if alpha >= 1.0 {
        color
    } else {
        color.scale_alpha(alpha)
    }
}

fn base_color_for(category: HighlightCategory) -> iced::Color {
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
/// Also scales the background by `background_alpha` and the base text
/// color by `foreground_alpha` (both plain parameters, for testability -
/// syntax-highlighted text instead goes through `color_for`).
fn editor_style(
    theme: &Theme,
    status: text_editor::Status,
    background_alpha: f32,
    foreground_alpha: f32,
) -> text_editor::Style {
    let default = text_editor::default(theme, status);
    let background = if background_alpha >= 1.0 {
        default.background
    } else {
        default.background.scale_alpha(background_alpha)
    };
    let value = apply_alpha(default.value, foreground_alpha);
    text_editor::Style {
        border: Border {
            width: 0.0,
            ..Border::default()
        },
        background,
        value,
        ..default
    }
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
    fn editor_style_scales_background_and_value_independently() {
        let theme = Theme::ALL[0].clone();
        let default = text_editor::default(&theme, text_editor::Status::Active);
        let style = editor_style(&theme, text_editor::Status::Active, 0.5, 1.0);
        assert_ne!(style.background, default.background);
        assert_eq!(style.value, default.value); // foreground untouched

        let style = editor_style(&theme, text_editor::Status::Active, 1.0, 0.3);
        assert_eq!(style.background, default.background); // background untouched
        assert_ne!(style.value, default.value);
    }

    #[test]
    fn color_for_scales_by_whatever_foreground_alpha_is_currently_set() {
        // `FOREGROUND_ALPHA.set` only takes effect once per process, so
        // this derives the expected value from its actual current reading
        // rather than assuming 0.25 won.
        let _ = FOREGROUND_ALPHA.set(0.25);
        let alpha = foreground_alpha();
        let color = color_for(HighlightCategory::Keyword);
        let base = base_color_for(HighlightCategory::Keyword);
        assert_eq!((color.r, color.g, color.b), (base.r, base.g, base.b));
        let expected_alpha = if alpha >= 1.0 { base.a } else { base.a * alpha };
        assert!((color.a - expected_alpha).abs() < f32::EPSILON);
    }
}
