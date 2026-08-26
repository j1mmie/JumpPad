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
