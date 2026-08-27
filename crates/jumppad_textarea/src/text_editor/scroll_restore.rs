use iced::advanced::graphics;
use iced_core::text::editor::{Action, Editor as _, Selection};
use iced_core::Size;

use crate::safe_area::SafeArea;

/// A change to the document the widget has not laid out yet, and where the
/// view sat when it happened. Both variants want the same thing of the next
/// shape - a cursor inside the safe area, clear of the edge it came in from -
/// but they start from opposite ends: an in-place edit keeps the view it had,
/// while a rebuilt `Content` starts at the top of the document with the view
/// thrown away.
#[derive(Debug, Clone, Copy)]
pub(super) enum PendingView {
    /// An `Action` that edited the document in place.
    Edited { scrolled_to: f32 },
    /// A line command, spliced in place. The buffer keeps its own scroll, so
    /// unlike `Rebuilt` there is no view to put back - only the safe area to
    /// honour. All it carries is the *logical* line the caret started on:
    /// the boundaries are measured against the caret's row as the next shape
    /// finds it, and the only thing the past is needed for is which way the
    /// caret went. Lines answer that exactly, where rows would not - a
    /// wrapped line moves the caret three rows for one line of travel.
    Spliced { caret_line: usize },
    /// A `Content` rebuilt under the same document, by undo, redo or a line
    /// command, carrying what the `Content` it replaces had.
    Rebuilt(CapturedView),
}

/// Where a [`super::Content`] had its view and its cursor, read off before a
/// rebuild replaces it and handed to the [`super::Content`] that takes its
/// place.
///
/// The view is the buffer's own `Scroll` - a logical line and the pixels into
/// it - rather than the single number [`super::Content::scrolled_to`]
/// reports. That number adds the two together, which is fine for the
/// scrollbar but cannot be scrolled *back* to: see `restore_pixels`.
///
/// The cursor's row travels with them because a reveal needs to know which way
/// the cursor is *going*, not just where it ended up: a line moved up has no
/// business scrolling the view down to chase it past the low boundary.
#[derive(Debug, Clone, Copy)]
pub struct CapturedView {
    pub(super) scroll_line: usize,
    pub(super) scroll_vertical: f32,
    pub(super) cursor_row: f32,
}

/// Where the view sits, in lines from the top of the document, or `None` if it
/// has no line height to measure against yet. Fractional: `scroll.vertical` is
/// a pixel offset into the wrapped rows of the line at `scroll.line`.
pub(super) fn scrolled_to(editor: &graphics::text::Editor) -> Option<f32> {
    let buffer = editor.buffer();
    let line_height = buffer.metrics().line_height;
    if line_height <= 0.0 {
        return None;
    }

    let scroll = buffer.scroll();
    Some(scroll.line as f32 + scroll.vertical / line_height)
}

/// Which visible row the cursor is on, counting from the top of the view.
/// Below zero or past the last row means it is off screen.
///
/// An `Indent` keeps its selection and an undo restores one, so the caret is
/// not always what is on screen; the row of the selection nearest the view
/// stands in for it.
pub(super) fn cursor_row(editor: &graphics::text::Editor) -> Option<f32> {
    let line_height = editor.buffer().metrics().line_height;
    if line_height <= 0.0 {
        return None;
    }

    Some(match editor.selection() {
        Selection::Caret(position) => position.y / line_height,
        Selection::Range(regions) => {
            let first = regions.first()?.y / line_height;
            let last = regions.last()?.y / line_height;

            // Its own top if the selection is below the view, its bottom if
            // it is above, and a row inside the view if it straddles one edge.
            first.max(0.0).min(last)
        }
    })
}

/// How tall the view is, in rows - fractional, since it rarely divides into a
/// whole number of them.
pub(super) fn viewport_rows(
    editor: &graphics::text::Editor,
    text_bounds: Size,
) -> f32 {
    text_bounds.height / editor.buffer().metrics().line_height
}

/// Shapes the editor, then hands the cursor of a change the widget has not
/// seen yet the view it wants.
///
/// Shaping is where cosmic-text reveals a cursor a change left off screen, so
/// there is nothing to correct until it has run - and on a rebuilt `Content`
/// there are no line metrics to scroll by until then either. Each extra
/// scroll needs a shape of its own to settle before the frame draws.
pub(super) fn shape_and_reveal(
    editor: &mut graphics::text::Editor,
    pending: Option<PendingView>,
    text_bounds: Size,
    shape: impl Fn(&mut graphics::text::Editor),
) {
    shape(editor);

    let scroll = |editor: &mut graphics::text::Editor, lines| {
        if lines != 0 {
            editor.perform(Action::Scroll { lines });
            shape(editor);
        }
    };

    // The restore is the one scroll that has to land between two lines: it is
    // putting a view back, not counting lines onto it, and a view the user
    // left half a line down owes them that half line back. `Action::Scroll`
    // carries whole `lines: i32` and would round it away, snapping a bottom
    // row they left cut off flush against the edge.
    let scroll_exactly = |editor: &mut graphics::text::Editor, pixels: f32| {
        // The correction is absolute, so dropping a sub-pixel remainder
        // neither drifts nor compounds - it just saves a shape.
        if pixels.abs() >= 0.5 {
            editor.scroll_by(pixels);
            shape(editor);
        }
    };

    match pending {
        None => {}
        Some(PendingView::Edited {
            scrolled_to: before,
        }) => {
            if let Some(lines) = reveal_scroll(editor, before, text_bounds) {
                scroll(editor, lines);
            }
        }
        Some(PendingView::Spliced { caret_line }) => {
            // Nothing to put back: the splice never threw the view away. The
            // caret may have walked out of the safe area, though, and
            // cosmic-text only ever chases it as far as the bare edge.
            let Some(row) = cursor_row(editor) else {
                return;
            };
            let moved_by =
                editor.cursor().position.line as f32 - caret_line as f32;

            let rows = viewport_rows(editor, text_bounds);

            if let Some(lines) = restore_offset(row, moved_by, rows) {
                scroll(editor, lines);
            }
        }
        Some(PendingView::Rebuilt(before)) => {
            // The shape above revealed the cursor from the top of the
            // document, which is the one place the view was never at. Undoing
            // that is what makes the cursor's position mean anything.
            scroll_exactly(editor, restore_pixels(editor, before));

            if let Some(lines) = restore_scroll(editor, before, text_bounds) {
                scroll(editor, lines);
            }
        }
    }
}

/// The pixels from where the view sits now back to where `before` had it.
///
/// A pixel scroll moves in *visual* rows; a buffer scroll names a *logical*
/// line. The two come apart the moment a line wraps - which the widget does by
/// default (`Wrapping::default()` is `Word`, and nothing overrides it) - so
/// subtracting one `scrolled_to` from another and calling the difference
/// pixels lands the view somewhere it was never asked to go. That is what used
/// to yank the view on every line command in a document with a long line in
/// it: the restore missed, which left the cursor past the low boundary, and
/// the reveal below then "corrected" it onto that boundary.
///
/// So the gap is measured in rows that have actually been laid out. It spans
/// the handful of lines between a fresh buffer's reveal and the view it is
/// going back to, all of them in or beside the view and so already shaped.
/// This is *not* the whole-document row count the scrollbar deliberately
/// avoids (see its section in AGENTS.md): it is bounded, local, and runs once
/// per rebuild rather than every frame. A line cosmic-text has not shaped
/// falls back to counting logical lines, which is exact for a document that
/// does not wrap and only reachable when the cursor moved further than a
/// viewport - where the reveal is about to take over anyway.
fn restore_pixels(editor: &graphics::text::Editor, before: CapturedView) -> f32 {
    let buffer = editor.buffer();
    let now = buffer.scroll();

    let rows = (now.line.min(before.scroll_line)
        ..now.line.max(before.scroll_line))
        .try_fold(0.0f32, |rows, line| {
            Some(rows + buffer.lines.get(line)?.layout_opt()?.len() as f32)
        });

    let lines = match rows {
        Some(rows) if before.scroll_line >= now.line => rows,
        Some(rows) => -rows,
        None => before.scroll_line as f32 - now.line as f32,
    };

    lines * buffer.metrics().line_height + before.scroll_vertical - now.vertical
}

/// The scroll an edit still owes the view, in lines, measured against where
/// the view sat before it.
fn reveal_scroll(
    editor: &graphics::text::Editor,
    scrolled_before: f32,
    text_bounds: Size,
) -> Option<i32> {
    reveal_offset(
        scrolled_before,
        scrolled_to(editor)?,
        cursor_row(editor)?,
        viewport_rows(editor, text_bounds),
    )
}

/// How far to scroll after an edit, in lines, given where the view sat before
/// it, where cosmic-text left it after, and which row the cursor ended up on.
///
/// cosmic-text reveals an off-screen cursor by scrolling the bare minimum,
/// leaving it on the first or last visible row; this backs the view off by the
/// safe area's inset so it lands inside instead. `None` leaves the view alone:
/// either it never moved (the cursor was still on screen after the edit) or it
/// moved without chasing the cursor, which is what a document shrinking under
/// a view anchored at its end does.
pub(super) fn reveal_offset(
    scrolled_before: f32,
    scrolled_after: f32,
    cursor_row: f32,
    rows: f32,
) -> Option<i32> {
    let area = SafeArea::of(rows);
    if area.inset_line_count() == 0 {
        return None;
    }

    if scrolled_after < scrolled_before && area.on_first_row(cursor_row) {
        Some(-area.inset_line_count())
    } else if scrolled_after > scrolled_before && area.on_last_row(cursor_row) {
        Some(area.inset_line_count())
    } else {
        None
    }
}

/// The scroll a restored view owes the cursor, in lines, once it is back where
/// it was. Nothing reveals the cursor on this path - the `Content` the change
/// happened to is gone - so this places it outright rather than backing off an
/// edge cosmic-text already chased it to.
fn restore_scroll(
    editor: &graphics::text::Editor,
    before: CapturedView,
    text_bounds: Size,
) -> Option<i32> {
    let cursor_row = cursor_row(editor)?;

    restore_offset(
        cursor_row,
        cursor_row - before.cursor_row,
        viewport_rows(editor, text_bounds),
    )
}

/// How far to scroll to put a cursor a rebuilt view left outside the safe area
/// back onto its nearest boundary, or `None` to leave the view alone.
/// `moved_by` is the rows the cursor travelled, positive downwards.
///
/// A cursor still on screen only gets a scroll in the direction it is *going*.
/// Chasing it off whichever boundary it happens to be sitting past is what
/// made a line moved up scroll the view down, and the two rules answer
/// different questions: the safe area says the cursor is running out of
/// context ahead of it, `moved_by` says which way "ahead" is. A cursor already
/// off screen has no context either way and is placed whichever way it went.
///
/// Waiting for the cursor to leave the view entirely was the other half of
/// that: the caret crept onto the last visible row with nothing beneath it,
/// the view sat still, and then it jumped a whole inset at once when the caret
/// finally crossed.
pub(super) fn restore_offset(cursor_row: f32, moved_by: f32, rows: f32) -> Option<i32> {
    let area = SafeArea::of(rows);

    let target = if cursor_row < 0.0 {
        area.high()
    } else if cursor_row > area.last_row() {
        area.low()
    } else if cursor_row < area.high() && moved_by < 0.0 {
        area.high()
    } else if cursor_row > area.low() && moved_by > 0.0 {
        area.low()
    } else {
        return None;
    };

    Some((cursor_row - target).round() as i32)
}
