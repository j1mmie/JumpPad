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

/// The fewest digits the column is ever drawn at. Without a floor the column
/// would widen the first time a document reached ten lines, shifting every
/// line of text sideways as the file was typed into.
pub const MIN_DIGIT_COUNT: u32 = 3;

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

/// How wide the numbers are drawn, and how much room they take from the text.
///
/// Built from a measurement rather than the font size: a digit's width is the
/// typeface's business, and the column has to fit the numbers whatever face
/// the document is set in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    digits: u32,
    numbers_width: f32,
}

impl Column {
    /// A column `digits` wide, where that many digits measure `numbers_width`
    /// in the document's own font.
    pub fn new(digits: u32, numbers_width: f32) -> Self {
        Self {
            digits: digits.max(1),
            numbers_width: numbers_width.max(0.0),
        }
    }

    /// The digits the widest number in the document needs, floored at
    /// [`MIN_DIGIT_COUNT`].
    pub fn digits_for(line_count: usize) -> u32 {
        (line_count.max(1).ilog10() + 1).max(MIN_DIGIT_COUNT)
    }

    /// What the column takes from the text: the numbers, plus one blank digit
    /// between them and the first character of every line. A gap measured in
    /// digits rather than pixels keeps its proportions at any text size.
    pub fn width(&self) -> f32 {
        self.numbers_width + self.digit_width()
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
        left + self.numbers_width - digits as f32 * self.digit_width()
    }

    /// Whether a text area this wide can spare the column. See
    /// [`MIN_TEXT_CHARACTERS`].
    pub fn leaves_room_in(&self, text_width: f32) -> bool {
        text_width - self.width() >= MIN_TEXT_CHARACTERS * self.digit_width()
    }

    /// One digit's width, which is also the width of the blank column
    /// between the numbers and the text.
    fn digit_width(&self) -> f32 {
        self.numbers_width / self.digits as f32
    }
}

#[cfg(test)]
#[path = "line_numbers_tests.rs"]
mod tests;
