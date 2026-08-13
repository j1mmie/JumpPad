//! The rows of the viewport a cursor is allowed to come to rest on: everything
//! but a few lines held back at the top and bottom edges.
//!
//! The held-back lines are context. A cursor sitting hard against an edge has
//! nothing on the far side of it to read, so a change that moves the cursor
//! puts it inside this region rather than on the edge cosmic-text would leave
//! it on. Who scrolls, and which way, is the caller's business - see
//! `reveal_offset` and `restore_offset` in `text_editor.rs`. This module only
//! says where the region is.
//!
//! Pure geometry, in rows and out: it tests without a window.

/// Lines held back at each edge, so a cursor placed inside the safe area keeps
/// this much context past it - landing on the sixth visible line rather than
/// the first or the last.
pub const INSET_LINE_COUNT: i32 = 5;

/// The safe area of a viewport, in rows counted from its top.
///
/// Rows count downwards, so [`high`] is the *smaller* index - the boundary
/// nearest the top of the screen - and [`low`] the larger one, nearest the
/// bottom. A cursor between the two is clear of both edges.
///
/// [`high`]: Self::high
/// [`low`]: Self::low
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SafeArea {
    /// Rows the viewport shows, fractional - a partly clipped bottom row
    /// counts for the fraction of it that's visible.
    rows: f32,
    /// The inset this viewport can afford, which is [`INSET_LINE_COUNT`] until
    /// the viewport is too short for it.
    inset: i32,
}

impl SafeArea {
    /// The safe area of a viewport `rows` tall.
    ///
    /// The inset gives way at the viewport's middle. A view too short for the
    /// whole of it would otherwise answer a cursor inset from one edge by
    /// parking it past the other, and `high` and `low` would cross.
    pub fn of(rows: f32) -> Self {
        Self {
            rows,
            inset: INSET_LINE_COUNT.min((rows as i32 - 1) / 2).max(0),
        }
    }

    /// The high boundary: the topmost row a cursor may rest on.
    pub fn high(&self) -> f32 {
        self.inset as f32
    }

    /// The low boundary: the bottommost row a cursor may rest on.
    pub fn low(&self) -> f32 {
        (self.last_row() - self.inset as f32).max(0.0)
    }

    /// The last row the viewport shows, whole or clipped. Past it is off
    /// screen.
    pub fn last_row(&self) -> f32 {
        self.rows.floor() - 1.0
    }

    /// The lines held back at each edge - [`INSET_LINE_COUNT`], or less in a
    /// viewport too short for it, or zero in one too short to hold back any.
    ///
    /// The same number as [`high`], counted the other way round: from the top
    /// edge to the boundary rather than from row zero.
    ///
    /// [`high`]: Self::high
    pub fn inset_line_count(&self) -> i32 {
        self.inset
    }

    /// Whether `row` is the first row the viewport shows - where a reveal that
    /// scrolled the view up leaves a cursor it chased by the bare minimum.
    pub fn on_first_row(&self, row: f32) -> bool {
        row < 1.0
    }

    /// Whether `row` is the last row the viewport shows, the counterpart to
    /// [`on_first_row`] for a view that scrolled down.
    ///
    /// Measured against the unclipped viewport rather than [`last_row`]: a
    /// cursor on the clipped row of a fractional viewport is on the last row
    /// the view has, however little of it is showing.
    ///
    /// [`on_first_row`]: Self::on_first_row
    /// [`last_row`]: Self::last_row
    pub fn on_last_row(&self, row: f32) -> bool {
        row > self.rows - 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tall_viewport_holds_back_the_whole_inset() {
        let area = SafeArea::of(20.0);

        assert_eq!(area.inset_line_count(), INSET_LINE_COUNT);
        assert_eq!(area.high(), 5.0);
        assert_eq!(area.low(), 14.0);
        assert_eq!(area.last_row(), 19.0);
    }

    #[test]
    fn the_inset_gives_way_at_the_middle() {
        // Five rows of view: a whole inset either side would leave nowhere to
        // rest, so it gives up at two.
        let area = SafeArea::of(5.0);

        assert_eq!(area.inset_line_count(), 2);
        assert_eq!(area.high(), 2.0);
        assert_eq!(area.low(), 2.0);
    }

    #[test]
    fn the_boundaries_never_cross() {
        for rows in 0..40 {
            let area = SafeArea::of(rows as f32);
            assert!(
                area.high() <= area.low(),
                "{rows} rows: high {} past low {}",
                area.high(),
                area.low()
            );
        }
    }

    #[test]
    fn a_viewport_too_short_holds_nothing_back() {
        for rows in [0.0, 1.0, 2.0] {
            assert_eq!(SafeArea::of(rows).inset_line_count(), 0);
        }
    }

    #[test]
    fn a_clipped_bottom_row_is_still_the_last_row() {
        let area = SafeArea::of(20.4);

        // The twenty-first row shows a sliver, and a cursor on it is as far
        // down as the view goes - but it isn't a row the view can rest on.
        assert!(area.on_last_row(20.0));
        assert_eq!(area.last_row(), 19.0);
    }
}
