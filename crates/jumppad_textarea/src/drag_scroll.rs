//! A selection drag that outlives the edges of the text: the pointer is
//! tracked wherever it goes - over the tab bar, past the window, off the
//! screen - and while it sits beyond the top or bottom edge the view walks
//! that way for as long as the button is held, so the selection can reach
//! lines that were never on screen.
//!
//! Everything here is pure - a pointer and a viewport in, pixels out, with
//! `now` passed in rather than read from the clock - so it can be tested
//! without a window (same convention as `scrollbar.rs`).

use iced_core::Point;
use iced_core::time::{Duration, Instant};

/// How fast the view moves the moment the pointer crosses an edge, in pixels
/// per second. Slow enough to stop on the line you meant.
const EDGE_SPEED: f32 = 120.0;
/// How fast the view moves once the pointer is `TOP_SPEED_REACH` past an
/// edge, in pixels per second.
const TOP_SPEED: f32 = 2000.0;
/// How far past the edge the pointer has to go to reach `TOP_SPEED`. Beyond
/// it the speed stops growing, so a pointer flung to the far side of the
/// screen scrolls no faster than one just outside the window.
///
/// Kept within a maximized window's reach: the room between the text and the
/// screen edge is all a maximized window leaves to ask for speed with, and a
/// reach longer than that would put the top of the range out of reach
/// exactly when the document is longest.
const TOP_SPEED_REACH: f32 = 160.0;
/// The longest stretch of time one step may cover, so that a frame arriving
/// late - a busy machine, a window that just came back - moves the view by a
/// step rather than by everything it missed.
const LONGEST_STEP: Duration = Duration::from_millis(50);
/// A step shorter than this waits and rolls into the next one instead. Well
/// under a pixel, so the walk stays smooth; its job is only to keep a frame
/// that re-runs in the same instant from asking for a second step.
const SHORTEST_STEP: f32 = 0.05;

/// The text a drag is walking over, and how fast the walk should go.
#[derive(Debug, Clone, Copy)]
pub struct Walk {
    /// The height of the text, in pixels.
    pub text_height: f32,
    /// The height of one row of it.
    pub line_height: f32,
    /// How much of the topmost row the top edge is cutting off. The view
    /// scrolls by pixels, so a row is nearly always part-way past the edge.
    pub clipped_top: f32,
    /// `[scroll] drag_speed`: a plain multiplier on how fast the view moves,
    /// where `1.0` is the shipped speed.
    pub speed: f32,
}

impl Walk {
    /// The same text once the view has moved `pixels`, which is the geometry
    /// a drag published alongside a scroll actually lands in - the scroll
    /// reaches the document first.
    pub fn after_scrolling(self, pixels: f32) -> Self {
        Self {
            clipped_top: (self.clipped_top + pixels)
                .rem_euclid(self.line_height),
            ..self
        }
    }

    /// The middle of the first row the top edge is *not* cutting through.
    fn first_whole_row(&self) -> f32 {
        let hidden = self.line_height - self.clipped_top;

        hidden % self.line_height + self.line_height / 2.0
    }

    /// The middle of the last row the bottom edge is *not* cutting through,
    /// or the first one when the text is too short to hold two.
    fn last_whole_row(&self) -> f32 {
        let whole_rows =
            ((self.text_height + self.clipped_top) / self.line_height).floor();
        let last = whole_rows * self.line_height
            - self.clipped_top
            - self.line_height / 2.0;

        last.max(self.first_whole_row())
    }
}

/// A selection drag in progress: where the pointer was last seen, in
/// coordinates relative to the text, and when the view last moved for it.
#[derive(Debug, Clone, Copy)]
pub struct Drag {
    pointer: Point,
    scrolled_at: Instant,
}

impl Drag {
    pub fn new(pointer: Point, now: Instant) -> Self {
        Self {
            pointer,
            scrolled_at: now,
        }
    }

    pub fn move_to(&mut self, pointer: Point) {
        self.pointer = pointer;
    }

    /// Where to drag the selection to. The pointer's own position while it
    /// is on the text, and the nearest whole row while it is past an edge.
    ///
    /// Aiming at the pointer's own height out there would put the caret on
    /// the row the edge is cutting through, and cosmic-text scrolls to
    /// reveal a caret it has clipped - a whole line at a time, on top of the
    /// pixels the walk just asked for. Those two pulling against each other
    /// is what makes a walk jump rather than glide.
    pub fn selecting_at(&self, walk: Walk) -> Point {
        if self.pointer.y >= 0.0 && self.pointer.y <= walk.text_height {
            return self.pointer;
        }

        Point::new(
            self.pointer.x,
            self.pointer
                .y
                .clamp(walk.first_whole_row(), walk.last_whole_row()),
        )
    }

    /// What this frame owes the drag.
    pub fn scroll_step(&mut self, walk: Walk, now: Instant) -> Step {
        let Some(speed) = speed_at(self.pointer.y, walk) else {
            // The clock only runs while the view is moving, so crossing an
            // edge starts from a standstill rather than from however long the
            // pointer spent inside the text.
            self.scrolled_at = now;
            return Step::Still;
        };

        let elapsed = now
            .saturating_duration_since(self.scrolled_at)
            .min(LONGEST_STEP);
        let pixels = speed * elapsed.as_secs_f32();

        if pixels.abs() < SHORTEST_STEP {
            return Step::Waiting;
        }

        self.scrolled_at = now;
        Step::Scroll(pixels)
    }
}

/// What one frame of a drag comes to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// The pointer is on the text. The view holds still, and the pointer's
    /// own movement is all the selection needs.
    Still,
    /// Past an edge, but not for long enough yet to have earned a step. The
    /// wait is what keeps a frame that re-runs in the same instant from
    /// asking for a second one.
    Waiting,
    /// Move the view this many pixels - negative up, positive down.
    Scroll(f32),
}

/// How fast the view should move for a pointer at `y`, in pixels per second -
/// negative up, positive down. `None` when the pointer is on the text, where
/// the view holds still and only the selection follows it.
///
/// The ramp is squared rather than straight so that the speeds a pointer just
/// outside the window can ask for are spread over most of the reach: a line
/// or two a second is what picking an exact line needs, and it would be a
/// sliver of the range under a straight ramp.
fn speed_at(y: f32, walk: Walk) -> Option<f32> {
    let past_edge = if y < 0.0 {
        y
    } else if y > walk.text_height {
        y - walk.text_height
    } else {
        return None;
    };

    let reach = (past_edge.abs() / TOP_SPEED_REACH).clamp(0.0, 1.0);
    let speed = EDGE_SPEED + (TOP_SPEED - EDGE_SPEED) * reach * reach;

    Some(past_edge.signum() * speed * walk.speed)
}

#[cfg(test)]
mod tests {
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
}
