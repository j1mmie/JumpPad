use std::ops::Range;
use std::sync::Arc;

use editor_core::FindMatch;
use iced::advanced::text::Highlighter;
use iced::advanced::text::highlighter::Format;
use iced::{Font, Theme};
use syntax_registry::{Grammar, HighlightCategory};

use crate::shared_config::foreground_alpha;
use crate::style::apply_alpha;

/// The settings a [`TreeSitterHighlighter`] is built/updated from: the
/// grammar to highlight with (if any) and the entire current document text -
/// `highlight_line` only sees one line at a time, but tree-sitter needs the
/// whole buffer to parse.
#[derive(Clone)]
pub(crate) struct HighlighterSettings {
    /// Shared with the [`crate::TextArea`] this was built from - see its
    /// `source` field. Cloning these settings (which the widget does on
    /// every layout that changes them) copies a refcount, not the document.
    pub(crate) source: Arc<String>,
    pub(crate) grammar: Option<Arc<Grammar>>,
    /// The registry's load revision. The grammar `Arc` stays the same object
    /// as its injection targets resolve, so without this the settings would
    /// still compare equal and the incomplete first parse would stay on
    /// screen until an unrelated edit changed `source`.
    pub(crate) revision: u64,
    pub(crate) matches: Arc<Vec<FindMatch>>,
    pub(crate) current_match: Option<usize>,
    /// Not read by the highlighter itself - `to_format` resolves colors
    /// through `FOREGROUND_ALPHA` directly. It rides along so a config
    /// reload makes the settings compare unequal (see `PartialEq` below).
    pub(crate) foreground_alpha: f32,
    /// Outside `PartialEq`: it describes the change, not the state.
    pub(crate) edited_from: Option<usize>,
}

impl HighlighterSettings {
    /// Whether the text is the only thing that moved. Anything else can
    /// recolor a line no edit went near.
    pub(crate) fn only_the_text_moved_since(&self, previous: &Self) -> bool {
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
pub(crate) enum Highlighted {
    Syntax(HighlightCategory),
    Match,
    CurrentMatch,
}

pub(crate) struct TreeSitterHighlighter {
    pub(crate) spans: Arc<Vec<syntax_registry::HighlightSpan>>,
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

pub(crate) fn to_format(highlighted: &Highlighted, _theme: &Theme) -> Format<Font> {
    Format {
        color: Some(color_for(*highlighted)),
        font: None,
    }
}

pub(crate) fn color_for(highlighted: Highlighted) -> iced::Color {
    apply_alpha(base_color_for(highlighted), foreground_alpha())
}

/// Find matches are recolored rather than given a highlight box: iced's
/// `highlighter::Format` carries only a color and a font, with no background
/// to fill. The current match is the brighter of the two so it stands out
/// from its neighbours.
const MATCH_COLOR: iced::Color = iced::Color::from_rgb(0.85, 0.62, 0.24);
const CURRENT_MATCH_COLOR: iced::Color = iced::Color::from_rgb(1.0, 0.85, 0.35);

pub(crate) fn base_color_for(highlighted: Highlighted) -> iced::Color {
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
