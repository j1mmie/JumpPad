use super::*;

const LINE_HEIGHT: f32 = 20.0;
const TEXT_HEIGHT: f32 = 400.0;
const FRAME: Duration = Duration::from_millis(16);

fn walk() -> Walk {
    Walk {
        text_height: TEXT_HEIGHT,
        line_height: LINE_HEIGHT,
        clipped_top: 0.0,
        speed: 1.0,
    }
}

fn drag_at(y: f32) -> (Drag, Instant) {
    let now = Instant::now();

    (Drag::new(Point::new(10.0, y), now), now)
}

/// What a drag from a standstill asks for over one frame.
fn frame_step(y: f32) -> Step {
    let (mut drag, now) = drag_at(y);

    drag.scroll_step(walk(), now + FRAME)
}

fn pixels(step: Step) -> f32 {
    match step {
        Step::Scroll(pixels) => pixels,
        held => panic!("expected a scroll, got {held:?}"),
    }
}

#[test]
fn a_pointer_on_the_text_holds_the_view_still() {
    assert_eq!(frame_step(0.0), Step::Still);
    assert_eq!(frame_step(TEXT_HEIGHT / 2.0), Step::Still);
    assert_eq!(frame_step(TEXT_HEIGHT), Step::Still);
}

#[test]
fn a_pointer_above_the_text_walks_the_view_up() {
    assert!(pixels(frame_step(-20.0)) < 0.0);
}

#[test]
fn a_pointer_below_the_text_walks_the_view_down() {
    assert!(pixels(frame_step(TEXT_HEIGHT + 20.0)) > 0.0);
}

#[test]
fn the_further_past_the_edge_the_faster_it_goes() {
    let near = pixels(frame_step(-5.0));
    let far = pixels(frame_step(-100.0));

    assert!(far < near, "{far} should outrun {near}");
}

#[test]
fn the_speed_stops_growing_past_the_full_reach() {
    let full_reach = frame_step(-TOP_SPEED_REACH);
    let off_screen = frame_step(-10_000.0);

    assert_eq!(full_reach, off_screen);
}

/// The squared ramp, in the terms it exists for: the slow speeds - the
/// ones picking an exact line needs - get most of the reach to
/// themselves instead of a sliver of it.
#[test]
fn half_the_reach_buys_a_quarter_of_the_speed() {
    let half_way = speed_at(-TOP_SPEED_REACH / 2.0, walk()).unwrap().abs();
    let full_reach = speed_at(-TOP_SPEED_REACH, walk()).unwrap().abs();

    assert_eq!(half_way - EDGE_SPEED, (full_reach - EDGE_SPEED) / 4.0);
}

#[test]
fn a_step_too_small_to_see_waits_for_the_next_one() {
    let (mut drag, now) = drag_at(-5.0);
    let sliver = Duration::from_micros(1);

    assert_eq!(drag.scroll_step(walk(), now + sliver), Step::Waiting);
    // The time a wait covered is still there to be spent, so the view
    // moves by the whole frame rather than by what is left of it.
    assert_eq!(drag.scroll_step(walk(), now + FRAME), frame_step(-5.0));
}

#[test]
fn a_late_frame_moves_by_one_step_not_by_everything_it_missed() {
    let (mut drag, now) = drag_at(-5.0);

    let stalled = drag.scroll_step(walk(), now + Duration::from_secs(3));

    assert_eq!(
        pixels(stalled),
        speed_at(-5.0, walk()).unwrap() * LONGEST_STEP.as_secs_f32(),
    );
}

#[test]
fn crossing_an_edge_starts_from_a_standstill() {
    let (mut drag, now) = drag_at(TEXT_HEIGHT / 2.0);
    let dwelt = Duration::from_secs(3);

    assert_eq!(drag.scroll_step(walk(), now + dwelt), Step::Still);

    drag.move_to(Point::new(10.0, -5.0));

    assert_eq!(
        drag.scroll_step(walk(), now + dwelt + FRAME),
        frame_step(-5.0),
    );
}

#[test]
fn the_configured_speed_scales_the_whole_ramp() {
    let half = Walk {
        speed: 0.5,
        ..walk()
    };

    for past_edge in [1.0, 60.0, TOP_SPEED_REACH, 10_000.0] {
        let full_speed = speed_at(-past_edge, walk()).unwrap();
        let half_speed = speed_at(-past_edge, half).unwrap();

        assert_eq!(half_speed, full_speed / 2.0, "at {past_edge}px out");
    }
}

#[test]
fn a_pointer_on_the_text_selects_where_it_is() {
    let (drag, _) = drag_at(123.0);

    assert_eq!(drag.selecting_at(walk()), drag.pointer);
}

#[test]
fn a_pointer_past_an_edge_selects_to_the_last_whole_row() {
    let clipped = Walk {
        clipped_top: 5.0,
        ..walk()
    };
    // Rows sit at -5, 15, 35 ... so the first whole one is 15..35 and
    // the last one to fit inside 400 is 375..395.
    let above = drag_at(-500.0).0.selecting_at(clipped);
    let below = drag_at(10_000.0).0.selecting_at(clipped);

    assert_eq!(above.y, 25.0);
    assert_eq!(below.y, 385.0);
}

/// Nothing is clipped at the top of a document, so the first row is
/// whole - and a drag above the text has to be able to reach it.
#[test]
fn a_view_at_the_top_of_the_document_selects_into_its_first_row() {
    assert_eq!(drag_at(-500.0).0.selecting_at(walk()).y, 10.0);
}

/// The scroll reaches the document before the drag does, so aiming at
/// the rows as they sit *now* would put the caret on one the bottom edge
/// has started cutting through by the time it arrives - and cosmic-text
/// would scroll a whole line to reveal it.
#[test]
fn a_walk_aims_at_the_rows_the_scroll_is_about_to_leave() {
    let scrolled = walk().after_scrolling(32.0);

    assert_eq!(scrolled.clipped_top, 12.0);
    assert_eq!(
        drag_at(10_000.0).0.selecting_at(scrolled).y,
        walk().last_whole_row() - 12.0,
    );
}

#[test]
fn a_text_too_short_for_a_whole_row_still_has_somewhere_to_aim() {
    let sliver = Walk {
        text_height: 12.0,
        ..walk()
    };
    let aimed = drag_at(-500.0).0.selecting_at(sliver);

    assert_eq!(aimed.y, sliver.first_whole_row());
}
