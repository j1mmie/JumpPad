use iced::advanced::graphics;
use iced_core::{Padding, Point, Rectangle, Vector, mouse};

use crate::{line_numbers, scrollbar};

/// How much of the widget sits above and to the left of its text: the
/// padding, and the line numbers the padding's left edge is followed by.
///
/// Both together are the inset every coordinate in the text is measured
/// against - the wrap width, the draw origin, a pointer's position, the
/// scrollbar's track. They have to agree, or the text draws somewhere other
/// than where clicking it lands.
#[derive(Debug, Clone, Copy)]
pub(super) struct TextInset {
    padding: Padding,
    line_numbers: Option<line_numbers::Column>,
}

impl TextInset {
    pub(super) fn new(
        padding: Padding,
        line_numbers: Option<line_numbers::Column>,
    ) -> Self {
        Self {
            padding,
            line_numbers,
        }
    }

    /// The widget's padding with the numbers' room folded into its left
    /// edge, which is what a position in the text is measured from.
    pub(super) fn padding(&self) -> Padding {
        Padding {
            left: self.padding.left + self.line_numbers_width(),
            ..self.padding
        }
    }

    /// Everything inside the widget's own padding: the text, and the line
    /// numbers beside it.
    pub(super) fn inside(&self, bounds: Rectangle) -> Rectangle {
        bounds.shrink(self.padding)
    }

    /// The rectangle the document's text alone is laid out and drawn in.
    pub(super) fn text_bounds(&self, bounds: Rectangle) -> Rectangle {
        let inside = self.inside(bounds);
        let taken = self.line_numbers_width();

        Rectangle {
            x: inside.x + taken,
            width: (inside.width - taken).max(0.0),
            ..inside
        }
    }

    /// Whether a position already measured from the text's own origin landed
    /// on a line number rather than on a character. Only the numbers are to
    /// the left of that origin at a negative offset this small - the padding
    /// beyond them is further out still.
    pub(super) fn is_on_line_numbers(&self, position: Point) -> bool {
        (-self.line_numbers_width()..0.0).contains(&position.x)
    }

    /// The line numbers themselves, if the document is numbered.
    pub(super) fn line_numbers(&self) -> Option<line_numbers::Column> {
        self.line_numbers
    }

    pub(super) fn line_numbers_width(&self) -> f32 {
        self.line_numbers.map_or(0.0, |column| column.width())
    }
}

/// The scrollbar's geometry for wherever the document currently sits, or
/// `None` if it has no line height to measure against yet. Read through the
/// scrollbar's own state, which is where the document's height is kept between
/// events.
pub(super) fn scrollbar_layout(
    state: &scrollbar::State,
    editor: &graphics::text::Editor,
    text_bounds: Rectangle,
    width: f32,
) -> Option<scrollbar::Layout> {
    let metrics = state.metrics(editor.buffer(), text_bounds.size())?;
    Some(scrollbar::Layout::new(text_bounds, metrics, width))
}

/// Lines scrolled per notch of a discrete wheel, at `sensitivity == 1.0`.
/// Upstream `iced_widget` uses 4.0; JumpPad ships half that, and puts the
/// rest of the range behind `[scroll] sensitivity` in `config.toml`.
pub(super) const LINES_PER_WHEEL_NOTCH: f32 = 2.0;

/// Pixels of a precise (trackpad, or a free-spinning wheel) delta that make
/// one line, at `sensitivity == 1.0`. Upstream divides by 4.0; doubling the
/// divisor halves the speed, to match `LINES_PER_WHEEL_NOTCH`.
pub(super) const PIXELS_PER_LINE: f32 = 8.0;

/// The range `[scroll] sensitivity` and `[scroll] drag_speed` are held to.
/// The ceiling is only there to keep a typo'd config from making scrolling
/// useless; the floor is above zero so it never stops entirely.
pub(super) const SCROLL_MULTIPLIER_RANGE: std::ops::RangeInclusive<f32> =
    0.05..=20.0;

/// One wheel or trackpad event, in lines to scroll down by - fractional, so
/// a sensitivity below `1.0` doesn't round every event to a standstill. The
/// caller banks the fraction (`State::partial_scroll`) until it makes a whole
/// line, which is the only unit `Action::Scroll` can carry.
pub(super) fn wheel_lines(delta: mouse::ScrollDelta, sensitivity: f32) -> f32 {
    sensitivity
        * match delta {
            // A discrete wheel: `y` is notches, and the floor keeps a
            // fraction of a notch from reading as no scroll at all.
            mouse::ScrollDelta::Lines { y, .. } => {
                if y.abs() > 0.0 {
                    y.signum() * -(y.abs() * LINES_PER_WHEEL_NOTCH).max(1.0)
                } else {
                    0.0
                }
            }
            // A precise device: `y` is already pixels of intended travel.
            mouse::ScrollDelta::Pixels { y, .. } => -y / PIXELS_PER_LINE,
        }
}

/// Where the pointer sits relative to the text, with the widget's own
/// position and padding taken off - the coordinates every editor action is
/// written in.
///
/// Deliberately unclamped, unlike a hit test for a click: a selection drag
/// keeps following a pointer that has left the widget, and the window with
/// it, and the editor resolves a position outside the text against its
/// nearest row. `None` only when the pointer's position is unknown, which is
/// what the platform reports when it isn't on this window at all.
pub(super) fn text_position(
    cursor: mouse::Cursor,
    bounds: Rectangle,
    padding: Padding,
) -> Option<Point> {
    Some(
        cursor.position()?
            - Vector::new(bounds.x + padding.left, bounds.y + padding.top),
    )
}

/// How much shorter than the text area the clip handed to the renderer is -
/// see [`text_clip`]. A tenth of a pixel, well under the half a pixel that
/// would change which pixels the clip covers.
const TEXT_CLIP_SHORTFALL: f32 = 0.1;

/// The rectangle the document's text is clipped to: the text area, a sliver
/// shorter.
///
/// That sliver is the whole point. The software renderer builds a clip mask
/// for text only when the text's own bounds reach past the clip it was given,
/// on the assumption that text inside its bounds cannot paint outside them.
/// A pixel-scrolled editor breaks the assumption: the rows the top and bottom
/// edges cut through are drawn whole, overhanging the editor by whatever the
/// edge cut off, and an unclipped overhang lands on the tab bar - where
/// nothing repaints it (see AGENTS.md). A clip the editor demonstrably
/// doesn't fit inside is what gets the mask built, and with it the overhang
/// clipped.
///
/// It costs no pixel of real text: the mask is not anti-aliased, so a pixel
/// belongs to it by its centre, and the sliver is far too thin to move one.
pub(super) fn text_clip(text_bounds: Rectangle) -> Rectangle {
    Rectangle {
        height: (text_bounds.height - TEXT_CLIP_SHORTFALL).max(0.0),
        ..text_bounds
    }
}

/// The guard on [`super::TextEditor::scroll_sensitivity`] and
/// [`super::TextEditor::drag_speed`], split out so the range is enforced in
/// one place. A non-finite value falls back to the default rather than
/// clamping - `NaN` has no meaningful end of the range.
pub(super) fn clamp_scroll_multiplier(multiplier: f32) -> f32 {
    if multiplier.is_finite() {
        multiplier.clamp(
            *SCROLL_MULTIPLIER_RANGE.start(),
            *SCROLL_MULTIPLIER_RANGE.end(),
        )
    } else {
        1.0
    }
}
