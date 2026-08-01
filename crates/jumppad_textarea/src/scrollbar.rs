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

/// Thumb/track width while the pointer is outside the reveal strip and
/// nothing is being dragged.
const THUMB_WIDTH_IDLE: f32 = 4.0;
/// Thumb/track width while hovered or actively dragged.
const THUMB_WIDTH_HOVERED: f32 = 12.0;
/// How long the width takes to ramp between idle and hovered, in either
/// direction. Its own clock, independent of FADE_IN/HOLD/FADE_OUT, so a hover
/// still fading in doesn't reset how far the width has grown, and vice versa.
const WIDTH_RAMP: Duration = Duration::from_millis(120);
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
    /// When the fade-in started. Separate from `active_at` because activity
    /// repeats - a drag or a spun wheel touches the state every few
    /// milliseconds - and folding the two together would restart the ramp on
    /// every event, leaving the thumb pinned at invisible for as long as the
    /// user kept going.
    revealed_at: Option<Instant>,
    /// When the thumb last had a reason to be visible. The hold, and then the
    /// fade-out, are measured from here.
    active_at: Option<Instant>,
    drag: Option<Drag>,
    /// Scroll position at the last frame, to notice the cursor scrolling the
    /// document on its own (arrow keys, typing past the bottom edge).
    last_position: Option<f32>,
    /// The thumb/track width's in-flight grow-or-shrink transition, if any.
    /// `None` means settled at `THUMB_WIDTH_IDLE` - no timer needed until
    /// something happens, same spirit as the fields above.
    width_ramp: Option<WidthRamp>,
}

/// A width transition in progress: interpolates linearly from `from` to `to`
/// over `WIDTH_RAMP`, starting at `started_at`.
#[derive(Debug, Clone, Copy)]
struct WidthRamp {
    started_at: Instant,
    from: f32,
    to: f32,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    /// Where in the thumb it was grabbed, so it doesn't jump to centre itself
    /// under the pointer on the first press.
    grab_offset: f32,
    /// The scroll position (in lines, full precision) this drag has already
    /// asked the document to be at. The baseline for the next delta, rather
    /// than the document's actual live position - several `CursorMoved`
    /// events can land in the same input batch and each call `drag_to`
    /// before the previous call's `Action::Scroll` has actually reached the
    /// document, so reading the document's position fresh each time would
    /// have every call in the batch compute a full correction against the
    /// same stale value and stack them into an overshoot (visible as the
    /// thumb briefly jumping backwards once a later event corrects it).
    /// Tracking our own running total keeps repeated calls idempotent.
    requested: f32,
}

impl State {
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Marks the thumb as worth showing - starting the hold-then-fade clock.
    ///
    /// Reads the current opacity, so it has to run *before* whatever flag is
    /// changing: `hovered` and `drag` both freeze the fade, which leaves
    /// `active_at` deliberately stale while they hold, and reading through the
    /// new flag would see a fade that never actually ran.
    pub fn touch(&mut self, now: Instant) {
        let shown = self.opacity(now);
        if shown < 1.0 {
            // Back-date the ramp so it picks up from however visible the thumb
            // already is: catching it mid-fade-out continues upward from there
            // rather than restarting at nothing or snapping to full.
            self.revealed_at = now.checked_sub(FADE_IN.mul_f32(shown)).or(Some(now));
        }
        self.active_at = Some(now);
    }

    /// Tracks the pointer against the reveal strip. Returns whether the answer
    /// changed, i.e. whether a redraw is needed.
    pub fn set_hovered(&mut self, hovered: bool, now: Instant) -> bool {
        if self.hovered == hovered {
            return false;
        }
        // Both directions refresh the clock: entering so a fade already under
        // way resumes, leaving so the hold is measured from the exit.
        self.touch(now);
        self.hovered = hovered;
        self.sync_width(now);
        true
    }

    /// Starts (or re-aims) the width ramp toward wherever `hovered`/`drag`
    /// currently say the width should be heading. Captures the *current*
    /// interpolated width as the new `from`, so catching a shrink mid-flight
    /// and reversing resumes from there instead of snapping back to
    /// `THUMB_WIDTH_IDLE` first.
    fn sync_width(&mut self, now: Instant) {
        let wide = self.hovered || self.drag.is_some();
        let target = if wide { THUMB_WIDTH_HOVERED } else { THUMB_WIDTH_IDLE };
        if self.width_ramp.is_some_and(|ramp| ramp.to == target) {
            return;
        }
        let current = self.width(now);
        self.width_ramp = Some(WidthRamp { started_at: now, from: current, to: target });
    }

    /// The animated thumb/track width right now, in pixels.
    pub fn width(&self, now: Instant) -> f32 {
        let Some(ramp) = self.width_ramp else {
            return THUMB_WIDTH_IDLE;
        };
        let t = ratio(now.saturating_duration_since(ramp.started_at), WIDTH_RAMP).clamp(0.0, 1.0);
        ramp.from + (ramp.to - ramp.from) * t
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
        let (Some(revealed_at), Some(active_at)) = (self.revealed_at, self.active_at) else {
            return 0.0;
        };
        let faded_in =
            ratio(now.saturating_duration_since(revealed_at), FADE_IN).min(1.0);

        if self.hovered || self.drag.is_some() {
            return faded_in;
        }
        let Some(fading) = now.saturating_duration_since(active_at).checked_sub(HOLD) else {
            return faded_in;
        };
        (faded_in - ratio(fading, FADE_OUT)).max(0.0)
    }

    /// When the next frame is needed, or `None` if the thumb has settled and
    /// nothing needs to be drawn until the user does something. Keeping this
    /// exact is what lets the editor go back to zero CPU once it fades out.
    /// The opacity fade and the width ramp run on independent clocks, so a
    /// frame is needed until *both* have settled.
    pub fn next_redraw(&self, now: Instant) -> Option<Instant> {
        earliest(self.opacity_next_redraw(now), self.width_next_redraw(now))
    }

    fn opacity_next_redraw(&self, now: Instant) -> Option<Instant> {
        let (Some(revealed_at), Some(active_at)) = (self.revealed_at, self.active_at) else {
            return None;
        };
        if now.saturating_duration_since(revealed_at) < FADE_IN {
            return Some(now);
        }
        if self.hovered || self.drag.is_some() {
            return None;
        }
        match now.saturating_duration_since(active_at).checked_sub(HOLD) {
            // Still holding - sleep until the fade is due rather than spinning.
            None => Some(active_at + HOLD),
            Some(fading) if fading < FADE_OUT => Some(now),
            Some(_) => None,
        }
    }

    fn width_next_redraw(&self, now: Instant) -> Option<Instant> {
        let ramp = self.width_ramp?;
        (now.saturating_duration_since(ramp.started_at) < WIDTH_RAMP).then_some(now)
    }

    /// Grabs the thumb, if `position` is on it. Returns whether it took hold.
    pub fn press(&mut self, position: Point, layout: Layout, now: Instant) -> bool {
        let Some(thumb) = layout.thumb else {
            return false;
        };
        if !thumb.contains(position) {
            return false;
        }
        self.touch(now);
        self.drag = Some(Drag {
            grab_offset: position.y - thumb.y,
            requested: layout.metrics.position,
        });
        self.sync_width(now);
        true
    }

    pub fn release(&mut self, now: Instant) {
        if self.drag.is_some() {
            self.touch(now);
            self.drag = None;
            self.sync_width(now);
        }
    }

    /// Turns a drag to `position` into whole lines to scroll by, holding onto
    /// the fraction that didn't fit. `None` when nothing is being dragged, or
    /// when the movement so far hasn't added up to a line yet.
    ///
    /// Idempotent against repeated calls for the same `position` - calling
    /// this again before the document has caught up to a previous call must
    /// not ask for more (see `Drag::requested`).
    pub fn drag_to(&mut self, position: Point, layout: Layout, now: Instant) -> Option<i32> {
        // Only the hold clock: the ramp is already running from the press.
        self.active_at = Some(now);
        let drag = self.drag.as_mut()?;
        let thumb = layout.thumb?;

        // The thumb's travel is shorter than the track by its own height.
        let travel = layout.track.height - thumb.height;
        let target = if travel > 0.0 {
            ((position.y - drag.grab_offset - layout.track.y) / travel).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let wanted = target * layout.metrics.max_position() - drag.requested;
        let whole = wanted.trunc();
        drag.requested += whole;

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
    pub fn new(bounds: Rectangle, metrics: Metrics, width: f32) -> Self {
        let track = Rectangle {
            x: bounds.x + bounds.width - INSET - width,
            y: bounds.y + INSET,
            width,
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
        self.track.width / 2.0
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

fn earliest(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) | (None, Some(x)) => Some(x),
        (Some(x), Some(y)) => Some(x.min(y)),
    }
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

    /// An arbitrary fixed width for tests that don't care about the width
    /// ramp - geometry assertions below hold for any width value.
    const TEST_WIDTH: f32 = 8.0;

    fn scrollable() -> Layout {
        Layout::new(BOUNDS, metrics(0.0, 1000.0, 20.0), TEST_WIDTH)
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

        let huge = Layout::new(BOUNDS, metrics(0.0, 100_000.0, 20.0), TEST_WIDTH)
            .thumb
            .unwrap();
        assert_eq!(huge.height, MIN_THUMB_HEIGHT);

        // 20 of 21 lines visible would otherwise fill almost the whole track.
        let barely = Layout::new(BOUNDS, metrics(0.0, 21.0, 20.0), TEST_WIDTH)
            .thumb
            .unwrap();
        assert_eq!(barely.height, track.height * MAX_THUMB_FRACTION);
    }

    #[test]
    fn thumb_travels_from_the_top_of_the_track_to_the_bottom() {
        let track = scrollable().track;

        let top = Layout::new(BOUNDS, metrics(0.0, 1000.0, 20.0), TEST_WIDTH)
            .thumb
            .unwrap();
        assert_eq!(top.y, track.y);

        let bottom = Layout::new(BOUNDS, metrics(980.0, 1000.0, 20.0), TEST_WIDTH)
            .thumb
            .unwrap();
        assert_eq!(bottom.y + bottom.height, track.y + track.height);
    }

    #[test]
    fn thumb_stays_inside_the_track_when_scrolled_past_the_end() {
        let track = scrollable().track;
        let thumb = Layout::new(BOUNDS, metrics(5000.0, 1000.0, 20.0), TEST_WIDTH)
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
        let midpoint = THUMB_WIDTH_IDLE + (THUMB_WIDTH_HOVERED - THUMB_WIDTH_IDLE) * 0.5;
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
        let midpoint = THUMB_WIDTH_IDLE + (THUMB_WIDTH_HOVERED - THUMB_WIDTH_IDLE) * 0.5;
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
    fn drag_to_is_idempotent_when_called_again_before_the_document_catches_up() {
        // Several `CursorMoved` events can land in the same input batch and
        // each call `drag_to` before the first one's `Action::Scroll` has
        // actually reached the document - so `layout` (built from the
        // document's live position) is identical across both calls here,
        // simulating the document not having caught up yet. A second call
        // for the same pointer position must report nothing new, or the
        // repeated correction stacks into an overshoot that the following
        // frame then has to visibly correct back (the thumb briefly jumping
        // backwards mid-drag).
        let now = Instant::now();
        let layout = scrollable();
        let thumb = layout.thumb.unwrap();
        let mut state = State::default();

        let grab = Point::new(thumb.center_x(), thumb.y);
        state.press(grab, layout, now);

        let to = Point::new(grab.x, grab.y + 40.0);
        let first = state.drag_to(to, layout, now);
        assert!(first.is_some_and(|lines| lines > 0));

        let second = state.drag_to(to, layout, now);
        assert_eq!(second, None);
    }

    #[test]
    fn a_drag_too_small_to_move_a_line_is_saved_up_rather_than_lost() {
        let now = Instant::now();
        // 100 lines over a ~192px track: each line is well under a pixel.
        let layout = Layout::new(BOUNDS, metrics(0.0, 100.0, 20.0), TEST_WIDTH);
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
            state.drag_to(Point::new(thumb.center_x(), thumb.y + 1.0), layout, now);
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
}
