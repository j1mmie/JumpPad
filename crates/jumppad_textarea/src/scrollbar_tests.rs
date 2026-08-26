use super::*;

const BOUNDS: Rectangle = Rectangle {
    x: 0.0,
    y: 0.0,
    width: 400.0,
    height: 200.0,
};

fn metrics(position: f32, content: f32, viewport: f32) -> Metrics {
    Metrics {
        position,
        content,
        viewport,
    }
}

/// An arbitrary fixed width for tests that don't care about the width
/// ramp - geometry assertions below hold for any width value.
const TEST_WIDTH: f32 = 8.0;

/// Long enough to be a frame of its own rather than a second pass over
/// the one before it, which is a distinction a drag draws.
const FRAME: Duration = Duration::from_millis(16);

fn scrollable() -> Layout {
    Layout::new(BOUNDS, metrics(0.0, 1000.0, 20.0), TEST_WIDTH)
}

/// The same view once it has moved by `pixels`, the way a document whose
/// rows were estimated exactly moves: every pixel lands on the row it was
/// aimed at.
fn scrolled_by(layout: Layout, pixels: f32) -> Layout {
    let position = (layout.metrics.position
        + pixels / layout.pixels_per_row)
        .clamp(0.0, layout.metrics.max_position());

    Layout::new(
        BOUNDS,
        metrics(position, layout.metrics.content, layout.metrics.viewport),
        TEST_WIDTH,
    )
}

#[test]
fn a_document_that_fits_has_no_thumb() {
    assert!(
        Layout::new(BOUNDS, metrics(0.0, 12.0, 20.0), TEST_WIDTH)
            .thumb
            .is_none()
    );
}

#[test]
fn thumb_shrinks_as_the_document_grows() {
    let short = Layout::new(BOUNDS, metrics(0.0, 40.0, 20.0), TEST_WIDTH)
        .thumb
        .unwrap();
    let long = Layout::new(BOUNDS, metrics(0.0, 4000.0, 20.0), TEST_WIDTH)
        .thumb
        .unwrap();
    assert!(long.height < short.height);
}

#[test]
fn thumb_respects_the_minimum_and_maximum() {
    let track = scrollable().track;

    let huge =
        Layout::new(BOUNDS, metrics(0.0, 100_000.0, 20.0), TEST_WIDTH)
            .thumb
            .unwrap();
    assert_eq!(huge.height, MIN_THUMB_HEIGHT);

    // 20 of 21 lines visible would otherwise fill almost the whole track.
    let barely = Layout::new(BOUNDS, metrics(0.0, 21.0, 20.0), TEST_WIDTH)
        .thumb
        .unwrap();
    assert_eq!(barely.height, track.height * MAX_THUMB_FRACTION);
}

/// A window shrunk far enough that `MAX_THUMB_FRACTION` of the track is
/// shorter than `MIN_THUMB_HEIGHT`. The minimum is the one that holds:
/// there is still room for it, and a thumb too small to grab is worse than
/// one that fills more of its track than it should.
#[test]
fn a_track_too_short_for_the_maximum_keeps_the_minimum() {
    let bounds = Rectangle {
        // A track of 44.67px, whose 60% is the 26.8px of the crash report.
        height: MIN_THUMB_HEIGHT / MAX_THUMB_FRACTION - 2.0 + INSET * 2.0,
        ..BOUNDS
    };
    let layout =
        Layout::new(bounds, metrics(0.0, 1000.0, 20.0), TEST_WIDTH);

    assert!(layout.track.height * MAX_THUMB_FRACTION < MIN_THUMB_HEIGHT);
    assert_eq!(layout.thumb.unwrap().height, MIN_THUMB_HEIGHT);
}

/// Shrunk past even the minimum, the thumb is the whole track rather than
/// one hanging off the end of it.
#[test]
fn a_track_too_short_for_the_minimum_fills_itself() {
    let bounds = Rectangle {
        height: MIN_THUMB_HEIGHT - 8.0 + INSET * 2.0,
        ..BOUNDS
    };
    let layout =
        Layout::new(bounds, metrics(0.0, 1000.0, 20.0), TEST_WIDTH);

    assert!(layout.track.height < MIN_THUMB_HEIGHT);
    assert_eq!(layout.thumb.unwrap().height, layout.track.height);
}

/// Every height a window dragged down to nothing passes through, a tenth
/// of a pixel at a time. The bounds cross on the way, which used to hand
/// `f32::clamp` a floor above its ceiling and panic the app.
#[test]
fn shrinking_a_window_to_nothing_never_leaves_the_track() {
    for tenths in 0..=1000u32 {
        let bounds = Rectangle {
            height: tenths as f32 / 10.0,
            ..BOUNDS
        };

        for position in [0.0, 500.0, 980.0] {
            let layout = Layout::new(
                bounds,
                metrics(position, 1000.0, 20.0),
                TEST_WIDTH,
            );
            let track = layout.track;
            let Some(thumb) = layout.thumb else {
                continue;
            };

            assert!(thumb.height > 0.0, "{thumb:?} at {bounds:?}");
            assert!(thumb.y >= track.y, "{thumb:?} above {track:?}");
            assert!(
                thumb.y + thumb.height <= track.y + track.height,
                "{thumb:?} past the end of {track:?}"
            );
        }
    }
}

#[test]
fn thumb_travels_from_the_top_of_the_track_to_the_bottom() {
    let track = scrollable().track;

    let top = Layout::new(BOUNDS, metrics(0.0, 1000.0, 20.0), TEST_WIDTH)
        .thumb
        .unwrap();
    assert_eq!(top.y, track.y);

    let bottom =
        Layout::new(BOUNDS, metrics(980.0, 1000.0, 20.0), TEST_WIDTH)
            .thumb
            .unwrap();
    assert_eq!(bottom.y + bottom.height, track.y + track.height);
}

#[test]
fn thumb_stays_inside_the_track_when_scrolled_past_the_end() {
    let track = scrollable().track;
    let thumb =
        Layout::new(BOUNDS, metrics(5000.0, 1000.0, 20.0), TEST_WIDTH)
            .thumb
            .unwrap();
    assert!(thumb.y + thumb.height <= track.y + track.height);
}

#[test]
fn track_sits_inside_the_right_edge() {
    let track = scrollable().track;
    assert_eq!(track.x + track.width, BOUNDS.width - INSET);
    assert_eq!(track.height, BOUNDS.height - INSET * 2.0);
}

#[test]
fn reveal_strip_covers_the_right_edge_only() {
    assert!(Layout::is_in_reveal_strip(BOUNDS, Point::new(399.0, 100.0)));
    assert!(Layout::is_in_reveal_strip(
        BOUNDS,
        Point::new(BOUNDS.width - REVEAL_STRIP_WIDTH + 1.0, 100.0)
    ));
    assert!(!Layout::is_in_reveal_strip(
        BOUNDS,
        Point::new(BOUNDS.width - REVEAL_STRIP_WIDTH - 1.0, 100.0)
    ));
    assert!(!Layout::is_in_reveal_strip(BOUNDS, Point::new(10.0, 100.0)));
}

#[test]
fn reveal_strip_never_exceeds_a_narrow_editor() {
    let narrow = Rectangle {
        width: 40.0,
        ..BOUNDS
    };
    assert!(Layout::is_in_reveal_strip(narrow, Point::new(1.0, 10.0)));
    assert!(!Layout::is_in_reveal_strip(narrow, Point::new(-1.0, 10.0)));
}

#[test]
fn starts_hidden_until_something_happens() {
    let state = State::default();
    assert_eq!(state.opacity(Instant::now()), 0.0);
    assert_eq!(state.next_redraw(Instant::now()), None);
}

#[test]
fn hovering_fades_in_then_holds_at_full() {
    let start = Instant::now();
    let mut state = State::default();
    assert!(state.set_hovered(true, start));

    assert_eq!(state.opacity(start), 0.0);
    assert!((state.opacity(start + FADE_IN / 2) - 0.5).abs() < 0.01);
    assert_eq!(state.opacity(start + FADE_IN), 1.0);
    // Hovering holds it open indefinitely - no fade while the pointer stays.
    assert_eq!(state.opacity(start + Duration::from_secs(60)), 1.0);
}

#[test]
fn leaving_holds_then_fades_out() {
    let start = Instant::now();
    let mut state = State::default();
    state.set_hovered(true, start);
    let left = start + Duration::from_secs(1);
    state.set_hovered(false, left);

    assert_eq!(state.opacity(left), 1.0);
    assert_eq!(state.opacity(left + HOLD), 1.0);
    assert!((state.opacity(left + HOLD + FADE_OUT / 2) - 0.5).abs() < 0.01);
    assert_eq!(state.opacity(left + HOLD + FADE_OUT), 0.0);
    assert_eq!(state.opacity(left + HOLD + FADE_OUT * 4), 0.0);
}

#[test]
fn re_entering_restarts_a_fade_already_under_way() {
    let start = Instant::now();
    let mut state = State::default();
    state.set_hovered(true, start);
    state.set_hovered(false, start);

    let mid_fade = start + HOLD + FADE_OUT / 2;
    assert!(state.opacity(mid_fade) < 1.0);
    state.set_hovered(true, mid_fade);
    assert_eq!(state.opacity(mid_fade + FADE_IN), 1.0);
}

#[test]
fn scrolling_reveals_it_without_a_hover() {
    let start = Instant::now();
    let mut state = State::default();

    // The first observation only establishes a baseline.
    assert!(!state.note_scroll(0.0, start));
    assert_eq!(state.opacity(start), 0.0);

    assert!(state.note_scroll(12.0, start));
    assert_eq!(state.opacity(start + HOLD), 1.0);
    assert_eq!(state.opacity(start + HOLD + FADE_OUT), 0.0);

    // A frame where nothing moved must not extend the hold.
    assert!(!state.note_scroll(12.0, start + Duration::from_millis(10)));
}

#[test]
fn stops_asking_for_frames_once_it_has_settled() {
    let start = Instant::now();
    let mut state = State::default();
    state.set_hovered(true, start);

    assert_eq!(state.next_redraw(start), Some(start));
    // Opacity is fully faded in by FADE_IN, but the width ramp
    // (WIDTH_RAMP, deliberately longer) still needs frames of its own.
    assert_eq!(state.next_redraw(start + FADE_IN), Some(start + FADE_IN));
    // Once both clocks have settled, nothing more to draw.
    assert_eq!(state.next_redraw(start + WIDTH_RAMP), None);

    state.set_hovered(false, start + WIDTH_RAMP);
    let left = start + WIDTH_RAMP;
    // Leaving restarts the (much shorter) width ramp too, so the very
    // next frame is needed immediately rather than waiting out the hold.
    assert_eq!(state.next_redraw(left), Some(left));
    // Waiting out the hold sleeps to its end rather than spinning, once
    // the width ramp has long since settled back to idle.
    assert_eq!(state.next_redraw(left + HOLD), Some(left + HOLD));
    assert_eq!(
        state.next_redraw(left + HOLD + FADE_OUT / 2),
        Some(left + HOLD + FADE_OUT / 2)
    );
    assert_eq!(state.next_redraw(left + HOLD + FADE_OUT), None);
}

#[test]
fn starts_at_idle_width_until_something_happens() {
    let state = State::default();
    assert_eq!(state.width(Instant::now()), THUMB_WIDTH_IDLE);
}

#[test]
fn hovering_grows_the_width_then_holds() {
    let start = Instant::now();
    let mut state = State::default();
    state.set_hovered(true, start);

    assert_eq!(state.width(start), THUMB_WIDTH_IDLE);
    let midpoint =
        THUMB_WIDTH_IDLE + (THUMB_WIDTH_HOVERED - THUMB_WIDTH_IDLE) * 0.5;
    assert!((state.width(start + WIDTH_RAMP / 2) - midpoint).abs() < 0.01);
    assert_eq!(state.width(start + WIDTH_RAMP), THUMB_WIDTH_HOVERED);
    // Holds wide indefinitely while still hovered - no shrink while the
    // pointer stays.
    assert_eq!(
        state.width(start + Duration::from_secs(60)),
        THUMB_WIDTH_HOVERED
    );
}

#[test]
fn leaving_shrinks_the_width_immediately_with_no_hold() {
    let start = Instant::now();
    let mut state = State::default();
    state.set_hovered(true, start);
    let left = start + WIDTH_RAMP;
    state.set_hovered(false, left);

    // Unlike opacity, there's no hold phase for width - it starts
    // shrinking the instant the pointer leaves the reveal strip.
    assert_eq!(state.width(left), THUMB_WIDTH_HOVERED);
    let midpoint =
        THUMB_WIDTH_IDLE + (THUMB_WIDTH_HOVERED - THUMB_WIDTH_IDLE) * 0.5;
    assert!((state.width(left + WIDTH_RAMP / 2) - midpoint).abs() < 0.01);
    assert_eq!(state.width(left + WIDTH_RAMP), THUMB_WIDTH_IDLE);
}

#[test]
fn re_entering_mid_shrink_resumes_growing_rather_than_snapping() {
    let start = Instant::now();
    let mut state = State::default();
    state.set_hovered(true, start);
    state.set_hovered(false, start + WIDTH_RAMP);

    let mid_shrink = start + WIDTH_RAMP + WIDTH_RAMP / 2;
    let width_at_mid_shrink = state.width(mid_shrink);
    assert!(width_at_mid_shrink > THUMB_WIDTH_IDLE);
    assert!(width_at_mid_shrink < THUMB_WIDTH_HOVERED);

    state.set_hovered(true, mid_shrink);
    // Resumes from wherever it was, rather than snapping back to idle
    // first.
    assert_eq!(state.width(mid_shrink), width_at_mid_shrink);
    assert_eq!(state.width(mid_shrink + WIDTH_RAMP), THUMB_WIDTH_HOVERED);
}

#[test]
fn pressing_off_the_thumb_does_not_start_a_drag() {
    let now = Instant::now();
    let layout = scrollable();
    let mut state = State::default();

    assert!(!state.press(Point::new(10.0, 10.0), layout, now));
    assert!(!state.is_dragging());
    // Below the thumb, but still in the track.
    assert!(!state.press(
        Point::new(layout.track.center_x(), 190.0),
        layout,
        now
    ));
    assert!(!state.is_dragging());
}

#[test]
fn dragging_the_thumb_down_scrolls_down() {
    let now = Instant::now();
    let layout = scrollable();
    let thumb = layout.thumb.unwrap();
    let mut state = State::default();

    let grab = Point::new(thumb.center_x(), thumb.y + 4.0);
    assert!(state.press(grab, layout, now));
    // A press alone asks for nothing: the thumb is already under the
    // pointer, which is the whole point of the grab offset.
    assert_eq!(state.scroll_to_pointer(layout, now), None);

    state.drag_to(Point::new(grab.x, grab.y + 20.0), now);
    assert!(state.scroll_to_pointer(layout, now).unwrap() > 0.0);

    state.release(now);
    assert!(!state.is_dragging());
    assert_eq!(state.scroll_to_pointer(layout, now + FRAME), None);
}

#[test]
fn dragging_to_the_bottom_scrolls_to_the_end() {
    let now = Instant::now();
    let layout = scrollable();
    let thumb = layout.thumb.unwrap();
    let mut state = State::default();

    state.press(Point::new(thumb.center_x(), thumb.y), layout, now);
    state.drag_to(Point::new(thumb.center_x(), 10_000.0), now);

    // A thumb against the end of its track asks for the end of the
    // document, which is past the last row the estimate knows about.
    assert_eq!(
        state.scroll_to_pointer(layout, now),
        Some(
            (layout.metrics.max_position() + layout.metrics.viewport)
                * layout.pixels_per_row
        )
    );
}

#[test]
fn a_drag_stops_asking_once_the_document_will_not_move() {
    // The rows past the view are estimated, so a drag can be asking for a
    // row the document doesn't have. A scroll that moved nothing says so,
    // and the drag waits for the pointer rather than asking again on every
    // frame for as long as the button is held.
    let now = Instant::now();
    let layout = scrollable();
    let thumb = layout.thumb.unwrap();
    let mut state = State::default();

    state.press(Point::new(thumb.center_x(), thumb.y), layout, now);
    state.drag_to(Point::new(thumb.center_x(), 10_000.0), now);
    assert!(state.scroll_to_pointer(layout, now).is_some());

    // The frame after, with the same view back again: the scroll moved
    // nothing.
    assert_eq!(state.scroll_to_pointer(layout, now + FRAME), None);

    // A pointer that moves is a fresh question.
    state.drag_to(Point::new(thumb.center_x(), 9_000.0), now);
    assert!(state.scroll_to_pointer(layout, now + FRAME).is_some());
}

#[test]
fn a_second_ask_in_the_same_frame_waits_for_the_next_one() {
    // iced re-runs the redraw event at the same instant after a widget
    // publishes anything, laying the whole window out again each time, so
    // a drag is asked two or three times a frame. Answering every ask
    // bought a fraction of a row for two extra layouts, and kept iced
    // re-running the frame until it gave up and filled the log with
    // "More than 3 consecutive RedrawRequested events".
    let now = Instant::now();
    let layout = scrollable();
    let thumb = layout.thumb.unwrap();
    let mut state = State::default();

    state.press(Point::new(thumb.center_x(), thumb.y), layout, now);
    state.drag_to(Point::new(thumb.center_x(), thumb.y + 40.0), now);

    let pixels = state.scroll_to_pointer(layout, now).unwrap();
    assert_eq!(state.scroll_to_pointer(layout, now), None);

    // The frame after picks up whatever the scroll left over - here a
    // view that only moved half as far as it was asked to.
    let moved = scrolled_by(layout, pixels / 2.0);
    assert!(state.scroll_to_pointer(moved, now + FRAME).is_some());
}

#[test]
fn a_scroll_that_lands_short_is_finished_off_by_the_frames_after_it() {
    // The rows the drag is about to cross were estimated at a third of
    // what they turn out to be, so its scroll covers a third of the
    // distance it asked for. What is left over is the next frame's
    // correction, and the drag arrives rather than stopping short.
    let now = Instant::now();
    let mut layout =
        Layout::new(BOUNDS, metrics(0.0, 1000.0, 20.0), TEST_WIDTH);
    let thumb = layout.thumb.unwrap();
    let mut state = State::default();

    // Half way down the track, so the drag has somewhere to arrive at
    // rather than running into the end of the document.
    let travel = layout.track.height - thumb.height;
    state.press(Point::new(thumb.center_x(), thumb.y), layout, now);
    state.drag_to(
        Point::new(thumb.center_x(), layout.track.y + travel / 2.0),
        now,
    );
    let target = layout.metrics.max_position() / 2.0;

    let mut now = now;
    let mut frames = 0;
    while let Some(pixels) = state.scroll_to_pointer(layout, now) {
        layout = scrolled_by(layout, pixels / 3.0);
        now += FRAME;
        frames += 1;

        assert!(layout.metrics.position <= target, "{:?}", layout.metrics);
        assert!(frames < 100, "the drag never arrived");
    }

    assert!(frames > 1, "one frame is all an exact estimate would need");
    assert!(
        target - layout.metrics.position < 0.1,
        "{:?} short of {target}",
        layout.metrics
    );
}

#[test]
fn a_sub_row_drag_moves_the_view_rather_than_waiting_for_a_whole_row() {
    let now = Instant::now();
    // 100 rows over a ~192px track: each row is well under a pixel of
    // travel, so every nudge below is a fraction of one. These used to be
    // banked until they added up to a whole row, which is what made a
    // slow drag step instead of track the pointer.
    let mut layout =
        Layout::new(BOUNDS, metrics(0.0, 100.0, 20.0), TEST_WIDTH);
    let thumb = layout.thumb.unwrap();
    let mut state = State::default();

    let grab = Point::new(thumb.center_x(), thumb.y);
    state.press(grab, layout, now);

    state.drag_to(Point::new(grab.x, grab.y + 0.4), now);
    let first = state.scroll_to_pointer(layout, now).unwrap();
    assert!(first > 0.0 && first < layout.pixels_per_row, "{first}");

    layout = scrolled_by(layout, first);
    let after_one = layout.metrics.position;
    assert!(after_one > 0.0 && after_one < 1.0, "{after_one}");

    // And the fractions add up rather than being dropped, so a run of
    // them leaves the view further down than the first one did.
    let mut now = now;
    for step in 2..=10u8 {
        let to = Point::new(grab.x, grab.y + 0.4 * f32::from(step));
        now += FRAME;
        state.drag_to(to, now);

        if let Some(pixels) = state.scroll_to_pointer(layout, now) {
            layout = scrolled_by(layout, pixels);
        }
    }

    let moved = layout.metrics.position;
    assert!(
        moved > after_one,
        "{moved} should have grown past {after_one}"
    );
}

#[test]
fn repeated_activity_does_not_restart_the_fade_in() {
    let start = Instant::now();
    let layout = scrollable();
    let thumb = layout.thumb.unwrap();
    let mut state = State::default();

    // A drag touches the state every few milliseconds. Once faded in, it
    // has to stay in - the thumb was invisible for the whole drag when
    // each event reset the ramp.
    state.press(Point::new(thumb.center_x(), thumb.y), layout, start);
    let mut now = start + FADE_IN;
    assert_eq!(state.opacity(now), 1.0);

    for _ in 0..20 {
        now += Duration::from_millis(8);
        state.drag_to(Point::new(thumb.center_x(), thumb.y + 1.0), now);
        assert_eq!(state.opacity(now), 1.0);
    }

    // Same for a spun wheel, which reveals it without any hover at all.
    let mut state = State::default();
    state.note_scroll(0.0, start);
    let mut now = start;
    for line in 1..=20u8 {
        now += Duration::from_millis(8);
        state.note_scroll(f32::from(line), now);
    }
    assert_eq!(state.opacity(now), 1.0);
}

#[test]
fn re_entering_mid_fade_resumes_rather_than_snapping() {
    let start = Instant::now();
    let mut state = State::default();
    state.set_hovered(true, start);
    state.set_hovered(false, start + FADE_IN);

    // Catch it half faded out, and it must keep climbing from ~0.5 rather
    // than jumping straight to full.
    let half_gone = start + FADE_IN + HOLD + FADE_OUT / 2;
    assert!((state.opacity(half_gone) - 0.5).abs() < 0.01);

    state.set_hovered(true, half_gone);
    assert!((state.opacity(half_gone) - 0.5).abs() < 0.01);
    assert!((state.opacity(half_gone + FADE_IN / 4) - 0.75).abs() < 0.02);
    assert_eq!(state.opacity(half_gone + FADE_IN / 2), 1.0);
}

#[test]
fn dragging_holds_it_open_like_a_hover() {
    let now = Instant::now();
    let layout = scrollable();
    let thumb = layout.thumb.unwrap();
    let mut state = State::default();

    state.press(Point::new(thumb.center_x(), thumb.y), layout, now);
    assert_eq!(state.opacity(now + FADE_IN + HOLD + FADE_OUT), 1.0);
}

#[test]
fn dragging_holds_the_width_wide_like_a_hover() {
    let now = Instant::now();
    let layout = scrollable();
    let thumb = layout.thumb.unwrap();
    let mut state = State::default();

    state.press(Point::new(thumb.center_x(), thumb.y), layout, now);
    assert_eq!(
        state.width(now + WIDTH_RAMP + Duration::from_secs(60)),
        THUMB_WIDTH_HOVERED
    );
}
