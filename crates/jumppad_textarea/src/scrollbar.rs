//! The editor's auto-hiding overlay scrollbar: a rounded thumb on an
//! invisible track, revealed by hovering near the right edge or by scrolling,
//! and faded back out after a delay.
//!
//! Everything here is pure - geometry in, geometry out, with `now` passed in
//! rather than read from the clock - so it can be tested without a window.
//! The one exception is [`State::metrics`], which remembers how tall the
//! document is rather than measuring it again on every event.

use std::cell::Cell;

use iced::advanced::graphics::text::cosmic_text::{self, Buffer};
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
/// A drag correction smaller than this is not worth a scroll: half a pixel
/// moves nothing on screen, and the next frame measures the gap again from
/// wherever the view actually is, so dropping it neither drifts nor compounds.
pub const SMALLEST_DRAG_SCROLL: f32 = 0.5;
/// How far the characters that fit on a row have to move before the document
/// is counted again. Wide enough that the mean character width over a screen
/// of a proportional face - which wanders with the text under it - doesn't
/// call for a fresh count every frame, and narrow enough that a change of
/// typeface or window width does. Five percent is a twentieth of a row on a
/// line that wraps once: less than the thumb's length can show.
const ROW_CAPACITY_TOLERANCE: f32 = 0.05;

const FADE_IN: Duration = Duration::from_millis(90);
/// How long the thumb stays at full opacity after the last hover or scroll.
const HOLD: Duration = Duration::from_millis(900);
const FADE_OUT: Duration = Duration::from_millis(300);

/// Where the document is scrolled to, in rows - the wrapped lines it draws
/// as, three of them for a line that wraps into three.
///
/// Rows rather than logical lines because rows are what the eye compares: a
/// screen of paragraphs shows less document than a screen of short lines, and
/// a thumb measured in logical lines cannot say so. All three below are in the
/// same unit, so the thumb's length is `viewport / content` and its travel
/// runs from nothing to `max_position`.
///
/// Only the rows on screen are laid out (cosmic-text shapes lazily, and
/// shaping the whole document costs the memory this editor exists not to
/// spend), so the rest are estimated rather than counted: see
/// [`State::metrics`]. The estimate being a row or two out matters far less
/// than it being the same answer wherever the document is scrolled to. A
/// measure taken from the lines on screen is exact and useless: it changes as
/// the view crosses a paragraph, which grew and shrank the thumb as it was
/// dragged, and moved the row a drag was aiming for out from under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Rows from the top of the document to the top of the viewport.
    pub position: f32,
    /// Rows in the document.
    pub content: f32,
    /// Rows the viewport shows.
    pub viewport: f32,
}

impl Metrics {
    /// How far the document can scroll, in rows. Zero when it all fits.
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
    /// How tall the document is, kept rather than counted again on every
    /// event - see `State::metrics`.
    counted: Cell<Option<Counted>>,
}

/// What has been counted about the document, and what it was counted against.
///
/// Counting walks lines, so the answer is kept until what it depends on moves:
/// the number of lines, or how many characters fit on a row - which carries
/// both the width the text wraps at and the face it is drawn in. An edit that
/// leaves the line count alone lands a row or two out of true, which is not
/// worth walking a document for.
#[derive(Debug, Clone, Copy)]
struct Counted {
    lines: usize,
    row_capacity: f32,
    /// Rows in the whole document.
    rows: f32,
    /// Rows above `above`, and the line they were counted up to. The next walk
    /// starts from here rather than from the top of the document, so following
    /// a view down one costs the lines it passes rather than the lines behind
    /// it.
    rows_above: f32,
    above: usize,
}

impl Counted {
    fn still_stands_for(
        &self,
        lines: usize,
        row_capacity: Option<f32>,
    ) -> bool {
        self.lines == lines
            && row_capacity.is_none_or(|capacity| {
                (self.row_capacity - capacity).abs()
                    <= capacity * ROW_CAPACITY_TOLERANCE
            })
    }
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
    /// Where the pointer has the thumb now. The view is walked toward it one
    /// frame at a time rather than on the pointer's own events - see
    /// [`State::scroll_to_pointer`].
    pointer: Point,
    /// Where the view was when this drag last asked it to move. Finding it
    /// there again means the ask went nowhere - the document is against one of
    /// its ends - and there is no point asking again until the pointer moves.
    scrolled_from: Option<f32>,
    /// The frame this drag last asked in. iced re-runs the redraw event at the
    /// same instant after a widget publishes anything - laying the whole
    /// window out again each time - so a drag that answered every pass would
    /// buy a fraction of a row's accuracy with two extra layouts, and keep
    /// iced re-running the frame until it gave up and warned. Whatever the
    /// scroll leaves over is the next frame's, sixteen milliseconds away.
    scrolled_at: Option<Instant>,
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
            self.revealed_at =
                now.checked_sub(FADE_IN.mul_f32(shown)).or(Some(now));
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
        let target = if wide {
            THUMB_WIDTH_HOVERED
        } else {
            THUMB_WIDTH_IDLE
        };
        if self.width_ramp.is_some_and(|ramp| ramp.to == target) {
            return;
        }
        let current = self.width(now);
        self.width_ramp = Some(WidthRamp {
            started_at: now,
            from: current,
            to: target,
        });
    }

    /// The animated thumb/track width right now, in pixels.
    pub fn width(&self, now: Instant) -> f32 {
        let Some(ramp) = self.width_ramp else {
            return THUMB_WIDTH_IDLE;
        };
        let t =
            ratio(now.saturating_duration_since(ramp.started_at), WIDTH_RAMP)
                .clamp(0.0, 1.0);
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
        let (Some(revealed_at), Some(active_at)) =
            (self.revealed_at, self.active_at)
        else {
            return 0.0;
        };
        let faded_in =
            ratio(now.saturating_duration_since(revealed_at), FADE_IN).min(1.0);

        if self.hovered || self.drag.is_some() {
            return faded_in;
        }
        let Some(fading) =
            now.saturating_duration_since(active_at).checked_sub(HOLD)
        else {
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
        let (Some(revealed_at), Some(active_at)) =
            (self.revealed_at, self.active_at)
        else {
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
        (now.saturating_duration_since(ramp.started_at) < WIDTH_RAMP)
            .then_some(now)
    }

    /// Grabs the thumb, if `position` is on it. Returns whether it took hold.
    pub fn press(
        &mut self,
        position: Point,
        layout: Layout,
        now: Instant,
    ) -> bool {
        let Some(thumb) = layout.thumb else {
            return false;
        };
        if !thumb.contains(position) {
            return false;
        }
        self.touch(now);
        self.drag = Some(Drag {
            grab_offset: position.y - thumb.y,
            pointer: position,
            scrolled_from: None,
            scrolled_at: None,
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

    /// Points a drag in progress at `position`, for the next frame to close
    /// the distance to. Does nothing when nothing is being dragged.
    pub fn drag_to(&mut self, position: Point, now: Instant) {
        // Only the hold clock: the ramp is already running from the press.
        self.active_at = Some(now);
        if let Some(drag) =
            self.drag.as_mut().filter(|drag| drag.pointer != position)
        {
            drag.pointer = position;
            drag.scrolled_from = None;
        }
    }

    /// The pixels that would put the view where the pointer is holding the
    /// thumb - the rows between the two, at a row's own height. `None` when
    /// nothing is being dragged, when the view is already close enough that
    /// scrolling would not move it visibly, or when the document has nothing
    /// further to give in that direction.
    ///
    /// Fractional, so the thumb tracks the pointer pixel for pixel instead of
    /// stepping a row at a time, and measured against where the view actually
    /// is rather than where the last answer asked it to go: the rows the view
    /// crosses are estimated (see [`Self::metrics`]), so a scroll can land a
    /// little short of, or past, the row it was aimed at, and the frame after
    /// closes what is left. The row it is aiming *for* does not move while it
    /// does that, which is what lets the two meet.
    ///
    /// Answered once for the frame at `now` and no more, however many times it
    /// is asked - see `Drag::scrolled_at`. That also settles what to do about
    /// the several `CursorMoved` events that can land in one input batch, all
    /// of them before any of their scrolls has reached the document: a
    /// correction made against a view that has not moved yet is the last one
    /// over again, and answering each would stack them into an overshoot.
    pub fn scroll_to_pointer(
        &mut self,
        layout: Layout,
        now: Instant,
    ) -> Option<f32> {
        let thumb = layout.thumb?;
        let metrics = layout.metrics;
        let drag = self.drag.as_mut()?;

        if drag.scrolled_at == Some(now)
            || drag.scrolled_from == Some(metrics.position)
        {
            return None;
        }

        // The thumb's travel is shorter than the track by its own height.
        let travel = layout.track.height - thumb.height;
        let progress = if travel > 0.0 {
            ((drag.pointer.y - drag.grab_offset - layout.track.y) / travel)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };

        // A thumb held against the end of its track is asking for the end of
        // the document rather than for a row in it, so it aims past the last
        // row the estimate knows about and lets the document stop it where it
        // really ends - an estimate a row or two short would otherwise leave
        // the last of the document out of a drag's reach. Never backwards,
        // though: an estimate a row or two long would pull the view back up
        // off the end it had just reached. The top needs none of this, being
        // the one row that is known exactly.
        let end = metrics.max_position();
        let target = if progress >= 1.0 {
            (end + metrics.viewport).max(metrics.position)
        } else if progress <= 0.0 {
            0.0
        } else {
            progress * end
        };

        let pixels = (target - metrics.position) * layout.pixels_per_row;
        if pixels.abs() < SMALLEST_DRAG_SCROLL {
            return None;
        }

        drag.scrolled_from = Some(metrics.position);
        drag.scrolled_at = Some(now);
        Some(pixels)
    }
}

/// Where the thumb and its track sit inside the editor's text bounds.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub track: Rectangle,
    /// `None` when the document fits on screen and there's nothing to show.
    pub thumb: Option<Rectangle>,
    metrics: Metrics,
    /// A row's own height, which is what one row of `Metrics::position` costs
    /// the view in pixels - the unit a scroll actually moves in.
    pixels_per_row: f32,
}

impl Layout {
    pub fn new(bounds: Rectangle, metrics: Metrics, width: f32) -> Self {
        let track = Rectangle {
            x: bounds.x + bounds.width - INSET - width,
            y: bounds.y + INSET,
            width,
            height: (bounds.height - INSET * 2.0).max(0.0),
        };

        let thumb =
            (metrics.max_position() > 0.0 && track.height > 0.0).then(|| {
                let proportional =
                    track.height * (metrics.viewport / metrics.content);
                // A window dragged down to a few rows leaves a track the two
                // bounds cannot both fit inside, so they are ordered here
                // rather than handed to `clamp` to disagree over - a floor
                // above its ceiling is a panic, not a narrow thumb. Which one
                // gives way: a track too short for the minimum still gets a
                // thumb, just the whole track's height rather than one hanging
                // off the end, and between there and a track the fraction can
                // fill, being big enough to see and grab wins over reading as
                // a position indicator.
                let shortest = MIN_THUMB_HEIGHT.min(track.height);
                let tallest = (track.height * MAX_THUMB_FRACTION).max(shortest);
                let height = proportional.clamp(shortest, tallest);

                Rectangle {
                    y: track.y + (track.height - height) * metrics.progress(),
                    height,
                    ..track
                }
            });

        Self {
            track,
            thumb,
            metrics,
            pixels_per_row: bounds.height / metrics.viewport,
        }
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

    /// How far down the document the viewport starts, in rows.
    pub fn position(&self) -> f32 {
        self.metrics.position
    }
}

impl State {
    /// Reads the scroll position out of the editor's cosmic-text buffer, which
    /// is the only place it exists - see this crate's `text_editor` module
    /// header - and measures the document around it. `None` before the buffer
    /// has been laid out, when there is nothing to measure and no scrollbar to
    /// draw.
    ///
    /// The document's rows are estimated rather than counted: how wide each
    /// line would draw, against the width the text wraps at. Only the lines on
    /// screen are laid out, so those are the only ones that *could* be
    /// counted, and an answer that leaned on them would change every time the
    /// wrapping under the view did. The estimate is exact for every line that
    /// fits, which is all of most documents, and close for the ones that
    /// wrap - and, more to the point, it is the same answer wherever the
    /// document is scrolled to. Byte length stands in for character count: the
    /// same number for anything ASCII, an over-count for scripts that don't
    /// fit in one byte.
    pub fn metrics(&self, buffer: &Buffer, bounds: Size) -> Option<Metrics> {
        let line_height = buffer.metrics().line_height;
        if line_height <= 0.0 || bounds.height <= 0.0 {
            return None;
        }
        // Nothing laid out yet, so nothing to measure against.
        buffer.layout_runs().next()?;

        let counted = self.counted(buffer, bounds.width);
        let scroll = buffer.scroll();

        Some(Metrics {
            position: self.rows_above(buffer, counted, scroll.line)
                + rows_into_line(buffer, scroll, line_height, counted),
            content: counted.rows,
            viewport: bounds.height / line_height,
        })
    }

    /// The document's rows, counted again only when the document or the
    /// characters that fit on a row have moved.
    fn counted(&self, buffer: &Buffer, wrap_width: f32) -> Counted {
        let lines = buffer.lines.len();
        let capacity = row_capacity(buffer, wrap_width);

        if let Some(kept) = self
            .counted
            .get()
            .filter(|kept| kept.still_stands_for(lines, capacity))
        {
            return kept;
        }

        // Nothing on screen has a width to measure against - a document of
        // blank lines, where every line is one row whatever the capacity is.
        let row_capacity = capacity.unwrap_or(f32::MAX);
        let counted = Counted {
            lines,
            row_capacity,
            rows: buffer
                .lines
                .iter()
                .map(|line| rows_in(line.text(), row_capacity))
                .sum::<f32>()
                .max(1.0),
            rows_above: 0.0,
            above: 0,
        };

        self.counted.set(Some(counted));
        counted
    }

    /// The rows above `line`, walked from wherever the last walk ended rather
    /// than from the top of the document: a view follows the lines it passes,
    /// so the anchor is nearly always the line next to the one being asked
    /// for.
    fn rows_above(
        &self,
        buffer: &Buffer,
        counted: Counted,
        line: usize,
    ) -> f32 {
        let mut counted = counted;
        let rows = |line: usize| {
            buffer
                .lines
                .get(line)
                .map_or(1.0, |line| rows_in(line.text(), counted.row_capacity))
        };

        while counted.above < line {
            counted.rows_above += rows(counted.above);
            counted.above += 1;
        }
        while counted.above > line {
            counted.above -= 1;
            counted.rows_above -= rows(counted.above);
        }
        counted.rows_above = counted.rows_above.max(0.0);

        self.counted.set(Some(counted));
        counted.rows_above
    }
}

/// How far into the line at the top of the view the view starts, in rows.
///
/// `scroll.vertical` is a pixel offset into that line's *laid out* rows, which
/// are the real ones; scaling it by the line's estimated rows keeps the answer
/// in the same rows everything else here counts in. Straight rows would step
/// past what the line above the next one contributed, and the thumb would
/// twitch backwards as the view crossed a line the estimate had wrong.
fn rows_into_line(
    buffer: &Buffer,
    scroll: cosmic_text::Scroll,
    line_height: f32,
    counted: Counted,
) -> f32 {
    let Some(line) = buffer.lines.get(scroll.line) else {
        return 0.0;
    };
    let laid_out = line.layout_opt().map_or(1, Vec::len).max(1) as f32;

    rows_in(line.text(), counted.row_capacity) * scroll.vertical
        / (laid_out * line_height)
}

/// How many rows a line of this text takes at `row_capacity` characters to a
/// row. Never less than one: an empty line is still a row.
fn rows_in(text: &str, row_capacity: f32) -> f32 {
    (text.len() as f32 / row_capacity).ceil().max(1.0)
}

/// How many characters fit on a row: the width the text wraps at, over how
/// wide a character is - averaged over the glyphs on screen, which is exact
/// for a monospace face, where every glyph is the same width, and a fair
/// average for a proportional one. `None` when there is nothing on screen to
/// measure.
fn row_capacity(buffer: &Buffer, wrap_width: f32) -> Option<f32> {
    if buffer.wrap() == cosmic_text::Wrap::None {
        // Nothing wraps, so a line is a row however long it is.
        return Some(f32::MAX);
    }

    let (width, glyphs) = buffer
        .layout_runs()
        .flat_map(|row| row.glyphs)
        .fold((0.0, 0usize), |(width, glyphs), glyph| {
            (width + glyph.w, glyphs + 1)
        });

    (width > 0.0).then(|| (wrap_width * glyphs as f32 / width).max(1.0))
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
}
