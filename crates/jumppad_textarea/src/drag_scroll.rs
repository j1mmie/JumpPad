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
const EDGE_SPEED: f32 = 60.0;
/// How fast the view moves once the pointer is `TOP_SPEED_REACH` past an
/// edge, in pixels per second.
const TOP_SPEED: f32 = 1600.0;
/// How far past the edge the pointer has to go to reach `TOP_SPEED`. Beyond
/// it the speed stops growing, so a pointer flung to the far side of the
/// screen scrolls no faster than one just outside the window.
const TOP_SPEED_REACH: f32 = 240.0;
/// The longest stretch of time one step may cover, so that a frame arriving
/// late - a busy machine, a window that just came back - moves the view by a
/// step rather than by everything it missed.
const LONGEST_STEP: Duration = Duration::from_millis(50);
/// A step shorter than this waits and rolls into the next one instead, which
/// is what keeps several frames landing in the same instant from each asking
/// for a fraction of a pixel.
const SHORTEST_STEP: f32 = 1.0;

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

    /// Where the pointer is - inside the text, out over the tab bar, or
    /// somewhere else entirely. Coordinates outside the text are the point:
    /// the editor hit-tests them against its nearest row.
    pub fn pointer(&self) -> Point {
        self.pointer
    }

    pub fn move_to(&mut self, pointer: Point) {
        self.pointer = pointer;
    }

    /// What this frame owes the drag.
    pub fn scroll_step(&mut self, text_height: f32, now: Instant) -> Step {
        let Some(speed) = speed_at(self.pointer.y, text_height) else {
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
    /// Past an edge, but not for long enough yet to have earned a whole
    /// pixel. The wait is what keeps several frames landing in the same
    /// instant from each asking for a fraction of one.
    Waiting,
    /// Move the view this many pixels - negative up, positive down.
    Scroll(f32),
}

/// How fast the view should move for a pointer at `y`, in pixels per second -
/// negative up, positive down. `None` when the pointer is on the text, where
/// the view holds still and only the selection follows it.
fn speed_at(y: f32, text_height: f32) -> Option<f32> {
    let past_edge = if y < 0.0 {
        y
    } else if y > text_height {
        y - text_height
    } else {
        return None;
    };

    let reach = (past_edge.abs() / TOP_SPEED_REACH).clamp(0.0, 1.0);

    Some(past_edge.signum() * (EDGE_SPEED + (TOP_SPEED - EDGE_SPEED) * reach))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT_HEIGHT: f32 = 400.0;
    const FRAME: Duration = Duration::from_millis(16);

    fn drag_at(y: f32) -> (Drag, Instant) {
        let now = Instant::now();

        (Drag::new(Point::new(10.0, y), now), now)
    }

    /// What a drag from a standstill asks for over one frame.
    fn frame_step(y: f32) -> Step {
        let (mut drag, now) = drag_at(y);

        drag.scroll_step(TEXT_HEIGHT, now + FRAME)
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

    #[test]
    fn a_step_too_small_to_see_waits_for_the_next_one() {
        let (mut drag, now) = drag_at(-5.0);
        let sliver = Duration::from_micros(200);

        assert_eq!(drag.scroll_step(TEXT_HEIGHT, now + sliver), Step::Waiting);
        // The time a wait covered is still there to be spent, so the view
        // moves by the whole frame rather than by what is left of it.
        assert_eq!(
            drag.scroll_step(TEXT_HEIGHT, now + FRAME),
            frame_step(-5.0)
        );
    }

    #[test]
    fn a_late_frame_moves_by_one_step_not_by_everything_it_missed() {
        let (mut drag, now) = drag_at(-5.0);

        let stalled =
            drag.scroll_step(TEXT_HEIGHT, now + Duration::from_secs(3));

        assert_eq!(
            pixels(stalled),
            speed_at(-5.0, TEXT_HEIGHT).unwrap() * LONGEST_STEP.as_secs_f32(),
        );
    }

    #[test]
    fn crossing_an_edge_starts_from_a_standstill() {
        let (mut drag, now) = drag_at(TEXT_HEIGHT / 2.0);
        let dwelt = Duration::from_secs(3);

        assert_eq!(drag.scroll_step(TEXT_HEIGHT, now + dwelt), Step::Still);

        drag.move_to(Point::new(10.0, -5.0));

        assert_eq!(
            drag.scroll_step(TEXT_HEIGHT, now + dwelt + FRAME),
            frame_step(-5.0),
        );
    }
}
