use super::*;

/// Absolute, so a row is exactly this many pixels whatever font the
/// machine running the tests happens to resolve.
const LINE_HEIGHT: f32 = 20.0;
const VIEW_ROWS: usize = 20;
/// Long enough to leave room to scroll on either side of the view.
const DOCUMENT_LINES: usize = 400;

/// A numbered document, shaped into a `VIEW_ROWS`-tall view the way
/// `layout` leaves it on the first frame.
fn document() -> (graphics::text::Editor, Size) {
    let mut editor = graphics::text::Editor::with_text(&document_text());
    let bounds = Size::new(400.0, VIEW_ROWS as f32 * LINE_HEIGHT);
    shape(&mut editor, bounds);

    (editor, bounds)
}

/// Wide enough that nothing under test comes near wrapping, which would
/// measure the bounds rather than the text.
const MEASURE_BOUNDS: Size = Size::new(400.0, 400.0);

/// How wide a one-line document lays out. `Wrapping::None`, so this is
/// the text's natural width rather than the bounds it was given.
fn drawn_width(text: &str, tab_width: u16) -> f32 {
    let mut editor = graphics::text::Editor::with_text(text);
    editor.set_tab_width(tab_width);
    shape(&mut editor, MEASURE_BOUNDS);

    editor.min_bounds().width
}

#[test]
fn a_tab_is_drawn_as_wide_as_the_tab_width_asks() {
    // The face is monospace, so a tab reaching a stop `width` columns
    // away covers exactly that many spaces - which is the only reason
    // this is measurable rather than merely bigger-than.
    for width in [2u16, 4, 8] {
        let spaces = " ".repeat(usize::from(width));
        assert_eq!(
            drawn_width("\tx", width),
            drawn_width(&format!("{spaces}x"), width),
            "a tab at width {width}"
        );
    }
}

#[test]
fn a_wider_tab_width_draws_a_wider_tab() {
    // Guards the direction as well as the arithmetic above: a stop table
    // read backwards would still make tabs and spaces agree.
    assert!(drawn_width("\tx", 8) > drawn_width("\tx", 2));
}

#[test]
fn changing_the_tab_width_redraws_a_line_already_shaped() {
    // What a `config.toml` reload needs: the buffer caches a line's
    // shaping, and a width that only applied to lines shaped afterwards
    // would leave every open document at whatever it opened with.
    let mut editor = graphics::text::Editor::with_text("\tx");
    editor.set_tab_width(2);
    shape(&mut editor, MEASURE_BOUNDS);
    let narrow = editor.min_bounds().width;

    editor.set_tab_width(8);
    shape(&mut editor, MEASURE_BOUNDS);

    assert!(
        editor.min_bounds().width > narrow,
        "the shaped line kept its old tab width"
    );
}

fn document_text() -> String {
    (0..DOCUMENT_LINES)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn shape(editor: &mut graphics::text::Editor, bounds: Size) {
    editor.update(
        bounds,
        iced_core::Font::MONOSPACE,
        Pixels(14.0),
        LineHeight::Absolute(Pixels(LINE_HEIGHT)),
        Wrapping::None,
        &mut highlighter::PlainText::new(&()),
    );
}

/// Edits the way the widget does: the scroll goes on record before the
/// edit, and the reveal rides along with the next shape.
fn perform(editor: &mut graphics::text::Editor, bounds: Size, edit: Edit) {
    let pending = scrolled_to(editor)
        .map(|scrolled_to| PendingView::Edited { scrolled_to });

    editor.perform(Action::Edit(edit));
    shape_and_reveal(editor, pending, bounds, |editor| {
        shape(editor, bounds);
    });
}

/// Rebuilds the editor the way undo and redo do: a fresh one under the
/// same document, with the view carried across by hand and the cursor put
/// back where the change happened.
fn rebuild(
    editor: &graphics::text::Editor,
    bounds: Size,
    line: usize,
) -> graphics::text::Editor {
    rebuild_as(editor, bounds, line, &document_text())
}

fn rebuild_as(
    editor: &graphics::text::Editor,
    bounds: Size,
    line: usize,
    text: &str,
) -> graphics::text::Editor {
    let pending = captured(editor).map(PendingView::Rebuilt);

    let mut rebuilt = graphics::text::Editor::with_text(text);
    rebuilt.move_to(Cursor {
        position: Position { line, column: 0 },
        selection: None,
    });
    shape_and_reveal(&mut rebuilt, pending, bounds, |editor| {
        shape(editor, bounds);
    });

    rebuilt
}

/// What `Content::capture_view` reads off a `Content` about to be
/// replaced, straight from the editor the harness drives by hand.
fn captured(editor: &graphics::text::Editor) -> Option<CapturedView> {
    let scroll = editor.buffer().scroll();

    Some(CapturedView {
        scroll_line: scroll.line,
        scroll_vertical: scroll.vertical,
        cursor_row: cursor_row(editor)?,
    })
}

/// Puts the cursor on `line`, then scrolls the view `lines` away from it.
fn scroll_away(
    editor: &mut graphics::text::Editor,
    bounds: Size,
    line: usize,
    lines: i32,
) {
    editor.move_to(Cursor {
        position: Position { line, column: 0 },
        selection: None,
    });
    shape(editor, bounds);

    editor.perform(Action::Scroll { lines });
    shape(editor, bounds);
}

/// Which visible row the cursor is on, counting from the top of the view.
/// Below zero or past `VIEW_ROWS` means it is off screen. Measured here
/// rather than through `cursor_row`, which is what is under test.
fn visible_row(editor: &graphics::text::Editor) -> f32 {
    match editor.selection() {
        Selection::Caret(position) => position.y / LINE_HEIGHT,
        Selection::Range(_) => panic!("no selection is under test"),
    }
}

/// The row the reveal aims for, counting from whichever edge the cursor
/// came in from.
const REVEALED_ROW: f32 = crate::safe_area::INSET_LINE_COUNT as f32;

#[test]
fn an_edit_with_the_cursor_on_screen_does_not_scroll() {
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, 40);
    // Back over the cursor, leaving it a few lines shy of either edge.
    editor.perform(Action::Scroll { lines: -25 });
    shape(&mut editor, bounds);

    let scrolled_before = scrolled_to(&editor);
    let row = visible_row(&editor);
    assert!((0.0..VIEW_ROWS as f32).contains(&row), "cursor off screen");

    perform(&mut editor, bounds, Edit::Insert('x'));

    assert_eq!(scrolled_to(&editor), scrolled_before);
    assert_eq!(visible_row(&editor), row);
}

#[test]
fn an_edit_above_the_view_reveals_the_cursor_six_lines_from_the_top() {
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, 60);

    perform(&mut editor, bounds, Edit::Insert('x'));

    assert_eq!(visible_row(&editor), REVEALED_ROW);
}

#[test]
fn an_edit_below_the_view_reveals_the_cursor_six_lines_from_the_bottom() {
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, -60);

    perform(&mut editor, bounds, Edit::Insert('x'));

    assert_eq!(visible_row(&editor), VIEW_ROWS as f32 - 1.0 - REVEALED_ROW);
}

#[test]
fn an_edit_one_line_off_the_edge_still_gets_the_whole_inset() {
    let (mut editor, bounds) = document();
    // The case cosmic-text on its own would settle with a single line of
    // scroll, leaving the cursor hard against the bottom edge.
    scroll_away(&mut editor, bounds, 100, -1);

    perform(&mut editor, bounds, Edit::Insert('x'));

    assert_eq!(visible_row(&editor), VIEW_ROWS as f32 - 1.0 - REVEALED_ROW);
}

#[test]
fn a_paste_that_carries_the_cursor_off_the_bottom_reveals_it() {
    let (mut editor, bounds) = document();
    // The cursor is on screen going in - it is the pasted lines pushing
    // it past the bottom edge that has to be chased.
    scroll_away(&mut editor, bounds, 10, 0);
    assert_eq!(visible_row(&editor), 10.0);

    perform(&mut editor, bounds, Edit::Paste(Arc::new("\n".repeat(30))));

    assert_eq!(visible_row(&editor), VIEW_ROWS as f32 - 1.0 - REVEALED_ROW);
}

#[test]
fn the_reveal_stops_at_the_top_of_the_document() {
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 2, 40);

    perform(&mut editor, bounds, Edit::Insert('x'));

    assert_eq!(scrolled_to(&editor), Some(0.0));
    assert_eq!(visible_row(&editor), 2.0);
}

#[test]
fn the_reveal_stops_at_the_end_of_the_document() {
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, DOCUMENT_LINES - 2, -60);

    perform(&mut editor, bounds, Edit::Insert('x'));

    // The last line of the document is already on the last visible row,
    // so the cursor lands one row above it rather than six.
    assert_eq!(
        scrolled_to(&editor),
        Some((DOCUMENT_LINES - VIEW_ROWS) as f32)
    );
    assert_eq!(visible_row(&editor), VIEW_ROWS as f32 - 2.0);
}

#[test]
fn an_undo_with_the_cursor_on_screen_does_not_scroll() {
    let (editor, bounds) = document();
    let view = scrolled_to(&editor);

    // The undone edit is on screen, ten rows down.
    let rebuilt = rebuild(&editor, bounds, 10);

    assert_eq!(scrolled_to(&rebuilt), view);
    assert_eq!(visible_row(&rebuilt), 10.0);
}

#[test]
fn an_undo_above_the_view_reveals_the_cursor_six_lines_from_the_top() {
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, 60);

    let rebuilt = rebuild(&editor, bounds, 100);

    assert_eq!(visible_row(&rebuilt), REVEALED_ROW);
}

#[test]
fn an_undo_below_the_view_reveals_the_cursor_six_lines_from_the_bottom() {
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, -60);

    let rebuilt = rebuild(&editor, bounds, 100);

    assert_eq!(
        visible_row(&rebuilt),
        VIEW_ROWS as f32 - 1.0 - REVEALED_ROW
    );
}

#[test]
fn an_undo_one_line_off_the_edge_still_gets_the_whole_inset() {
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, -1);

    let rebuilt = rebuild(&editor, bounds, 100);

    assert_eq!(
        visible_row(&rebuilt),
        VIEW_ROWS as f32 - 1.0 - REVEALED_ROW
    );
}

#[test]
fn a_line_moved_down_at_the_bottom_edge_scrolls_context_under_it() {
    // What a held line command used to do: each rebuild left the cursor
    // on the last visible row with nothing beneath it, the view sitting
    // still until the cursor finally crossed the edge and it jumped.
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, 0);
    assert_eq!(visible_row(&editor), VIEW_ROWS as f32 - 1.0);

    let rebuilt = rebuild(&editor, bounds, 101);

    assert_eq!(
        visible_row(&rebuilt),
        VIEW_ROWS as f32 - 1.0 - REVEALED_ROW
    );
}

#[test]
fn a_line_moved_up_off_the_low_boundary_leaves_the_view_alone() {
    // The cursor sits past the low boundary, but it is walking away from
    // that edge - scrolling down to "reveal" it would drag the view the
    // opposite way to the line the user is moving.
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, 2);
    assert_eq!(visible_row(&editor), VIEW_ROWS as f32 - 3.0);

    let view = scrolled_to(&editor).expect("a shaped view scrolls");
    let rebuilt = rebuild(&editor, bounds, 99);

    assert_eq!(scrolled_to(&rebuilt), Some(view));
    assert_eq!(visible_row(&rebuilt), VIEW_ROWS as f32 - 4.0);
}

#[test]
fn a_line_moved_down_off_the_high_boundary_leaves_the_view_alone() {
    // The same, at the other edge.
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, 17);
    assert_eq!(visible_row(&editor), 2.0);

    let view = scrolled_to(&editor).expect("a shaped view scrolls");
    let rebuilt = rebuild(&editor, bounds, 101);

    assert_eq!(scrolled_to(&rebuilt), Some(view));
    assert_eq!(visible_row(&rebuilt), 3.0);
}

#[test]
fn a_rebuild_keeps_a_view_that_sits_between_two_lines() {
    let (mut editor, bounds) = document();
    // Cursor comfortably mid-view, then half a line further down, so the
    // bottom row is cut in half and nothing is near enough an edge to
    // want a reveal.
    scroll_away(&mut editor, bounds, 100, 10);
    editor.scroll_by(LINE_HEIGHT / 2.0);
    shape(&mut editor, bounds);

    let view = scrolled_to(&editor).expect("a shaped view scrolls");
    assert_eq!(view.fract(), 0.5, "half a line in");

    let rebuilt = rebuild(&editor, bounds, 100);

    assert_eq!(
        scrolled_to(&rebuilt),
        Some(view),
        "the restored view owes the user the exact offset, cut row and all"
    );
}

#[test]
fn a_line_moved_up_under_a_cut_off_bottom_row_changes_nothing() {
    // The whole report in one case: scrolled half a line down, so the
    // bottom row is cut off, with the caret seven rows up from it -
    // counting the cut one. Alt+Up moves the caret and nothing else.
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 100, 5);
    editor.scroll_by(LINE_HEIGHT / 2.0);
    shape(&mut editor, bounds);

    let view = scrolled_to(&editor).expect("a shaped view scrolls");
    let row = visible_row(&editor);
    assert_eq!(view.fract(), 0.5, "the bottom row is cut in half");
    assert_eq!(row, VIEW_ROWS as f32 - REVEALED_ROW - 1.5, "seven up");

    let rebuilt = rebuild(&editor, bounds, 99);

    assert_eq!(scrolled_to(&rebuilt), Some(view), "the view must hold");
    assert_eq!(visible_row(&rebuilt), row - 1.0, "only the caret moves");
}

#[test]
fn an_undo_that_shortens_the_document_past_the_view_still_shows_the_cursor()
{
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 300, 0);

    // Undoing a paste: the document the view was scrolled into no longer
    // reaches that far, so the restored view clamps at its new end.
    let short = 40;
    let text = (0..short)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let rebuilt = rebuild_as(&editor, bounds, short - 1, &text);

    assert_eq!(scrolled_to(&rebuilt), Some((short - VIEW_ROWS) as f32));
    assert_eq!(visible_row(&rebuilt), VIEW_ROWS as f32 - 1.0);
}

#[test]
fn an_undo_reveal_stops_at_the_top_of_the_document() {
    let (mut editor, bounds) = document();
    scroll_away(&mut editor, bounds, 2, 40);

    let rebuilt = rebuild(&editor, bounds, 2);

    assert_eq!(scrolled_to(&rebuilt), Some(0.0));
    assert_eq!(visible_row(&rebuilt), 2.0);
}

/// A view 20 rows tall, the cursor on `cursor_row`, after an edit that
/// moved the view by `scrolled` lines.
fn offset(scrolled: f32, cursor_row: f32) -> Option<i32> {
    reveal_offset(100.0, 100.0 + scrolled, cursor_row, 20.0)
}

#[test]
fn a_view_that_did_not_move_stays_put() {
    assert_eq!(offset(0.0, 0.0), None);
    assert_eq!(offset(0.0, 10.0), None);
    assert_eq!(offset(0.0, 19.0), None);
}

#[test]
fn a_cursor_revealed_at_an_edge_backs_off_by_the_inset() {
    const INSET: i32 = crate::safe_area::INSET_LINE_COUNT;

    assert_eq!(offset(-40.0, 0.0), Some(-INSET));
    assert_eq!(offset(40.0, 19.0), Some(INSET));
}

#[test]
fn a_view_that_moved_without_chasing_the_cursor_stays_put() {
    // What a document shrinking under a view anchored at its end does:
    // the scroll clamps, but the cursor is nowhere near an edge.
    assert_eq!(offset(-3.0, 8.0), None);
    assert_eq!(offset(3.0, 8.0), None);
}

#[test]
fn the_inset_never_scrolls_the_cursor_past_the_middle() {
    // Five rows of view, so the inset gives up at two.
    assert_eq!(reveal_offset(100.0, 140.0, 4.0, 5.0), Some(2));
    assert_eq!(reveal_offset(100.0, 60.0, 0.0, 5.0), Some(-2));
}

#[test]
fn a_view_too_short_to_reveal_into_stays_put() {
    assert_eq!(reveal_offset(100.0, 140.0, 0.0, 2.0), None);
    assert_eq!(reveal_offset(100.0, 140.0, 0.0, 1.0), None);
    assert_eq!(reveal_offset(100.0, 140.0, 0.0, 0.0), None);
}

/// A view 20 rows tall - six from the top is row five, six from the
/// bottom is row fourteen - with the cursor come to rest on `cursor_row`
/// after travelling `moved_by` rows, positive downwards.
fn restored(cursor_row: f32, moved_by: f32) -> Option<i32> {
    restore_offset(cursor_row, moved_by, 20.0)
}

#[test]
fn a_restored_view_places_a_cursor_it_left_off_screen() {
    // Off screen is off screen: there is no context to preserve either
    // side of it, so it comes back whichever way it went.
    assert_eq!(restored(-8.0, -30.0), Some(-13));
    assert_eq!(restored(25.0, 30.0), Some(11));
    assert_eq!(restored(-8.0, 30.0), Some(-13));
    assert_eq!(restored(25.0, -30.0), Some(11));
}

#[test]
fn a_restored_view_only_scrolls_the_way_the_cursor_went() {
    // A line moved *up* used to scroll the view *down*, purely because
    // the cursor was sitting past the low boundary at the time.
    assert_eq!(restored(19.0, -1.0), None);
    assert_eq!(restored(0.0, 1.0), None);
    // Heading for the edge it is near, though, and the view follows.
    assert_eq!(restored(19.0, 1.0), Some(5));
    assert_eq!(restored(0.0, -1.0), Some(-5));
}

#[test]
fn a_restored_view_stays_put_inside_the_safe_area() {
    // Mid-view there is context to spare, whichever way the cursor went.
    assert_eq!(restored(10.0, 1.0), None);
    assert_eq!(restored(10.0, -1.0), None);
    // A last row the view only half shows counts as outside.
    assert_eq!(restore_offset(20.0, 1.0, 20.4), Some(6));
}

#[test]
fn a_cursor_that_did_not_move_leaves_the_view_alone() {
    // Deleting the line under the cursor keeps the caret on its row, so
    // there is nothing to reveal even sitting outside a boundary.
    assert_eq!(restored(17.0, 0.0), None);
    assert_eq!(restored(2.0, 0.0), None);
}

#[test]
fn a_cursor_just_outside_a_boundary_is_pushed_back_onto_it() {
    // The boundaries themselves are where the cursor is allowed to rest:
    // one row further out and the view follows it by exactly that row.
    assert_eq!(restored(5.0, -1.0), None);
    assert_eq!(restored(14.0, 1.0), None);
    assert_eq!(restored(4.0, -1.0), Some(-1));
    assert_eq!(restored(15.0, 1.0), Some(1));
}

#[test]
fn a_restored_view_too_short_for_the_whole_inset_still_lands_inside_it() {
    // Five rows of view: the cursor comes to rest two rows in, from
    // whichever edge it was past.
    assert_eq!(restore_offset(9.0, 1.0, 5.0), Some(7));
    assert_eq!(restore_offset(-9.0, -1.0, 5.0), Some(-11));
}

#[test]
fn a_sub_line_scroll_leaves_the_view_between_two_lines() {
    // The whole feature, at its smallest: a quarter of a line in, and the
    // view rests a quarter of a line down - the top row clipped by five
    // pixels rather than snapped back to its own top edge.
    let (mut editor, bounds) = document();
    assert_eq!(scrolled_to(&editor), Some(0.0));

    editor.scroll_by(LINE_HEIGHT / 4.0);
    shape(&mut editor, bounds);

    assert_eq!(scrolled_to(&editor), Some(0.25));
}

#[test]
fn sub_line_scrolls_accumulate_across_a_line_boundary() {
    // Nothing banks the remainder any more, so crossing a line has to
    // fall out of the buffer's own arithmetic: three quarter-lines sit
    // inside line 0, the fourth rolls over into line 1 with nothing left.
    let (mut editor, bounds) = document();

    for expected in [0.25, 0.5, 0.75, 1.0] {
        editor.scroll_by(LINE_HEIGHT / 4.0);
        shape(&mut editor, bounds);
        assert_eq!(scrolled_to(&editor), Some(expected));
    }

    // And the rollover really did advance the buffer's line, rather than
    // parking a whole line's worth in the sub-line offset.
    assert_eq!(editor.buffer().scroll().line, 1);
    assert_eq!(editor.buffer().scroll().vertical, 0.0);
}

#[test]
fn scrolling_up_from_a_sub_line_offset_is_symmetric() {
    let (mut editor, bounds) = document();

    editor.scroll_by(LINE_HEIGHT * 3.5);
    shape(&mut editor, bounds);
    assert_eq!(scrolled_to(&editor), Some(3.5));

    editor.scroll_by(-LINE_HEIGHT * 0.25);
    shape(&mut editor, bounds);
    assert_eq!(scrolled_to(&editor), Some(3.25));

    // Back across a line boundary, which is where an offset kept as a
    // positive remainder has to borrow from the line above.
    editor.scroll_by(-LINE_HEIGHT * 0.5);
    shape(&mut editor, bounds);
    assert_eq!(scrolled_to(&editor), Some(2.75));
    assert_eq!(editor.buffer().scroll().line, 2);
}

#[test]
fn the_whole_line_action_still_snaps_and_is_still_what_reveal_uses() {
    // The contrast that makes the two levers worth having. `scroll_by` is
    // for pointing at a position; `Action::Scroll` is for counting lines,
    // which is what the cursor reveal in `shape_and_reveal` wants.
    let (mut editor, bounds) = document();

    editor.scroll_by(LINE_HEIGHT / 2.0);
    shape(&mut editor, bounds);
    assert_eq!(scrolled_to(&editor), Some(0.5));

    // A whole-line action moves by whole lines and *preserves* the
    // sub-line offset rather than re-snapping to a boundary - which is
    // what lets the reveal run without visibly straightening the view.
    editor.perform(Action::Scroll { lines: 2 });
    shape(&mut editor, bounds);
    assert_eq!(scrolled_to(&editor), Some(2.5));
}

#[test]
fn a_scroll_of_zero_pixels_does_nothing() {
    let (mut editor, bounds) = document();
    editor.scroll_by(LINE_HEIGHT * 2.0);
    shape(&mut editor, bounds);

    editor.scroll_by(0.0);
    shape(&mut editor, bounds);

    assert_eq!(scrolled_to(&editor), Some(2.0));
}

fn text_area() -> Rectangle {
    Rectangle::new(Point::new(25.0, 65.0), Size::new(450.0, 290.0))
}

#[test]
fn the_text_clip_is_one_the_editor_does_not_fit_inside() {
    // The whole reason it exists: a clip the text fits inside is one the
    // renderer skips building a mask for, and an editor's rows overhang
    // its bounds.
    assert!(!text_area().is_within(&text_clip(text_area())));
}

#[test]
fn the_text_clip_still_covers_every_pixel_of_the_text() {
    // The mask is not anti-aliased, so a pixel is in it if its centre is.
    for height in [290.0, 289.5, 17.3, 1.0] {
        let text_bounds = Rectangle {
            height,
            ..text_area()
        };
        let last_pixel_centre =
            (text_bounds.y + text_bounds.height).floor() - 0.5;
        let clip = text_clip(text_bounds);

        assert!(
            clip.y + clip.height > last_pixel_centre,
            "a {height}px text area lost its last row of pixels"
        );
    }
}

#[test]
fn a_text_area_too_short_to_shorten_stays_a_rectangle() {
    // Negative dimensions panic on their way into the renderer, and a
    // sliver of an editor is not worth one.
    let sliver = Rectangle {
        height: 0.05,
        ..text_area()
    };

    assert_eq!(text_clip(sliver).height, 0.0);
}

/// A widget sitting somewhere other than the window's corner - a tab bar
/// above it, say - so a pointer's window coordinates and its coordinates
/// in the text can't be mistaken for each other.
fn widget_bounds() -> Rectangle {
    Rectangle::new(Point::new(30.0, 50.0), Size::new(400.0, 400.0))
}

/// Where a pointer at `(x, y)` in the window lands in the text.
fn pointed_at(x: f32, y: f32) -> Option<Point> {
    text_position(
        mouse::Cursor::Available(Point::new(x, y)),
        widget_bounds(),
        Padding::new(5.0),
    )
}

#[test]
fn a_pointer_on_the_text_lands_inside_it() {
    assert_eq!(pointed_at(35.0, 55.0), Some(Point::ORIGIN));
    assert_eq!(pointed_at(135.0, 155.0), Some(Point::new(100.0, 100.0)));
}

#[test]
fn a_pointer_off_the_widget_keeps_its_place_past_the_edges() {
    // What a selection drag out of the window rides on: a position past
    // the text rather than no position at all, so the editor can carry
    // on hit-testing it against its nearest row.
    assert_eq!(pointed_at(0.0, 0.0), Some(Point::new(-35.0, -55.0)));
    assert_eq!(pointed_at(1000.0, 1000.0), Some(Point::new(965.0, 945.0)));
}

#[test]
fn a_pointer_the_window_cannot_place_has_no_position() {
    assert_eq!(
        text_position(
            mouse::Cursor::Unavailable,
            widget_bounds(),
            Padding::new(5.0),
        ),
        None,
    );
}

fn notch(y: f32) -> mouse::ScrollDelta {
    mouse::ScrollDelta::Lines { x: 0.0, y }
}

fn precise(y: f32) -> mouse::ScrollDelta {
    mouse::ScrollDelta::Pixels { x: 0.0, y }
}

#[test]
fn sensitivity_scales_the_wheel_in_both_directions() {
    // Wheel `y` is positive scrolling up, and the editor counts lines
    // down, so the sign flips on the way through.
    assert_eq!(wheel_lines(notch(-1.0), 1.0), LINES_PER_WHEEL_NOTCH);
    assert_eq!(wheel_lines(notch(1.0), 1.0), -LINES_PER_WHEEL_NOTCH);

    assert_eq!(wheel_lines(notch(-1.0), 2.0), LINES_PER_WHEEL_NOTCH * 2.0);
    assert_eq!(wheel_lines(notch(-1.0), 0.5), LINES_PER_WHEEL_NOTCH / 2.0);
}

#[test]
fn sensitivity_scales_a_precise_device_the_same_way() {
    assert_eq!(wheel_lines(precise(-PIXELS_PER_LINE), 1.0), 1.0);
    assert_eq!(wheel_lines(precise(-PIXELS_PER_LINE), 0.5), 0.5);
    assert_eq!(wheel_lines(precise(PIXELS_PER_LINE), 2.0), -2.0);
}

#[test]
fn the_shipped_speed_is_half_of_upstream_iced() {
    // The reason `[scroll] sensitivity` defaults to 1.0 rather than 0.5:
    // the knob reads as a multiplier on what JumpPad ships, and what it
    // ships is half of `iced_widget`'s 4 lines a notch / 4 pixels a line.
    assert_eq!(wheel_lines(notch(-1.0), 1.0), 4.0 / 2.0);
    assert_eq!(wheel_lines(precise(-4.0), 1.0), 1.0 / 2.0);
}

#[test]
fn a_low_sensitivity_still_moves_the_view() {
    // The floor in `wheel_lines` is on the notch count, not on the
    // result, so a small multiplier keeps its fraction - which
    // `partial_scroll` banks - instead of rounding to a dead wheel.
    let lines = wheel_lines(notch(-1.0), *SCROLL_MULTIPLIER_RANGE.start());
    assert!(lines > 0.0 && lines < 1.0, "{lines}");
}

#[test]
fn a_fraction_of_a_notch_still_counts_as_a_whole_one() {
    // Upstream's floor, kept: a device reporting 0.1 of a notch must not
    // scroll a tenth as far as one that reports a whole notch.
    assert_eq!(wheel_lines(notch(-0.1), 1.0), 1.0);
    assert_eq!(wheel_lines(notch(0.0), 1.0), 0.0);
}

#[test]
fn a_nonsense_multiplier_lands_somewhere_usable() {
    assert_eq!(clamp_scroll_multiplier(1.0), 1.0);
    assert_eq!(
        clamp_scroll_multiplier(0.0),
        *SCROLL_MULTIPLIER_RANGE.start()
    );
    assert_eq!(
        clamp_scroll_multiplier(-3.0),
        *SCROLL_MULTIPLIER_RANGE.start()
    );
    assert_eq!(
        clamp_scroll_multiplier(1e9),
        *SCROLL_MULTIPLIER_RANGE.end()
    );
    // No end of the range to clamp `NaN` to, so it takes the default.
    assert_eq!(clamp_scroll_multiplier(f32::NAN), 1.0);
}

/// Every line long enough to wrap at the harness width, so visual rows
/// and buffer lines come apart - which is what the widget actually runs
/// (`Wrapping::default()` is `Word`, and nothing overrides it).
fn wrapped_text() -> String {
    (0..DOCUMENT_LINES)
        .map(|line| format!("line {line} {}", "word ".repeat(20)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn shape_wrapped(editor: &mut graphics::text::Editor, bounds: Size) {
    editor.update(
        bounds,
        iced_core::Font::MONOSPACE,
        Pixels(14.0),
        LineHeight::Absolute(Pixels(LINE_HEIGHT)),
        Wrapping::Word,
        &mut highlighter::PlainText::new(&()),
    );
}

/// A wrapped document with the cursor on line 100 and the view scrolled
/// `lines` back over it, then that line moved down one - the way a line
/// command rebuilds. Returns the view and cursor row either side.
#[allow(clippy::type_complexity)]
fn wrapped_line_move(lines: i32) -> ((usize, f32), f32, (usize, f32), f32) {
    let bounds = Size::new(400.0, VIEW_ROWS as f32 * LINE_HEIGHT);
    let mut editor = graphics::text::Editor::with_text(&wrapped_text());
    shape_wrapped(&mut editor, bounds);
    editor.move_to(Cursor {
        position: Position {
            line: 100,
            column: 0,
        },
        selection: None,
    });
    shape_wrapped(&mut editor, bounds);
    editor.perform(Action::Scroll { lines });
    shape_wrapped(&mut editor, bounds);

    let before = editor.buffer().scroll();
    let before_row = visible_row(&editor);
    let pending = captured(&editor).map(PendingView::Rebuilt);

    let mut rebuilt = graphics::text::Editor::with_text(&wrapped_text());
    rebuilt.move_to(Cursor {
        position: Position {
            line: 101,
            column: 0,
        },
        selection: None,
    });
    shape_and_reveal(&mut rebuilt, pending, bounds, |editor| {
        shape_wrapped(editor, bounds);
    });

    let after = rebuilt.buffer().scroll();
    (
        (before.line, before.vertical),
        before_row,
        (after.line, after.vertical),
        visible_row(&rebuilt),
    )
}

/// A line command as it now runs: the buffer is kept, the caret walks
/// `by` lines, and the reveal gets the line it started on. Returns the
/// scroll and caret row either side.
#[allow(clippy::type_complexity)]
fn spliced_line_move(
    wrap: bool,
    scroll_by: i32,
    by: isize,
) -> ((usize, f32), f32, (usize, f32), f32) {
    spliced_line_move_maybe(wrap, scroll_by, by, true)
}

/// With `reveal` off, the same walk with no safe-area logic at all - which
/// is where the caret naturally lands, and so what the reveal should be
/// judged against.
#[allow(clippy::type_complexity)]
fn spliced_line_move_maybe(
    wrap: bool,
    scroll_by: i32,
    by: isize,
    reveal: bool,
) -> ((usize, f32), f32, (usize, f32), f32) {
    let bounds = Size::new(400.0, VIEW_ROWS as f32 * LINE_HEIGHT);
    let text = if wrap {
        wrapped_text()
    } else {
        document_text()
    };
    let lay_out = |e: &mut graphics::text::Editor, bounds| {
        if wrap {
            shape_wrapped(e, bounds);
        } else {
            shape(e, bounds);
        }
    };

    let mut editor = graphics::text::Editor::with_text(&text);
    lay_out(&mut editor, bounds);
    editor.move_to(Cursor {
        position: Position {
            line: 100,
            column: 0,
        },
        selection: None,
    });
    lay_out(&mut editor, bounds);
    editor.perform(Action::Scroll { lines: scroll_by });
    lay_out(&mut editor, bounds);

    let before = editor.buffer().scroll();
    let before_row = visible_row(&editor);
    let caret_line = editor.cursor().position.line;

    // The splice itself only matters here for where it leaves the caret.
    editor.move_to(Cursor {
        position: Position {
            line: caret_line.saturating_add_signed(by),
            column: 0,
        },
        selection: None,
    });
    shape_and_reveal(
        &mut editor,
        reveal.then_some(PendingView::Spliced { caret_line }),
        bounds,
        |editor| lay_out(editor, bounds),
    );

    let after = editor.buffer().scroll();
    (
        (before.line, before.vertical),
        before_row,
        (after.line, after.vertical),
        visible_row(&editor),
    )
}

#[test]
fn a_spliced_line_landing_inside_the_safe_area_never_moves_the_view() {
    // The whole point of splicing in place: there is no view to restore,
    // so there is nothing to restore it *wrongly*. Judged against where
    // the caret lands with no reveal at all, because one line of travel
    // is three rows in a wrapped document - a caret that looks mid-view
    // can land outside a boundary honestly.
    let area = SafeArea::of(VIEW_ROWS as f32);
    let mut checked = 0;

    for wrap in [true, false] {
        for scroll_by in [4, 6, 8, 10, 11, 12, 14] {
            for by in [-1, 1] {
                let (_, _, natural_view, natural_row) =
                    spliced_line_move_maybe(wrap, scroll_by, by, false);
                if !(area.high()..=area.low()).contains(&natural_row) {
                    continue;
                }

                let (_, _, revealed_view, revealed_row) =
                    spliced_line_move(wrap, scroll_by, by);
                let case = format!("wrap={wrap} scroll({scroll_by}) {by}");
                assert_eq!(revealed_view, natural_view, "{case}: view");
                assert_eq!(revealed_row, natural_row, "{case}: caret");
                checked += 1;
            }
        }
    }

    assert!(
        checked >= 10,
        "only {checked} cases landed in the safe area"
    );
}

#[test]
fn a_spliced_line_past_the_low_boundary_still_reveals() {
    for wrap in [true, false] {
        let (view, _, moved_view, moved_row) =
            spliced_line_move(wrap, 0, 1);

        assert_ne!(moved_view, view, "wrap={wrap}: view should follow");
        assert!(
            moved_row <= VIEW_ROWS as f32 - 1.0 - REVEALED_ROW,
            "wrap={wrap}: caret should be past it, at {moved_row}"
        );
    }
}

#[test]
fn a_spliced_line_moving_back_off_the_low_boundary_holds_still() {
    // Sitting past the boundary but walking away from it - the case that
    // scrolled the view the wrong way.
    for wrap in [true, false] {
        let (view, row, moved_view, moved_row) =
            spliced_line_move(wrap, 0, -1);

        assert_eq!(moved_view, view, "wrap={wrap}: view moved");
        assert!(moved_row < row, "wrap={wrap}: caret should walk up");
    }
}

#[test]
fn a_wrapped_documents_lines_really_do_wrap() {
    // Guards every case below: the moment this text stops wrapping they
    // all pass for the wrong reason, which is exactly how the bug they
    // cover got in.
    let bounds = Size::new(400.0, VIEW_ROWS as f32 * LINE_HEIGHT);
    let mut editor = graphics::text::Editor::with_text(&wrapped_text());
    shape_wrapped(&mut editor, bounds);

    let rows = editor.buffer().lines[100]
        .layout_opt()
        .map(|layout| layout.len());
    assert!(rows > Some(1), "line 100 should wrap, laid out as {rows:?}");
}

/// One number to a line, on the row the line begins on - which is the whole
/// point of numbering a document rather than a screen.
#[test]
fn a_wrapped_line_is_numbered_once_on_the_row_it_begins_on() {
    let bounds = Size::new(400.0, VIEW_ROWS as f32 * LINE_HEIGHT);
    let mut editor = graphics::text::Editor::with_text(&wrapped_text());
    shape_wrapped(&mut editor, bounds);

    let rows: Vec<_> = crate::line_numbers::rows(editor.buffer()).collect();
    let numbered: Vec<usize> = rows
        .iter()
        .filter(|row| row.starts_line)
        .map(|row| row.line)
        .collect();
    let on_screen: std::collections::BTreeSet<usize> =
        rows.iter().map(|row| row.line).collect();

    assert!(
        rows.len() > numbered.len(),
        "nothing on screen wrapped, so this proves nothing"
    );
    assert_eq!(
        numbered,
        on_screen.into_iter().collect::<Vec<_>>(),
        "every line on screen should be numbered exactly once, in order"
    );
    assert!(
        rows[0].starts_line,
        "a view at the top of the document starts on a line, not inside one"
    );
}

/// The rows above the top edge are dropped without a word, so the first row
/// `layout_runs` yields is as likely to be a continuation as a beginning -
/// and numbering it would put a number halfway down a paragraph.
#[test]
fn a_view_scrolled_into_a_wrapped_line_leaves_the_top_row_blank() {
    let bounds = Size::new(400.0, VIEW_ROWS as f32 * LINE_HEIGHT);
    let mut editor = graphics::text::Editor::with_text(&wrapped_text());
    shape_wrapped(&mut editor, bounds);
    editor.perform(Action::Scroll { lines: 1 });
    shape_wrapped(&mut editor, bounds);

    let scroll = editor.buffer().scroll();
    assert_eq!(scroll.line, 0, "the view should still be inside line 0");
    assert!(scroll.vertical > 0.0, "the view should be inside a line");

    let top = crate::line_numbers::rows(editor.buffer())
        .next()
        .expect("a row on screen");

    assert_eq!(top.line, 0);
    assert!(
        !top.starts_line,
        "the top row is a row line 0 wrapped onto, and carries no number"
    );
}

#[test]
fn a_line_moved_down_a_wrapped_document_leaves_the_view_alone() {
    // The report: cursor anywhere inside the safe area, move the line,
    // and the view has no business moving. `scrolled_to` mixes a logical
    // line with a visual offset, so restoring by its difference used to
    // miss - and the miss dropped the cursor past the low boundary,
    // where the reveal slammed it onto that boundary every time.
    for lines in [8, 10, 11, 12, 14] {
        let (view, row, moved_view, moved_row) = wrapped_line_move(lines);

        assert_eq!(moved_view, view, "scroll({lines}) moved the view");
        assert!(
            moved_row > row,
            "scroll({lines}): the caret should walk down the screen, \
             {row} -> {moved_row}"
        );
    }
}

#[test]
fn a_wrapped_line_moved_past_the_low_boundary_still_reveals() {
    // The safe area has to keep working under wrapping, or the fix
    // above is just the reveal switched off.
    let (view, _, moved_view, moved_row) = wrapped_line_move(4);

    assert_ne!(moved_view, view, "the view should follow the caret down");
    assert!(
        moved_row <= VIEW_ROWS as f32 - 1.0 - REVEALED_ROW,
        "the caret should land past the low boundary, at {moved_row}"
    );
}

/// The reference for any future pixel-granular scrolling: the buffer
/// underneath already *holds and renders* a sub-line offset. Only the
/// way in is quantized - `iced_core`'s `Action::Scroll` carries whole
/// `lines: i32`, and `iced_graphics` multiplies that by the line height
/// on the way to cosmic-text's pixel-valued scroll. Nothing here needs
/// custom drawing; it needs a fractional lever iced doesn't expose yet.
#[test]
fn the_buffer_can_sit_between_two_lines() {
    // A view whose height is *not* a whole number of rows, scrolled hard
    // against the end of the document. cosmic-text clamps that by pixels
    // (`shape_until_scroll`), so this is where a sub-line offset shows.
    let mut editor = graphics::text::Editor::with_text(&document_text());
    let bounds = Size::new(400.0, 19.5 * LINE_HEIGHT);
    shape(&mut editor, bounds);

    editor.perform(Action::Scroll { lines: 10_000 });
    shape(&mut editor, bounds);

    let scroll = editor.buffer().scroll();
    assert_eq!(
        scroll.vertical,
        LINE_HEIGHT / 2.0,
        "the top row should be half cut off, not snapped to a line"
    );
    // And `scrolled_to` reports it - the scrollbar reads through this,
    // which is why the thumb is already smooth where the text is not.
    assert_eq!(
        scrolled_to(&editor),
        Some(scroll.line as f32 + 0.5),
        "a half-line offset must survive into the reported position"
    );
}

/// Any width: the geometry a thumb drag turns on is vertical.
const THUMB_WIDTH: f32 = 12.0;

/// Long enough to be a frame of its own rather than a second pass over
/// the one before it, which is a distinction a thumb drag draws.
const FRAME: Duration = Duration::from_millis(16);

/// The scrollbar as the widget builds it, over a text area the size of the
/// view the buffer was shaped into.
fn scrollbar_of(
    state: &scrollbar::State,
    editor: &graphics::text::Editor,
    bounds: Size,
) -> scrollbar::Layout {
    scrollbar_layout(
        state,
        editor,
        Rectangle::new(Point::ORIGIN, bounds),
        THUMB_WIDTH,
    )
    .expect("a shaped buffer has metrics")
}

/// Drags the thumb to `y` the way the widget does: the pointer lands where
/// the drag is heading, and every frame after asks what the view still
/// owes it, scrolls by that, and shapes - which is where cosmic-text works
/// out what those pixels actually covered. Returns the frames it took.
fn drag_thumb_to(
    editor: &mut graphics::text::Editor,
    bounds: Size,
    y: f32,
) -> usize {
    let mut now = Instant::now();
    let mut state = scrollbar::State::default();
    let scrollbar = scrollbar_of(&state, editor, bounds);
    let thumb = scrollbar
        .thumb
        .expect("a document taller than its view has a thumb");

    assert!(state.press(thumb.center(), scrollbar, now));
    state.drag_to(Point::new(thumb.center_x(), y), now);

    let mut frames = 0;
    while let Some(pixels) =
        state.scroll_to_pointer(scrollbar_of(&state, editor, bounds), now)
    {
        editor.scroll_by(pixels);
        shape_wrapped(editor, bounds);
        now += FRAME;

        frames += 1;
        assert!(frames < 100, "the drag never arrived");
    }

    frames
}

/// A document of wrapped lines, shaped into a `VIEW_ROWS`-tall view.
fn wrapped_document() -> (graphics::text::Editor, Size) {
    let bounds = Size::new(400.0, VIEW_ROWS as f32 * LINE_HEIGHT);
    let mut editor = graphics::text::Editor::with_text(&wrapped_text());
    shape_wrapped(&mut editor, bounds);

    (editor, bounds)
}

/// Short enough that the thumb's length is clear of `MIN_THUMB_HEIGHT`,
/// where a change in it would be clamped away rather than shown.
const SHORT_DOCUMENT_LINES: usize = 60;

/// A document that wraps in blocks - a screenful of short lines, then a
/// screenful of long ones - so the rows a screen holds per line change as
/// the view moves down it.
fn mixed_document() -> (graphics::text::Editor, Size) {
    short_document(|line| {
        if (line / VIEW_ROWS).is_multiple_of(2) {
            format!("line {line}")
        } else {
            format!("line {line} {}", "word ".repeat(20))
        }
    })
}

fn short_document(
    line: impl Fn(usize) -> String,
) -> (graphics::text::Editor, Size) {
    let text = (0..SHORT_DOCUMENT_LINES)
        .map(line)
        .collect::<Vec<_>>()
        .join("\n");

    let bounds = Size::new(400.0, VIEW_ROWS as f32 * LINE_HEIGHT);
    let mut editor = graphics::text::Editor::with_text(&text);
    shape_wrapped(&mut editor, bounds);

    (editor, bounds)
}

#[test]
fn the_scrollbar_measures_the_document_in_rows_rather_than_lines() {
    // A document of lines that wrap into three rows each is three times
    // as tall as its line count says, and both the thumb's length and how
    // far a drag can carry it are drawn from that height. Counting lines
    // had the thumb believe the document ended a screenful above where it
    // does.
    let (flat, bounds) = document();
    let flat = scrollbar::State::default()
        .metrics(flat.buffer(), bounds)
        .unwrap();
    assert_eq!(flat.viewport, VIEW_ROWS as f32);
    assert_eq!(flat.content, DOCUMENT_LINES as f32, "no line wraps here");

    let (wrapped, bounds) = wrapped_document();
    let wrapped = scrollbar::State::default()
        .metrics(wrapped.buffer(), bounds)
        .unwrap();
    assert_eq!(wrapped.viewport, VIEW_ROWS as f32);
    assert!(
        wrapped.content > flat.content * 2.0,
        "lines wrapping into three rows should count for about three \
         times the document, not {}",
        wrapped.content
    );
}

#[test]
fn dragging_the_thumb_to_the_bottom_reaches_the_end_of_a_wrapped_document()
{
    // The report: drag the thumb down a document with wrapped lines in it
    // and it stops short, leaving lines below that the wheel can still
    // reach. A scroll moves in rows where the thumb counts in lines, so
    // the pixels each frame asked for covered fewer lines than it wanted.
    let (mut editor, bounds) = wrapped_document();

    let frames = drag_thumb_to(&mut editor, bounds, 10_000.0);
    assert!(
        (1..=2).contains(&frames),
        "the drag should arrive in a frame or two, not {frames}"
    );

    let scrollbar =
        scrollbar_of(&scrollbar::State::default(), &editor, bounds);
    let thumb = scrollbar.thumb.expect("still scrollable at the end");
    assert!(
        (thumb.y + thumb.height
            - (scrollbar.track.y + scrollbar.track.height))
            .abs()
            < 0.5,
        "the thumb should come to rest against the end of its track"
    );

    // And there is nothing below it: the wheel finds the same place, give
    // or take the half pixel a drag stops bothering to ask for.
    let dragged_to = editor.buffer().scroll();
    editor.scroll_by(LINE_HEIGHT * 10.0);
    shape_wrapped(&mut editor, bounds);

    let end = editor.buffer().scroll();
    assert_eq!(dragged_to.line, end.line, "{dragged_to:?} vs {end:?}");
    assert!(
        (dragged_to.vertical - end.vertical).abs()
            < scrollbar::SMALLEST_DRAG_SCROLL,
        "the wheel could still reach document the thumb had run out of: \
         {dragged_to:?} against {end:?}"
    );
}

#[test]
fn dragging_the_thumb_to_the_top_reaches_the_start_of_a_wrapped_document() {
    // The other half of the report: from the end of the file, drag the
    // thumb up and it used to come to rest a little short of the top, on a
    // line with more of the document still above it.
    let (mut editor, bounds) = wrapped_document();
    editor.scroll_by(LINE_HEIGHT * DOCUMENT_LINES as f32 * 5.0);
    shape_wrapped(&mut editor, bounds);
    assert!(editor.buffer().scroll().line > 0, "should be at the end");

    let frames = drag_thumb_to(&mut editor, bounds, -10_000.0);
    assert!(
        (1..=2).contains(&frames),
        "the drag should arrive in a frame or two, not {frames}"
    );

    let top = editor.buffer().scroll();
    assert_eq!(top.line, 0, "{top:?}");
    assert!(
        top.vertical < scrollbar::SMALLEST_DRAG_SCROLL,
        "the thumb came to rest short of the top: {top:?}"
    );
}

#[test]
fn a_wrapped_document_gets_a_shorter_thumb_than_a_flat_one() {
    // The same number of lines, but a third of them three rows tall, is a
    // taller document - and shows less of itself at once, which is what
    // the thumb's length is for.
    let state = scrollbar::State::default();

    let (flat, bounds) = short_document(|line| format!("line {line}"));
    let flat = scrollbar_of(&state, &flat, bounds)
        .thumb
        .expect("taller than its view");

    let (mixed, bounds) = mixed_document();
    let mixed = scrollbar_of(&scrollbar::State::default(), &mixed, bounds)
        .thumb
        .expect("taller than its view");

    assert!(
        mixed.height < flat.height,
        "wrapped lines should count for the rows they take: \
         {} against {}",
        mixed.height,
        flat.height
    );
}

#[test]
fn dragging_to_the_bottom_of_a_mixed_document_lands_on_the_end() {
    // Blocks of wrapped and unwrapped lines are the hard case for a
    // measure of the document's height, and the case a measure taken from
    // the view could never settle in at all: the rows a screen holds
    // change as the drag crosses a block, so the row it was aiming for
    // moved out from under it and the drag chased it up and down the
    // document instead of arriving.
    let (mut editor, bounds) = mixed_document();

    drag_thumb_to(&mut editor, bounds, 10_000.0);

    let scrollbar =
        scrollbar_of(&scrollbar::State::default(), &editor, bounds);
    let thumb = scrollbar.thumb.expect("taller than its view");
    assert!(
        (thumb.y + thumb.height
            - (scrollbar.track.y + scrollbar.track.height))
            .abs()
            < 1.0,
        "the thumb should come to rest against the end of its track"
    );

    let dragged_to = editor.buffer().scroll();
    editor.scroll_by(LINE_HEIGHT * 10.0);
    shape_wrapped(&mut editor, bounds);
    assert_eq!(
        editor.buffer().scroll().line,
        dragged_to.line,
        "the wheel could still reach document the thumb had run out of"
    );
}

#[test]
fn the_thumbs_length_holds_still_while_it_is_dragged() {
    // What the thumb's length shows is how tall the document is, which
    // does not change as you drag - so neither can the length. Measuring
    // it from the lines on screen swung it between tall and short every
    // time the drag crossed from short lines into a paragraph.
    let (mut editor, bounds) = mixed_document();
    let mut now = Instant::now();
    let mut state = scrollbar::State::default();

    let scrollbar = scrollbar_of(&state, &editor, bounds);
    let thumb = scrollbar.thumb.expect("taller than its view");
    assert!(state.press(thumb.center(), scrollbar, now));

    for step in 1..=20u8 {
        let down = scrollbar.track.height * f32::from(step) / 20.0;
        state.drag_to(
            Point::new(thumb.center_x(), scrollbar.track.y + down),
            now,
        );

        let mut frames = 0;
        while let Some(pixels) = state
            .scroll_to_pointer(scrollbar_of(&state, &editor, bounds), now)
        {
            editor.scroll_by(pixels);
            shape_wrapped(&mut editor, bounds);
            now += FRAME;

            frames += 1;
            assert!(frames < 100, "the drag never arrived");
        }

        let dragged = scrollbar_of(&state, &editor, bounds)
            .thumb
            .expect("taller than its view");
        assert!(
            (dragged.height - thumb.height).abs() < 0.5,
            "the thumb's length moved from {} to {} at step {step}",
            thumb.height,
            dragged.height
        );
    }

    // And the drag really did cross the document, rather than sitting
    // where the wrapping never changed under it.
    assert!(
        editor.buffer().scroll().line > SHORT_DOCUMENT_LINES / 2,
        "the drag should have reached the far end of the document"
    );
}
