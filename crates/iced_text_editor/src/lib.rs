use std::ops::Range;
use std::sync::Arc;

use editor_core::{EditorMessage, TextEditorWidget};
use iced::advanced::text::Highlighter;
use iced::advanced::text::highlighter::Format;
use iced::widget::text_editor;
use iced::widget::text_editor::Content;
use iced::{Border, Element, Fill, Font, Theme};
use syntax_registry::{Grammar, Handle, HighlightCategory, PollResult, SyntaxRegistry};

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
    pub fn new(text: &str, registry: &Arc<SyntaxRegistry>, extension: Option<&str>) -> Self {
        let highlighting = match extension {
            None => Highlighting::None,
            Some(ext) => Highlighting::Pending(registry.acquire(ext)),
        };
        Self {
            content: Content::with_text(text),
            highlighting,
        }
    }

    /// Builds an [`editor_core::EditorFactory`]-shaped closure that
    /// captures a shared syntax registry once. This is the one line the
    /// app shell changes to swap editor backends.
    pub fn factory(
        registry: Arc<SyntaxRegistry>,
    ) -> impl Fn(&str, Option<&str>) -> Box<dyn TextEditorWidget> {
        move |text, extension| Box::new(Self::new(text, &registry, extension))
    }

    fn grammar(&self) -> Option<Arc<Grammar>> {
        match &self.highlighting {
            Highlighting::Ready(_, grammar) => Some(grammar.clone()),
            _ => None,
        }
    }
}

impl TextEditorWidget for IcedTextEditor {
    fn view(&self) -> Element<'_, EditorMessage> {
        let settings = HighlighterSettings {
            source: self.content.text(),
            grammar: self.grammar(),
        };
        text_editor(&self.content)
            .placeholder("")
            .font(Font::MONOSPACE)
            .height(Fill)
            .style(borderless)
            .highlight_with::<TreeSitterHighlighter>(settings, to_format)
            .on_action(EditorMessage::Action)
            .into()
    }

    fn update(&mut self, message: EditorMessage) -> bool {
        let EditorMessage::Action(action) = message;
        let is_edit = action.is_edit();
        self.content.perform(action);
        is_edit
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
        self.content.cursor_position()
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
