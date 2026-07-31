//! The editor's auto-hiding overlay scrollbar: a rounded thumb on an
//! invisible track, revealed by hovering near the right edge or by scrolling,
//! and faded back out after a delay.
//!
//! Everything here is pure - geometry in, geometry out, with `now` passed in
//! rather than read from the clock - so it can be tested without a window.

use iced_core::time::{Duration, Instant};
use iced_core::{Point, Rectangle, Size};

/// How far in from the right edge the pointer counts as "near the scrollbar".
pub const REVEAL_STRIP_WIDTH: f32 = 100.0;

const THUMB_WIDTH: f32 = 8.0;
/// Gap between the thumb and the right/top/bottom edges of the text area.
const INSET: f32 = 4.0;
/// Keeps a very long document's thumb big enough to see and to grab.
const MIN_THUMB_HEIGHT: f32 = 28.0;
/// Keeps a barely-scrollable document from showing a thumb that fills the
/// track and reads as a solid bar rather than a position indicator.
const MAX_THUMB_FRACTION: f32 = 0.6;

const FADE_IN: Duration = Duration::from_millis(90);
/// How long the thumb stays at full opacity after the last hover or scroll.
const HOLD: Duration = Duration::from_millis(900);
const FADE_OUT: Duration = Duration::from_millis(300);

/// Where the document is scrolled to, in whole lines.
///
/// The unit is *logical* lines rather than wrapped visual rows. Counting rows
/// would mean summing `BufferLine::layout_opt()` across the document on every
/// frame, and cosmic-text shapes lazily - an off-screen line that wraps to
/// three rows reports one until it scrolls into view, so the total (and the
/// thumb's height with it) would twitch as you scroll. Logical lines are exact
/// for anything that doesn't wrap, and stable everywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Lines from the top of the document to the top of the viewport.
    pub position: f32,
    /// Lines in the document.
    pub content: f32,
    /// Lines that fit in the viewport.
    pub viewport: f32,
}

impl Metrics {
    /// How far the document can scroll, in lines. Zero when it all fits.
    pub fn max_position(&self) -> f32 {
        (self.content - self.viewport).max(0.0)
    }

    /// Scroll position as a 0..=1 fraction of the way down the document.
    fn progress(&self) -> f32 {
        let max = self.max_position();
        if max <= 0.0 {
            0.0
        } else {
            (self.position / max).clamp(0.0, 1.0)
        }
    }
}

/// The transient state behind the reveal, one per editor.
#[derive(Debug, Default)]
pub struct State {
    /// Whether the pointer is inside the reveal strip. Holds the thumb open
    /// for as long as it's true, with no timer running.
    hovered: bool,
    /// When the thumb last had a reason to be visible. The fade is measured
    /// from here once `hovered` goes false.
    active_at: Option<Instant>,
    drag: Option<Drag>,
    /// Scroll position at the last frame, to notice the cursor scrolling the
    /// document on its own (arrow keys, typing past the bottom edge).
    last_position: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    /// Where in the thumb it was grabbed, so it doesn't jump to centre itself
    /// under the pointer on the first press.
    grab_offset: f32,
    /// Scroll lines the caller hasn't been able to apply yet - `Action::Scroll`
    /// only carries whole lines, so the remainder rides along to the next move
    /// instead of being dropped (the same trick the wheel uses).
    partial_lines: f32,
}

impl State {
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Marks the thumb as worth showing - starting the hold-then-fade clock.
    pub fn touch(&mut self, now: Instant) {
        self.active_at = Some(now);
    }

    /// Tracks the pointer against the reveal strip. Returns whether the answer
    /// changed, i.e. whether a redraw is needed.
    pub fn set_hovered(&mut self, hovered: bool, now: Instant) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        // Both directions refresh the clock: entering so a fade already under
        // way restarts, leaving so the hold is measured from the exit.
        self.touch(now);
        true
    }

    /// Notices the document scrolling for any reason - the wheel, but also
    /// cursor-driven auto-scroll, which never reaches the app as an action.
    /// Returns whether it moved.
    pub fn note_scroll(&mut self, position: f32, now: Instant) -> bool {
        let moved = self.last_position.is_some_and(|last| last != position);
        self.last_position = Some(position);
        if moved {
            self.touch(now);
        }
        moved
    }

    /// How visible the thumb should be right now, 0.0 (hidden) to 1.0.
    pub fn opacity(&self, now: Instant) -> f32 {
        let Some(active_at) = self.active_at else {
            return 0.0;
        };
        let elapsed = now.saturating_duration_since(active_at);

        if self.hovered || self.drag.is_some() {
            return ratio(elapsed, FADE_IN).min(1.0);
        }
        let Some(fading) = elapsed.checked_sub(HOLD) else {
            return 1.0;
        };
        1.0 - ratio(fading, FADE_OUT).min(1.0)
    }

    /// When the next frame is needed, or `None` if the thumb has settled and
    /// nothing needs to be drawn until the user does something. Keeping this
    /// exact is what lets the editor go back to zero CPU once it fades out.
    pub fn next_redraw(&self, now: Instant) -> Option<Instant> {
        let active_at = self.active_at?;
        let elapsed = now.saturating_duration_since(active_at);

        if self.hovered || self.drag.is_some() {
            // Ramping up: next frame. Settled at full: nothing to do.
            return (elapsed < FADE_IN).then_some(now);
        }
        match elapsed.checked_sub(HOLD) {
            // Still holding - sleep until the fade is due rather than spinning.
            None => Some(active_at + HOLD),
            Some(fading) if fading < FADE_OUT => Some(now),
            Some(_) => None,
        }
    }

    /// Grabs the thumb, if `position` is on it. Returns whether it took hold.
    pub fn press(&mut self, position: Point, layout: Layout, now: Instant) -> bool {
        let Some(thumb) = layout.thumb else {
            return false;
        };
        if !thumb.contains(position) {
            return false;
        }
        self.drag = Some(Drag {
            grab_offset: position.y - thumb.y,
            partial_lines: 0.0,
        });
        self.touch(now);
        true
    }

    pub fn release(&mut self, now: Instant) {
        if self.drag.take().is_some() {
            self.touch(now);
        }
    }

    /// Turns a drag to `position` into whole lines to scroll by, holding onto
    /// the fraction that didn't fit. `None` when nothing is being dragged, or
    /// when the movement so far hasn't added up to a line yet.
    pub fn drag_to(&mut self, position: Point, layout: Layout, now: Instant) -> Option<i32> {
        let drag = self.drag.as_mut()?;
        let thumb = layout.thumb?;
        self.active_at = Some(now);

        // The thumb's travel is shorter than the track by its own height.
        let travel = layout.track.height - thumb.height;
        let target = if travel > 0.0 {
            ((position.y - drag.grab_offset - layout.track.y) / travel).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let wanted = target * layout.metrics.max_position() - layout.metrics.position;
        let total = wanted + drag.partial_lines;
        let whole = total.trunc();
        drag.partial_lines = total - whole;

        (whole != 0.0).then_some(whole as i32)
    }
}

/// Where the thumb and its track sit inside the editor's text bounds.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub track: Rectangle,
    /// `None` when the document fits on screen and there's nothing to show.
    pub thumb: Option<Rectangle>,
    metrics: Metrics,
}

impl Layout {
    pub fn new(bounds: Rectangle, metrics: Metrics) -> Self {
        let track = Rectangle {
            x: bounds.x + bounds.width - INSET - THUMB_WIDTH,
            y: bounds.y + INSET,
            width: THUMB_WIDTH,
            height: (bounds.height - INSET * 2.0).max(0.0),
        };

        let thumb = (metrics.max_position() > 0.0 && track.height > 0.0).then(|| {
            let proportional = track.height * (metrics.viewport / metrics.content);
            let height = proportional
                .clamp(MIN_THUMB_HEIGHT, track.height * MAX_THUMB_FRACTION)
                // A track too short for the minimum still gets a thumb, just
                // the whole track's height rather than one hanging off the end.
                .min(track.height);

            Rectangle {
                y: track.y + (track.height - height) * metrics.progress(),
                height,
                ..track
            }
        });

        Self { track, thumb, metrics }
    }

    /// Whether `position` is close enough to the right edge to reveal the
    /// thumb. Measured from the edge of the editor, not of the track, so the
    /// strip is the same width whether or not a thumb is showing.
    pub fn is_in_reveal_strip(bounds: Rectangle, position: Point) -> bool {
        // An editor narrower than the strip is all strip, rather than the
        // strip hanging off its left edge.
        let width = REVEAL_STRIP_WIDTH.min(bounds.width);
        Rectangle {
            x: bounds.x + bounds.width - width,
            width,
            ..bounds
        }
        .contains(position)
    }

    pub fn radius(&self) -> f32 {
        THUMB_WIDTH / 2.0
    }

    /// How far down the document the viewport starts, in lines.
    pub fn position(&self) -> f32 {
        self.metrics.position
    }
}

/// Reads the scroll position out of the editor's cosmic-text buffer, which is
/// the only place it exists - see this crate's `text_editor` module header.
pub fn metrics(
    buffer: &iced::advanced::graphics::text::cosmic_text::Buffer,
    bounds: Size,
) -> Option<Metrics> {
    let line_height = buffer.metrics().line_height;
    if line_height <= 0.0 {
        return None;
    }
    let scroll = buffer.scroll();
    Some(Metrics {
        // `scroll.vertical` is a pixel offset into the wrapped rows of the line
        // at `scroll.line`, so dividing gives the fraction of a line scrolled
        // past the top.
        position: scroll.line as f32 + scroll.vertical / line_height,
        content: buffer.lines.len() as f32,
        viewport: bounds.height / line_height,
    })
}

fn ratio(elapsed: Duration, total: Duration) -> f32 {
    elapsed.as_secs_f32() / total.as_secs_f32()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rectangle = Rectangle {
        x: 0.0,
        y: 0.0,
        width: 400.0,
        height: 200.0,
    };

    fn metrics(position: f32, content: f32, viewport: f32) -> Metrics {
        Metrics { position, content, viewport }
    }

    fn scrollable() -> Layout {
        Layout::new(BOUNDS, metrics(0.0, 1000.0, 20.0))
    }

    #[test]
    fn a_document_that_fits_has_no_thumb() {
        assert!(Layout::new(BOUNDS, metrics(0.0, 12.0, 20.0)).thumb.is_none());
    }

    #[test]
    fn thumb_shrinks_as_the_document_grows() {
        let short = Layout::new(BOUNDS, metrics(0.0, 40.0, 20.0)).thumb.unwrap();
        let long = Layout::new(BOUNDS, metrics(0.0, 4000.0, 20.0)).thumb.unwrap();
        assert!(long.height < short.height);
    }

    #[test]
    fn thumb_respects_the_minimum_and_maximum() {
        let track = scrollable().track;

        let huge = Layout::new(BOUNDS, metrics(0.0, 100_000.0, 20.0)).thumb.unwrap();
        assert_eq!(huge.height, MIN_THUMB_HEIGHT);

        // 20 of 21 lines visible would otherwise fill almost the whole track.
        let barely = Layout::new(BOUNDS, metrics(0.0, 21.0, 20.0)).thumb.unwrap();
        assert_eq!(barely.height, track.height * MAX_THUMB_FRACTION);
    }

    #[test]
    fn thumb_travels_from_the_top_of_the_track_to_the_bottom() {
        let track = scrollable().track;

        let top = Layout::new(BOUNDS, metrics(0.0, 1000.0, 20.0)).thumb.unwrap();
        assert_eq!(top.y, track.y);

        let bottom = Layout::new(BOUNDS, metrics(980.0, 1000.0, 20.0)).thumb.unwrap();
        assert_eq!(bottom.y + bottom.height, track.y + track.height);
    }

    #[test]
    fn thumb_stays_inside_the_track_when_scrolled_past_the_end() {
        let track = scrollable().track;
        let thumb = Layout::new(BOUNDS, metrics(5000.0, 1000.0, 20.0)).thumb.unwrap();
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
        let narrow = Rectangle { width: 40.0, ..BOUNDS };
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
        // Fully faded in and still hovered: nothing more to draw.
        assert_eq!(state.next_redraw(start + FADE_IN), None);

        state.set_hovered(false, start + FADE_IN);
        let left = start + FADE_IN;
        // Waiting out the hold sleeps to its end rather than spinning.
        assert_eq!(state.next_redraw(left), Some(left + HOLD));
        assert_eq!(state.next_redraw(left + HOLD), Some(left + HOLD));
        assert_eq!(
            state.next_redraw(left + HOLD + FADE_OUT / 2),
            Some(left + HOLD + FADE_OUT / 2)
        );
        assert_eq!(state.next_redraw(left + HOLD + FADE_OUT), None);
    }

    #[test]
    fn pressing_off_the_thumb_does_not_start_a_drag() {
        let now = Instant::now();
        let layout = scrollable();
        let mut state = State::default();

        assert!(!state.press(Point::new(10.0, 10.0), layout, now));
        assert!(!state.is_dragging());
        // Below the thumb, but still in the track.
        assert!(!state.press(Point::new(layout.track.center_x(), 190.0), layout, now));
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

        let lines = state.drag_to(Point::new(grab.x, grab.y + 20.0), layout, now).unwrap();
        assert!(lines > 0);

        state.release(now);
        assert!(!state.is_dragging());
        assert_eq!(state.drag_to(Point::new(grab.x, grab.y + 40.0), layout, now), None);
    }

    #[test]
    fn dragging_to_the_bottom_scrolls_to_the_end() {
        let now = Instant::now();
        let layout = scrollable();
        let thumb = layout.thumb.unwrap();
        let mut state = State::default();

        state.press(Point::new(thumb.center_x(), thumb.y), layout, now);
        let lines = state.drag_to(Point::new(thumb.center_x(), 10_000.0), layout, now).unwrap();
        assert_eq!(lines, layout.metrics.max_position() as i32);
    }

    #[test]
    fn a_drag_too_small_to_move_a_line_is_saved_up_rather_than_lost() {
        let now = Instant::now();
        // 100 lines over a ~192px track: each line is well under a pixel.
        let layout = Layout::new(BOUNDS, metrics(0.0, 100.0, 20.0));
        let thumb = layout.thumb.unwrap();
        let mut state = State::default();

        let grab = Point::new(thumb.center_x(), thumb.y);
        state.press(grab, layout, now);

        // Sub-line nudges report nothing...
        assert_eq!(state.drag_to(Point::new(grab.x, grab.y + 0.4), layout, now), None);
        // ...but accumulate, so a run of them eventually delivers a line.
        let mut moved = 0;
        for step in 1..=10u8 {
            let to = Point::new(grab.x, grab.y + 0.4 * f32::from(step));
            moved += state.drag_to(to, layout, now).unwrap_or(0);
        }
        assert!(moved > 0);
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
}
