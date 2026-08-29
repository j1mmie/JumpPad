//! The column of line numbers down the left of a document.
//!
//! A wrapped line is one line, so only the row a line *begins* on carries a
//! number and the rows it wraps onto are left blank - which is what makes the
//! numbers count the document rather than the screen.
//!
//! Two things live here: the arithmetic of how wide the column is, and the
//! walk over the rows cosmic-text currently has on screen. Everything else -
//! where the column is drawn, what color, whether it is shown at all - is the
//! widget's business.

use iced::advanced::graphics::text::cosmic_text::Buffer;

/// How much text the column has to leave beside it, in characters. A window
/// narrow enough to fail this would spend most of its width on numbers and
/// wrap the document to a few characters a row, so the numbers go instead.
const MIN_TEXT_CHARACTERS: f32 = 8.0;

/// How far a row may sit from where the first row of its line would sit and
/// still be taken for it. Well under a pixel: the two are either the same row
/// or a whole row apart.
const ROW_TOP_TOLERANCE: f32 = 0.5;

/// One visual row on screen, and the logical line it belongs to.
///
/// `top` is measured from the top of the text, the same origin the document
/// itself is drawn at, and is negative for the row the top edge cuts through -
/// the view scrolls by pixels, so there nearly always is one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// The logical line, counted from zero. The number drawn is one more.
    pub line: usize,
    pub top: f32,
    pub height: f32,
    /// Whether this row begins its line, and so is the one that carries the
    /// number. False for every row a line wrapped onto.
    pub starts_line: bool,
}

/// The rows cosmic-text has on screen, in order, each tagged with whether it
/// begins its line.
///
/// Only the rows on screen are laid out, and `layout_runs` yields only those,
/// so this walks the view rather than the document however long the document
/// is.
///
/// The tagging is a comparison against the row before - except for the very
/// first row, which has none. `layout_runs` starts at the line the view is
/// scrolled into and drops that line's earlier rows without saying how many,
/// so the first row it yields is as likely to be a continuation as a
/// beginning. What settles it is the scroll itself: the first row of that
/// line is the one sitting exactly `scroll().vertical` above the text.
pub fn rows(buffer: &Buffer) -> impl Iterator<Item = Row> + '_ {
    let scrolled_into_line = buffer.scroll().vertical;
    let mut previous_line: Option<usize> = None;

    buffer.layout_runs().map(move |run| {
        let starts_line = match previous_line {
            Some(line) => line != run.line_i,
            None => {
                (run.line_top + scrolled_into_line).abs() < ROW_TOP_TOLERANCE
            }
        };
        previous_line = Some(run.line_i);

        Row {
            line: run.line_i,
            top: run.line_top,
            height: run.line_height,
            starts_line,
        }
    })
}

/// The blank either side of the numbers, in **characters** - the width of one
/// character of the document's own face.
///
/// Characters rather than pixels so a setting keeps its proportions at any
/// text size, and rather than a fixed unit so a column reads the same beside
/// any face. A proportional face has no one character width to speak of, so
/// the digit the numbers are made of stands in for it and the answer is a
/// fair guess rather than an exact one.
///
/// Read from the theme, and clamped here: this is on the path from a
/// hand-edited `config.toml`, where a bad number should look wrong rather
/// than break the layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Padding {
    /// Between the widget's own left edge and the first digit.
    left: f32,
    /// Between the last digit and the first character of every line.
    right: f32,
}

/// The blank either side of the numbers when no theme names one: a single
/// character, which reads as a column of numbers rather than as text jammed
/// against an edge.
pub const DEFAULT_PADDING: f32 = 1.0;

/// The range both paddings are held to. The ceiling is only there to keep a
/// typo'd config from spending the window on blank; zero is a real answer at
/// the floor - numbers hard against the text.
const PADDING_RANGE: std::ops::RangeInclusive<f32> = 0.0..=8.0;

impl Padding {
    /// A non-finite value falls back to the default rather than clamping -
    /// `NaN` has no meaningful end of the range. The same reasoning as
    /// `geometry::clamp_scroll_multiplier`.
    pub fn new(left: f32, right: f32) -> Self {
        Self {
            left: clamp(left),
            right: clamp(right),
        }
    }
}

impl Default for Padding {
    fn default() -> Self {
        Self::new(DEFAULT_PADDING, DEFAULT_PADDING)
    }
}

fn clamp(characters: f32) -> f32 {
    if characters.is_finite() {
        characters.clamp(*PADDING_RANGE.start(), *PADDING_RANGE.end())
    } else {
        DEFAULT_PADDING
    }
}

/// How wide the numbers are drawn, and how much room they take from the text.
///
/// All pixels by the time it gets here. The numbers' own width is a
/// measurement rather than arithmetic on the font size - a digit's width is
/// the typeface's business, and the column has to fit the numbers whatever
/// face the document is set in - and that measurement is also what a
/// character of [`Padding`] is worth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    digit_width: f32,
    numbers_width: f32,
    padding_left: f32,
    padding_right: f32,
}

impl Column {
    /// A column for `digits` digits, which measure `digits_width` together in
    /// the document's own font.
    ///
    /// No text size to be told: one digit of that measurement is the
    /// character the padding counts in, so the whole column is settled by
    /// what the face actually draws.
    pub fn new(digits: u32, digits_width: f32, padding: Padding) -> Self {
        let digits_width = digits_width.max(0.0);
        let digit_width = digits_width / digits.max(1) as f32;

        Self {
            digit_width,
            numbers_width: digits_width,
            padding_left: padding.left * digit_width,
            padding_right: padding.right * digit_width,
        }
    }

    /// The digits the widest number in the document needs.
    pub fn digits_for(line_count: usize) -> u32 {
        line_count.max(1).ilog10() + 1
    }

    /// What the column takes from the text: the numbers, and the blank either
    /// side of them.
    pub fn width(&self) -> f32 {
        self.padding_left + self.numbers_width + self.padding_right
    }

    /// What is left for the text of a strip `width` wide once this column
    /// has taken its room from the left of it.
    pub fn text_width(&self, width: f32) -> f32 {
        (width - self.width()).max(0.0)
    }

    /// Where a number `digits` long starts, given the column's left edge, so
    /// that it ends flush with every other number in the column - the only
    /// way a hundred lines below a thousand line up.
    ///
    /// Worked out here rather than left to the renderer's own right-aligning,
    /// which positions the text by its right edge and then damages the
    /// rectangle to the *right* of it: a region it never paints, while the
    /// numbers themselves are never repainted.
    pub fn number_left_edge(&self, digits: u32, left: f32) -> f32 {
        left + self.padding_left + self.numbers_width
            - digits as f32 * self.digit_width
    }

    /// Whether a text area this wide can spare the column. See
    /// [`MIN_TEXT_CHARACTERS`].
    pub fn leaves_room_in(&self, text_width: f32) -> bool {
        text_width - self.width() >= MIN_TEXT_CHARACTERS * self.digit_width
    }
}

#[cfg(test)]
#[path = "line_numbers_tests.rs"]
mod tests;
