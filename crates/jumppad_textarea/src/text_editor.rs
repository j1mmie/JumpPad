//! JumpPad's text area: a fork of `iced_widget` 0.14.2's `text_editor`
//! (MIT), maintained here rather than tracked against upstream.
//!
//! Forked because `Content` kept its `iced_graphics::text::Editor` behind a
//! private field, so nothing outside the widget could read the scroll offset
//! (see AGENTS.md). The same field is what blocks background highlighting for
//! find matches and multiple cursors, so those land here too.
use iced_core::alignment;
use iced_core::clipboard::{self, Clipboard};
use iced_core::input_method;
use iced_core::keyboard;
use iced_core::keyboard::key;
use iced_core::layout::{self, Layout};
use iced_core::mouse;
use iced_core::renderer;
use iced_core::text::editor::Editor as _;
use iced_core::text::highlighter::{self, Highlighter};
use iced_core::text::{self, LineHeight, Text, Wrapping};
use iced_core::theme;
use iced_core::time::{Duration, Instant};
use iced_core::widget::operation;
use iced_core::widget::{self, Widget};
use iced_core::window;
use iced_core::{
    Background, Border, Color, Element, Event, InputMethod, Length, Padding,
    Pixels, Point, Rectangle, Shell, Size, SmolStr, Theme, Vector,
};

use crate::drag_scroll;
use crate::indent;
use crate::safe_area::SafeArea;
use crate::scrollbar;
use iced::advanced::graphics;

use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::ops;
use std::ops::DerefMut;
use std::sync::Arc;

pub use text::editor::{
    Action, Cursor, Edit, Line, LineEnding, Motion, Position, Selection,
};

/// The scrollbar's geometry for wherever the document currently sits, or
/// `None` if it has no line height to measure against yet.
fn scrollbar_layout(
    editor: &graphics::text::Editor,
    text_bounds: Rectangle,
    width: f32,
) -> Option<scrollbar::Layout> {
    let metrics = scrollbar::metrics(editor.buffer(), text_bounds.size())?;
    Some(scrollbar::Layout::new(text_bounds, metrics, width))
}

/// Lines scrolled per notch of a discrete wheel, at `sensitivity == 1.0`.
/// Upstream `iced_widget` uses 4.0; JumpPad ships half that, and puts the
/// rest of the range behind `[scroll] sensitivity` in `config.toml`.
const LINES_PER_WHEEL_NOTCH: f32 = 2.0;

/// Pixels of a precise (trackpad, or a free-spinning wheel) delta that make
/// one line, at `sensitivity == 1.0`. Upstream divides by 4.0; doubling the
/// divisor halves the speed, to match `LINES_PER_WHEEL_NOTCH`.
const PIXELS_PER_LINE: f32 = 8.0;

/// The range `[scroll] sensitivity` and `[scroll] drag_speed` are held to.
/// The ceiling is only there to keep a typo'd config from making scrolling
/// useless; the floor is above zero so it never stops entirely.
const SCROLL_MULTIPLIER_RANGE: ops::RangeInclusive<f32> = 0.05..=20.0;

/// One wheel or trackpad event, in lines to scroll down by - fractional, so
/// a sensitivity below `1.0` doesn't round every event to a standstill. The
/// caller banks the fraction (`State::partial_scroll`) until it makes a whole
/// line, which is the only unit `Action::Scroll` can carry.
fn wheel_lines(delta: mouse::ScrollDelta, sensitivity: f32) -> f32 {
    sensitivity
        * match delta {
            // A discrete wheel: `y` is notches, and the floor keeps a
            // fraction of a notch from reading as no scroll at all.
            mouse::ScrollDelta::Lines { y, .. } => {
                if y.abs() > 0.0 {
                    y.signum() * -(y.abs() * LINES_PER_WHEEL_NOTCH).max(1.0)
                } else {
                    0.0
                }
            }
            // A precise device: `y` is already pixels of intended travel.
            mouse::ScrollDelta::Pixels { y, .. } => -y / PIXELS_PER_LINE,
        }
}

/// Where the pointer sits relative to the text, with the widget's own
/// position and padding taken off - the coordinates every editor action is
/// written in.
///
/// Deliberately unclamped, unlike a hit test for a click: a selection drag
/// keeps following a pointer that has left the widget, and the window with
/// it, and the editor resolves a position outside the text against its
/// nearest row. `None` only when the pointer's position is unknown, which is
/// what the platform reports when it isn't on this window at all.
fn text_position(
    cursor: mouse::Cursor,
    bounds: Rectangle,
    padding: Padding,
) -> Option<Point> {
    Some(
        cursor.position()?
            - Vector::new(bounds.x + padding.left, bounds.y + padding.top),
    )
}

/// How much shorter than the text area the clip handed to the renderer is -
/// see [`text_clip`]. A tenth of a pixel, well under the half a pixel that
/// would change which pixels the clip covers.
const TEXT_CLIP_SHORTFALL: f32 = 0.1;

/// The rectangle the document's text is clipped to: the text area, a sliver
/// shorter.
///
/// That sliver is the whole point. The software renderer builds a clip mask
/// for text only when the text's own bounds reach past the clip it was given,
/// on the assumption that text inside its bounds cannot paint outside them.
/// A pixel-scrolled editor breaks the assumption: the rows the top and bottom
/// edges cut through are drawn whole, overhanging the editor by whatever the
/// edge cut off, and an unclipped overhang lands on the tab bar - where
/// nothing repaints it (see AGENTS.md). A clip the editor demonstrably
/// doesn't fit inside is what gets the mask built, and with it the overhang
/// clipped.
///
/// It costs no pixel of real text: the mask is not anti-aliased, so a pixel
/// belongs to it by its centre, and the sliver is far too thin to move one.
fn text_clip(text_bounds: Rectangle) -> Rectangle {
    Rectangle {
        height: (text_bounds.height - TEXT_CLIP_SHORTFALL).max(0.0),
        ..text_bounds
    }
}

/// The guard on [`TextEditor::scroll_sensitivity`] and
/// [`TextEditor::drag_speed`], split out so the range is enforced in one
/// place. A non-finite value falls back to the default rather than clamping -
/// `NaN` has no meaningful end of the range.
fn clamp_scroll_multiplier(multiplier: f32) -> f32 {
    if multiplier.is_finite() {
        multiplier.clamp(
            *SCROLL_MULTIPLIER_RANGE.start(),
            *SCROLL_MULTIPLIER_RANGE.end(),
        )
    } else {
        1.0
    }
}

/// A change to the document the widget has not laid out yet, and where the
/// view sat when it happened. Both variants want the same thing of the next
/// shape - a cursor inside the safe area, clear of the edge it came in from -
/// but they start from opposite ends: an in-place edit keeps the view it had,
/// while a rebuilt `Content` starts at the top of the document with the view
/// thrown away.
#[derive(Debug, Clone, Copy)]
enum PendingView {
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

/// Where a [`Content`] had its view and its cursor, read off before a rebuild
/// replaces it and handed to the [`Content`] that takes its place.
///
/// The view is the buffer's own `Scroll` - a logical line and the pixels into
/// it - rather than the single number [`Content::scrolled_to`] reports. That
/// number adds the two together, which is fine for the scrollbar but cannot be
/// scrolled *back* to: see `restore_pixels`.
///
/// The cursor's row travels with them because a reveal needs to know which way
/// the cursor is *going*, not just where it ended up: a line moved up has no
/// business scrolling the view down to chase it past the low boundary.
#[derive(Debug, Clone, Copy)]
pub struct CapturedView {
    scroll_line: usize,
    scroll_vertical: f32,
    cursor_row: f32,
}

/// Where the view sits, in lines from the top of the document, or `None` if it
/// has no line height to measure against yet. Fractional: `scroll.vertical` is
/// a pixel offset into the wrapped rows of the line at `scroll.line`.
fn scrolled_to(editor: &graphics::text::Editor) -> Option<f32> {
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
fn cursor_row(editor: &graphics::text::Editor) -> Option<f32> {
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
fn viewport_rows(editor: &graphics::text::Editor, text_bounds: Size) -> f32 {
    text_bounds.height / editor.buffer().metrics().line_height
}

/// Shapes the editor, then hands the cursor of a change the widget has not
/// seen yet the view it wants.
///
/// Shaping is where cosmic-text reveals a cursor a change left off screen, so
/// there is nothing to correct until it has run - and on a rebuilt `Content`
/// there are no line metrics to scroll by until then either. Each extra
/// scroll needs a shape of its own to settle before the frame draws.
fn shape_and_reveal(
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
fn restore_pixels(
    editor: &graphics::text::Editor,
    before: CapturedView,
) -> f32 {
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
fn reveal_offset(
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
fn restore_offset(cursor_row: f32, moved_by: f32, rows: f32) -> Option<i32> {
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

/// Creates a new [`TextEditor`]. Upstream this lives in `iced_widget::helpers`.
pub fn text_editor<'a, Message, Theme, Renderer>(
    content: &'a Content<Renderer>,
) -> TextEditor<'a, highlighter::PlainText, Message, Theme, Renderer>
where
    Message: Clone,
    Theme: Catalog + 'a,
    Renderer: text::Renderer,
{
    TextEditor::new(content)
}

/// A multi-line text input.
pub struct TextEditor<
    'a,
    Highlighter,
    Message,
    Theme = iced::Theme,
    Renderer = iced::Renderer,
> where
    Highlighter: text::Highlighter,
    Theme: Catalog,
    Renderer: text::Renderer,
{
    id: Option<widget::Id>,
    content: &'a Content<Renderer>,
    placeholder: Option<text::Fragment<'a>>,
    font: Option<Renderer::Font>,
    text_size: Option<Pixels>,
    line_height: LineHeight,
    width: Length,
    height: Length,
    min_height: f32,
    max_height: f32,
    padding: Padding,
    wrapping: Wrapping,
    /// Multiplier on wheel and trackpad scroll distance - see
    /// [`TextEditor::scroll_sensitivity`].
    scroll_sensitivity: f32,
    /// Multiplier on how fast a selection drag held past an edge walks the
    /// view - see [`TextEditor::drag_speed`].
    drag_speed: f32,
    /// Columns between tab stops, for drawing - see
    /// [`TextEditor::tab_width`].
    tab_width: u16,
    class: Theme::Class<'a>,
    #[allow(clippy::type_complexity)]
    key_binding: Option<Box<dyn Fn(KeyPress) -> Option<Binding<Message>> + 'a>>,
    on_edit: Option<Box<dyn Fn(Action) -> Message + 'a>>,
    /// Pixel scrolls, which `Action` can't carry - see
    /// [`TextEditor::on_scroll`]. Without one set, the wheel falls back to
    /// whole lines through `on_edit`.
    #[allow(clippy::type_complexity)]
    on_scroll: Option<Box<dyn Fn(f32) -> Message + 'a>>,
    highlighter_settings: Highlighter::Settings,
    highlighter_format: fn(
        &Highlighter::Highlight,
        &Theme,
    ) -> highlighter::Format<Renderer::Font>,
    last_status: Option<Status>,
}

impl<'a, Message, Theme, Renderer>
    TextEditor<'a, highlighter::PlainText, Message, Theme, Renderer>
where
    Theme: Catalog,
    Renderer: text::Renderer,
{
    /// Creates new [`TextEditor`] with the given [`Content`].
    pub fn new(content: &'a Content<Renderer>) -> Self {
        Self {
            id: None,
            content,
            placeholder: None,
            font: None,
            text_size: None,
            line_height: LineHeight::default(),
            width: Length::Fill,
            height: Length::Shrink,
            min_height: 0.0,
            max_height: f32::INFINITY,
            padding: Padding::new(5.0),
            wrapping: Wrapping::default(),
            scroll_sensitivity: 1.0,
            drag_speed: 1.0,
            tab_width: indent::DEFAULT_WIDTH,
            class: <Theme as Catalog>::default(),
            key_binding: None,
            on_edit: None,
            on_scroll: None,
            highlighter_settings: (),
            highlighter_format: |_highlight, _theme| {
                highlighter::Format::default()
            },
            last_status: None,
        }
    }

    /// Sets the [`Id`](widget::Id) of the [`TextEditor`].
    pub fn id(mut self, id: impl Into<widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }
}

impl<'a, Highlighter, Message, Theme, Renderer>
    TextEditor<'a, Highlighter, Message, Theme, Renderer>
where
    Highlighter: text::Highlighter,
    Theme: Catalog,
    Renderer: text::Renderer,
{
    /// Sets the placeholder of the [`TextEditor`].
    pub fn placeholder(
        mut self,
        placeholder: impl text::IntoFragment<'a>,
    ) -> Self {
        self.placeholder = Some(placeholder.into_fragment());
        self
    }

    /// Sets the width of the [`TextEditor`].
    pub fn width(mut self, width: impl Into<Pixels>) -> Self {
        self.width = Length::from(width.into());
        self
    }

    /// Sets the height of the [`TextEditor`].
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Sets the minimum height of the [`TextEditor`].
    pub fn min_height(mut self, min_height: impl Into<Pixels>) -> Self {
        self.min_height = min_height.into().0;
        self
    }

    /// Sets the maximum height of the [`TextEditor`].
    pub fn max_height(mut self, max_height: impl Into<Pixels>) -> Self {
        self.max_height = max_height.into().0;
        self
    }

    /// Sets the message that should be produced when some action is performed in
    /// the [`TextEditor`].
    ///
    /// If this method is not called, the [`TextEditor`] will be disabled.
    pub fn on_action(
        mut self,
        on_edit: impl Fn(Action) -> Message + 'a,
    ) -> Self {
        self.on_edit = Some(Box::new(on_edit));
        self
    }

    /// Sets the message produced when the wheel or the scrollbar thumb
    /// scrolls the view, carrying a distance in **pixels**.
    ///
    /// Separate from [`on_action`](Self::on_action) because `Action::Scroll`
    /// counts in whole lines, which is exactly the quantization this exists
    /// to avoid. Handle it with `Content::scroll_by`.
    ///
    /// Optional: with no handler set, scrolling falls back to whole lines
    /// through `on_action`, which is how upstream behaves.
    pub fn on_scroll(
        mut self,
        on_scroll: impl Fn(f32) -> Message + 'a,
    ) -> Self {
        self.on_scroll = Some(Box::new(on_scroll));
        self
    }

    /// Sets the [`Font`] of the [`TextEditor`].
    ///
    /// [`Font`]: text::Renderer::Font
    pub fn font(mut self, font: impl Into<Renderer::Font>) -> Self {
        self.font = Some(font.into());
        self
    }

    /// Sets the text size of the [`TextEditor`].
    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.text_size = Some(size.into());
        self
    }

    /// Sets the [`text::LineHeight`] of the [`TextEditor`].
    pub fn line_height(
        mut self,
        line_height: impl Into<text::LineHeight>,
    ) -> Self {
        self.line_height = line_height.into();
        self
    }

    /// Sets the [`Padding`] of the [`TextEditor`].
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Sets the [`Wrapping`] strategy of the [`TextEditor`].
    pub fn wrapping(mut self, wrapping: Wrapping) -> Self {
        self.wrapping = wrapping;
        self
    }

    /// Scales how far one unit of wheel or trackpad input scrolls the
    /// document. `1.0` is the shipped speed; larger is faster. Out-of-range
    /// values are clamped rather than rejected - this sits on the path from
    /// a hand-edited `config.toml`, and a bad number should slow the wheel
    /// down, not break it.
    pub fn scroll_sensitivity(mut self, sensitivity: f32) -> Self {
        self.scroll_sensitivity = clamp_scroll_multiplier(sensitivity);
        self
    }

    /// Scales how fast a selection drag held past the top or bottom edge
    /// walks the view. `1.0` is the shipped speed; larger is faster. Clamped
    /// the same way, and for the same reason, as `scroll_sensitivity`.
    pub fn drag_speed(mut self, speed: f32) -> Self {
        self.drag_speed = clamp_scroll_multiplier(speed);
        self
    }

    /// Sets how many columns a tab character covers when it is drawn. The
    /// same width the document is indented at, so tabs already in a file
    /// line up with the ones typed into it.
    ///
    /// Range-checked by [`Indentation`], which is the only thing that builds
    /// one.
    ///
    /// [`Indentation`]: crate::Indentation
    pub fn tab_width(mut self, width: u16) -> Self {
        self.tab_width = width;
        self
    }

    /// Highlights the [`TextEditor`] with the given [`Highlighter`] and
    /// a strategy to turn its highlights into some text format.
    pub fn highlight_with<H: text::Highlighter>(
        self,
        settings: H::Settings,
        to_format: fn(
            &H::Highlight,
            &Theme,
        ) -> highlighter::Format<Renderer::Font>,
    ) -> TextEditor<'a, H, Message, Theme, Renderer> {
        TextEditor {
            id: self.id,
            content: self.content,
            placeholder: self.placeholder,
            font: self.font,
            text_size: self.text_size,
            line_height: self.line_height,
            width: self.width,
            height: self.height,
            min_height: self.min_height,
            max_height: self.max_height,
            padding: self.padding,
            wrapping: self.wrapping,
            scroll_sensitivity: self.scroll_sensitivity,
            drag_speed: self.drag_speed,
            tab_width: self.tab_width,
            class: self.class,
            key_binding: self.key_binding,
            on_edit: self.on_edit,
            on_scroll: self.on_scroll,
            highlighter_settings: settings,
            highlighter_format: to_format,
            last_status: self.last_status,
        }
    }

    /// Sets the closure to produce key bindings on key presses.
    ///
    /// See [`Binding`] for the list of available bindings.
    pub fn key_binding(
        mut self,
        key_binding: impl Fn(KeyPress) -> Option<Binding<Message>> + 'a,
    ) -> Self {
        self.key_binding = Some(Box::new(key_binding));
        self
    }

    /// Sets the style of the [`TextEditor`].
    #[must_use]
    pub fn style(mut self, style: impl Fn(&Theme, Status) -> Style + 'a) -> Self
    where
        Theme::Class<'a>: From<StyleFn<'a, Theme>>,
    {
        self.class = (Box::new(style) as StyleFn<'a, Theme>).into();
        self
    }

    /// Sets the style class of the [`TextEditor`].
    #[must_use]
    pub fn class(mut self, class: impl Into<Theme::Class<'a>>) -> Self {
        self.class = class.into();
        self
    }

    fn input_method<'b>(
        &self,
        state: &'b State<Highlighter>,
        renderer: &Renderer,
        layout: Layout<'_>,
    ) -> InputMethod<&'b str> {
        let Some(Focus {
            is_window_focused: true,
            ..
        }) = &state.focus
        else {
            return InputMethod::Disabled;
        };

        let bounds = layout.bounds();
        let internal = self.content.0.borrow_mut();

        let text_bounds = bounds.shrink(self.padding);
        let translation = text_bounds.position() - Point::ORIGIN;

        let cursor = match internal.editor.selection() {
            Selection::Caret(position) => position,
            Selection::Range(ranges) => {
                ranges.first().cloned().unwrap_or_default().position()
            }
        };

        let line_height = self.line_height.to_absolute(
            self.text_size.unwrap_or_else(|| renderer.default_size()),
        );

        let position = cursor + translation;

        InputMethod::Enabled {
            cursor: Rectangle::new(
                position,
                Size::new(1.0, f32::from(line_height)),
            ),
            purpose: input_method::Purpose::Normal,
            preedit: state.preedit.as_ref().map(input_method::Preedit::as_ref),
        }
    }
}

/// The content of a [`TextEditor`].
pub struct Content<R = iced::Renderer>(RefCell<Internal<R>>)
where
    R: text::Renderer;

struct Internal<R>
where
    R: text::Renderer,
{
    editor: R::Editor,
    /// The change awaiting layout, if there is one. Everything that settles
    /// the view around a cursor happens on the next shape, so this is the only
    /// chance to record where the view sat when the change was made - `layout`
    /// acts on it and takes it back to `None`.
    pending_view: Option<PendingView>,
    /// Whether `layout` has shaped this editor even once. Nothing that reads a
    /// laid-out position may be asked before that - cosmic-text panics on a
    /// line whose layout it has not cached yet - which rules out the cursor's
    /// row, and so [`Content::capture_view`].
    shaped: bool,
}

impl<R> Content<R>
where
    R: text::Renderer,
{
    /// Creates an empty [`Content`].
    pub fn new() -> Self {
        Self::with_text("")
    }

    /// Creates a [`Content`] with the given text.
    pub fn with_text(text: &str) -> Self {
        Self(RefCell::new(Internal {
            editor: R::Editor::with_text(text),
            pending_view: None,
            shaped: false,
        }))
    }

    /// Moves the current cursor to reflect the given one.
    pub fn move_to(&mut self, cursor: Cursor) {
        let internal = self.0.get_mut();

        internal.editor.move_to(cursor);
    }

    /// Returns the current cursor position of the [`Content`].
    pub fn cursor(&self) -> Cursor {
        self.0.borrow().editor.cursor()
    }

    /// Returns the amount of lines of the [`Content`].
    pub fn line_count(&self) -> usize {
        self.0.borrow().editor.line_count()
    }

    /// Returns the text of the line at the given index, if it exists.
    pub fn line(&self, index: usize) -> Option<Line<'_>> {
        let internal = self.0.borrow();
        let line = internal.editor.line(index)?;

        Some(Line {
            text: Cow::Owned(line.text.into_owned()),
            ending: line.ending,
        })
    }

    /// Returns an iterator of the text of the lines in the [`Content`].
    pub fn lines(&self) -> impl Iterator<Item = Line<'_>> {
        (0..)
            .map(|i| self.line(i))
            .take_while(Option::is_some)
            .flatten()
    }

    /// Returns the text of the [`Content`].
    pub fn text(&self) -> String {
        let mut contents = String::new();
        let mut lines = self.lines().peekable();

        while let Some(line) = lines.next() {
            contents.push_str(&line.text);

            if lines.peek().is_some() {
                contents.push_str(if line.ending == LineEnding::None {
                    LineEnding::default().as_str()
                } else {
                    line.ending.as_str()
                });
            }
        }

        contents
    }

    /// Returns the selected text of the [`Content`].
    pub fn selection(&self) -> Option<String> {
        self.0.borrow().editor.copy()
    }

    /// Returns the kind of [`LineEnding`] used for separating lines in the [`Content`].
    pub fn line_ending(&self) -> Option<LineEnding> {
        Some(self.line(0)?.ending)
    }

    /// Returns whether or not the the [`Content`] is empty.
    pub fn is_empty(&self) -> bool {
        self.0.borrow().editor.is_empty()
    }
}

// Pinned to the concrete graphics editor, like the `Widget` impl below: the
// view a change is about to move lives on the cosmic-text buffer, which only
// that editor exposes.
impl<R> Content<R>
where
    R: text::Renderer<Editor = graphics::text::Editor>,
{
    /// Performs an [`Action`] on the [`Content`].
    pub fn perform(&mut self, action: Action) {
        let internal = self.0.get_mut();

        if action.is_edit() {
            internal.pending_view = scrolled_to(&internal.editor)
                .map(|scrolled_to| PendingView::Edited { scrolled_to });
        }

        internal.editor.perform(action);
    }

    /// Scrolls the view by a number of pixels, leaving it wherever that
    /// lands - between two lines as readily as on one.
    ///
    /// The counterpart to `Action::Scroll`, which counts in whole lines and
    /// is what everything that *wants* a line boundary still uses (the
    /// cursor reveal in `shape_and_reveal`, and `restore_view`). This is for
    /// the wheel and the scrollbar thumb, where the user is pointing at a
    /// position rather than counting lines.
    ///
    /// Never records a `pending_view`: scrolling is not an edit, so there is
    /// no cursor to chase back onto the screen afterwards.
    pub fn scroll_by(&mut self, pixels: f32) {
        self.0.get_mut().editor.scroll_by(pixels);
    }

    /// Where the view sits, in lines from the top of the document, or `None`
    /// before the first layout has given it any line metrics to measure with.
    pub fn scrolled_to(&self) -> Option<f32> {
        scrolled_to(&self.0.borrow().editor)
    }

    /// The logical line the caret is on, to hand back to
    /// [`reveal_caret_from`] once a line command has finished moving it.
    ///
    /// [`reveal_caret_from`]: Self::reveal_caret_from
    pub fn caret_line(&self) -> usize {
        self.0.borrow().editor.cursor().position.line
    }

    /// Asks the next shape to keep the caret inside the safe area, clear of
    /// the edge it is heading for, given where it started.
    ///
    /// For the line commands, which splice in place and so keep the buffer's
    /// own scroll - there is no view to restore, only the safe area to honour.
    /// Call it *after* the caret has been put where the command leaves it:
    /// this is the record the next `layout` reads, and it measures the caret
    /// as it finds it.
    pub fn reveal_caret_from(&mut self, caret_line: usize) {
        self.0.get_mut().pending_view =
            Some(PendingView::Spliced { caret_line });
    }

    /// Where the view and the cursor sit, to hand to the [`Content`] that
    /// replaces this one. `None` until the first layout, which is what gives
    /// them a shaped line to measure against - and there is nothing to carry
    /// across before then anyway, since the view has never been anywhere.
    pub fn capture_view(&self) -> Option<CapturedView> {
        let internal = self.0.borrow();
        if !internal.shaped {
            return None;
        }
        let scroll = internal.editor.buffer().scroll();

        Some(CapturedView {
            scroll_line: scroll.line,
            scroll_vertical: scroll.vertical,
            cursor_row: cursor_row(&internal.editor)?,
        })
    }

    /// Puts the view back where the [`Content`] this one replaces had it, then
    /// reveals the cursor from there if the change left it short of context.
    ///
    /// For undo, redo and the line commands, which rebuild the whole
    /// [`Content`] rather than edit the document in place: a rebuilt one
    /// starts at the top of the document with no line metrics to scroll by,
    /// so neither can happen before the next layout has shaped it once.
    pub fn restore_view(&mut self, before: Option<CapturedView>) {
        self.0.get_mut().pending_view = before.map(PendingView::Rebuilt);
    }
}

impl<Renderer> Clone for Content<Renderer>
where
    Renderer: text::Renderer,
{
    fn clone(&self) -> Self {
        Self::with_text(&self.text())
    }
}

impl<Renderer> Default for Content<Renderer>
where
    Renderer: text::Renderer,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Renderer> fmt::Debug for Content<Renderer>
where
    Renderer: text::Renderer,
    Renderer::Editor: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let internal = self.0.borrow();

        f.debug_struct("Content")
            .field("editor", &internal.editor)
            .finish()
    }
}

/// The state of a [`TextEditor`].
#[derive(Debug)]
pub struct State<Highlighter: text::Highlighter> {
    focus: Option<Focus>,
    preedit: Option<input_method::Preedit>,
    last_click: Option<mouse::Click>,
    /// The selection drag in progress, if any. Set by a single click, and the
    /// only thing that makes a pointer move count as a drag - a double or
    /// triple click selects outright and leaves this empty.
    selection_drag: Option<drag_scroll::Drag>,
    partial_scroll: f32,
    scrollbar: scrollbar::State,
    last_theme: RefCell<Option<String>>,
    highlighter: RefCell<Highlighter>,
    highlighter_settings: Highlighter::Settings,
    highlighter_format_address: usize,
}

#[derive(Debug, Clone)]
struct Focus {
    updated_at: Instant,
    now: Instant,
    is_window_focused: bool,
}

impl Focus {
    const CURSOR_BLINK_INTERVAL_MILLIS: u128 = 500;

    fn now() -> Self {
        let now = Instant::now();

        Self {
            updated_at: now,
            now,
            is_window_focused: true,
        }
    }

    fn is_cursor_visible(&self) -> bool {
        self.is_window_focused
            && ((self.now - self.updated_at).as_millis()
                / Self::CURSOR_BLINK_INTERVAL_MILLIS)
                .is_multiple_of(2)
    }
}

impl<Highlighter: text::Highlighter> State<Highlighter> {
    /// Returns whether the [`TextEditor`] is currently focused or not.
    pub fn is_focused(&self) -> bool {
        self.focus.is_some()
    }
}

impl<Highlighter: text::Highlighter> operation::Focusable
    for State<Highlighter>
{
    fn is_focused(&self) -> bool {
        self.focus.is_some()
    }

    fn focus(&mut self) {
        self.focus = Some(Focus::now());
    }

    fn unfocus(&mut self) {
        self.focus = None;
    }
}

impl<Highlighter, Message, Theme, Renderer>
    TextEditor<'_, Highlighter, Message, Theme, Renderer>
where
    Highlighter: text::Highlighter,
    Theme: Catalog,
    Renderer:
        text::Renderer<Font = iced_core::Font, Editor = graphics::text::Editor>,
{
    /// One line's height in pixels, which is what turns a scroll measured in
    /// lines into one measured in pixels.
    fn absolute_line_height(&self, renderer: &Renderer) -> f32 {
        self.line_height
            .to_absolute(
                self.text_size.unwrap_or_else(|| renderer.default_size()),
            )
            .0
    }

    /// The text a selection drag is walking over: the rows it is scrolling,
    /// where the top edge is cutting through them, and the speed the config
    /// asks for.
    fn walk(
        &self,
        layout: Layout<'_>,
        renderer: &Renderer,
    ) -> drag_scroll::Walk {
        let line_height = self.absolute_line_height(renderer);
        let scrolled =
            self.content.0.borrow().editor.buffer().scroll().vertical;

        drag_scroll::Walk {
            text_height: layout.bounds().shrink(self.padding).height,
            line_height,
            clipped_top: scrolled.rem_euclid(line_height),
            speed: self.drag_speed,
        }
    }

    /// The scrollbar's geometry against the widget's laid-out bounds.
    fn scrollbar(
        &self,
        layout: Layout<'_>,
        width: f32,
    ) -> Option<scrollbar::Layout> {
        let text_bounds = layout.bounds().shrink(self.padding);
        scrollbar_layout(&self.content.0.borrow().editor, text_bounds, width)
    }

    fn is_over_thumb(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        width: f32,
    ) -> bool {
        let Some(position) = cursor.position() else {
            return false;
        };
        self.scrollbar(layout, width)
            .and_then(|scrollbar| scrollbar.thumb)
            .is_some_and(|thumb| thumb.contains(position))
    }

    /// Carries a selection drag held past the top or bottom edge of the text
    /// forward by one frame: the view walks that way and the drag lands again
    /// at the same pointer, so the selection takes in the lines that scrolled
    /// under it. Nothing happens while the pointer is on the text - there the
    /// pointer's own movement is all the selection needs.
    ///
    /// Asks for another frame for as long as the pointer is out there, even
    /// on one too short to have moved the view: a pointer sitting still
    /// outside the window sends nothing of its own, so the frames are the
    /// only thing left to walk on.
    fn advance_selection_drag(
        &self,
        state: &mut State<Highlighter>,
        walk: drag_scroll::Walk,
        now: Instant,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let Some(on_edit) = self.on_edit.as_ref() else {
            return;
        };
        let Some(mut drag) = state.selection_drag else {
            return;
        };
        let step = drag.scroll_step(walk, now);
        state.selection_drag = Some(drag);

        match step {
            drag_scroll::Step::Still => return,
            drag_scroll::Step::Waiting => {}
            drag_scroll::Step::Scroll(pixels) => {
                self.publish_scroll(pixels, state, renderer, shell);
                shell.publish(on_edit(Action::Drag(
                    drag.selecting_at(walk.after_scrolling(pixels)),
                )));
            }
        }

        shell.request_redraw();
    }

    /// Moves the view by `pixels`, however the app asked to hear about it:
    /// through the pixel handler when one is set, and otherwise in whole
    /// lines through `on_edit`, with the remainder banked until it makes one.
    fn publish_scroll(
        &self,
        pixels: f32,
        state: &mut State<Highlighter>,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        if let Some(on_scroll) = self.on_scroll.as_ref() {
            if pixels != 0.0 {
                shell.publish(on_scroll(pixels));
            }
            return;
        }

        let Some(on_edit) = self.on_edit.as_ref() else {
            return;
        };

        let lines =
            pixels / self.absolute_line_height(renderer) + state.partial_scroll;
        state.partial_scroll = lines.fract();

        let lines = lines as i32;
        if lines != 0 {
            shell.publish(on_edit(Action::Scroll { lines }));
        }
    }
}

// Pinned to the concrete graphics editor, rather than generic over
// `text::Renderer` the way upstream is: the scrollbar reads its position off
// the cosmic-text buffer, which only that editor exposes.
impl<Highlighter, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for TextEditor<'_, Highlighter, Message, Theme, Renderer>
where
    Highlighter: text::Highlighter,
    Theme: Catalog,
    Renderer:
        text::Renderer<Font = iced_core::Font, Editor = graphics::text::Editor>,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State<Highlighter>>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State {
            focus: None,
            preedit: None,
            last_click: None,
            selection_drag: None,
            partial_scroll: 0.0,
            scrollbar: scrollbar::State::default(),
            last_theme: RefCell::default(),
            highlighter: RefCell::new(Highlighter::new(
                &self.highlighter_settings,
            )),
            highlighter_settings: self.highlighter_settings.clone(),
            highlighter_format_address: self.highlighter_format as usize,
        })
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> iced_core::layout::Node {
        let mut internal = self.content.0.borrow_mut();
        let state = tree.state.downcast_mut::<State<Highlighter>>();

        if state.highlighter_format_address != self.highlighter_format as usize
        {
            state.highlighter.borrow_mut().change_line(0);

            state.highlighter_format_address = self.highlighter_format as usize;
        }

        if state.highlighter_settings != self.highlighter_settings {
            state
                .highlighter
                .borrow_mut()
                .update(&self.highlighter_settings);

            state.highlighter_settings = self.highlighter_settings.clone();
        }

        let limits = limits
            .width(self.width)
            .height(self.height)
            .min_height(self.min_height)
            .max_height(self.max_height);

        let text_bounds = limits.shrink(self.padding).max();
        let font = self.font.unwrap_or_else(|| renderer.default_font());
        let text_size =
            self.text_size.unwrap_or_else(|| renderer.default_size());
        let line_height = self.line_height;
        let wrapping = self.wrapping;
        let tab_width = self.tab_width;
        let shape = |editor: &mut Renderer::Editor| {
            // Before the update, so a width that just changed is in effect
            // for the shaping it triggers rather than the one after it.
            editor.set_tab_width(tab_width);
            editor.update(
                text_bounds,
                font,
                text_size,
                line_height,
                wrapping,
                state.highlighter.borrow_mut().deref_mut(),
            );
        };

        let pending_view = internal.pending_view.take();
        shape_and_reveal(
            &mut internal.editor,
            pending_view,
            text_bounds,
            shape,
        );
        internal.shaped = true;

        match self.height {
            Length::Fill | Length::FillPortion(_) | Length::Fixed(_) => {
                layout::Node::new(limits.max())
            }
            Length::Shrink => {
                let min_bounds = internal.editor.min_bounds();

                layout::Node::new(
                    limits
                        .height(min_bounds.height)
                        .max()
                        .expand(Size::new(0.0, self.padding.y())),
                )
            }
        }
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let Some(on_edit) = self.on_edit.as_ref() else {
            return;
        };

        let state = tree.state.downcast_mut::<State<Highlighter>>();
        let is_redraw = matches!(
            event,
            Event::Window(window::Event::RedrawRequested(_now)),
        );

        match event {
            Event::Window(window::Event::Unfocused) => {
                if let Some(focus) = &mut state.focus {
                    focus.is_window_focused = false;
                }
                // The pointer grab a drag rides on goes back to the system
                // along with the focus, so no release is coming and there is
                // nothing left to follow.
                state.selection_drag = None;
            }
            Event::Window(window::Event::Focused) => {
                if let Some(focus) = &mut state.focus {
                    focus.is_window_focused = true;
                    focus.updated_at = Instant::now();

                    shell.request_redraw();
                }
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                if let Some(focus) =
                    state.focus.as_mut().filter(|focus| focus.is_window_focused)
                {
                    focus.now = *now;

                    let millis_until_redraw =
                        Focus::CURSOR_BLINK_INTERVAL_MILLIS
                            - (focus.now - focus.updated_at).as_millis()
                                % Focus::CURSOR_BLINK_INTERVAL_MILLIS;

                    shell.request_redraw_at(
                        focus.now
                            + Duration::from_millis(millis_until_redraw as u64),
                    );
                }

                self.advance_selection_drag(
                    state,
                    self.walk(layout, renderer),
                    *now,
                    renderer,
                    shell,
                );
            }
            _ => {}
        }

        // The scrollbar gets first look at the pointer, so a press on the
        // thumb never also lands as a click in the document.
        let now = Instant::now();
        let text_bounds = layout.bounds().shrink(self.padding);
        let width = state.scrollbar.width(now);
        let scrollbar = self.scrollbar(layout, width);

        if is_redraw {
            if let Some(scrollbar) = scrollbar {
                // Catches the wheel and cursor-driven auto-scroll alike -
                // the latter happens inside cosmic-text and is invisible
                // from anywhere else.
                state.scrollbar.note_scroll(scrollbar.position(), now);
            }
            if let Some(at) = state.scrollbar.next_redraw(now) {
                shell.request_redraw_at(at);
            }
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let grabbed = scrollbar.zip(cursor.position()).is_some_and(
                    |(scrollbar, position)| {
                        state.scrollbar.press(position, scrollbar, now)
                    },
                );

                if grabbed {
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.scrollbar.is_dragging() {
                    let lines = scrollbar.zip(cursor.position()).and_then(
                        |(scrollbar, position)| {
                            state.scrollbar.drag_to(position, scrollbar, now)
                        },
                    );

                    if let Some(lines) = lines {
                        if let Some(on_scroll) = self.on_scroll.as_ref() {
                            shell.publish(on_scroll(
                                lines * self.absolute_line_height(renderer),
                            ));
                        } else {
                            shell.publish(on_edit(Action::Scroll {
                                lines: lines as i32,
                            }));
                        }
                    }
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }

                let hovered = cursor.position().is_some_and(|position| {
                    scrollbar::Layout::is_in_reveal_strip(text_bounds, position)
                });

                if state.scrollbar.set_hovered(hovered, now) {
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.scrollbar.is_dragging() =>
            {
                state.scrollbar.release(now);
                shell.capture_event();
                shell.request_redraw();
                return;
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                if state.scrollbar.set_hovered(false, now) {
                    shell.request_redraw();
                }
            }
            _ => {}
        }

        if let Some(update) = Update::from_event(
            event,
            state,
            layout.bounds(),
            self.padding,
            cursor,
            self.scroll_sensitivity,
            self.key_binding.as_deref(),
        ) {
            match update {
                Update::Click(click) => {
                    let action = match click.kind() {
                        mouse::click::Kind::Single => {
                            Action::Click(click.position())
                        }
                        mouse::click::Kind::Double => Action::SelectWord,
                        mouse::click::Kind::Triple => Action::SelectLine,
                    };

                    state.focus = Some(Focus::now());
                    state.last_click = Some(click);
                    state.selection_drag =
                        matches!(click.kind(), mouse::click::Kind::Single)
                            .then(|| {
                                drag_scroll::Drag::new(click.position(), now)
                            });

                    shell.publish(on_edit(action));
                    shell.capture_event();
                }
                Update::Drag(position) => {
                    let walk = self.walk(layout, renderer);
                    let selecting_at = match &mut state.selection_drag {
                        Some(drag) => {
                            drag.move_to(position);
                            drag.selecting_at(walk)
                        }
                        None => position,
                    };

                    shell.publish(on_edit(Action::Drag(selecting_at)));
                }
                Update::Release => {
                    state.selection_drag = None;
                }
                Update::Scroll(lines) => {
                    let bounds = self.content.0.borrow().editor.bounds();

                    if bounds.height >= i32::MAX as f32 {
                        return;
                    }

                    // Pixels, so the view can land between two lines: a
                    // tenth of a line scrolls a tenth of a line, wherever
                    // the app has a pixel handler to hear it.
                    self.publish_scroll(
                        lines * self.absolute_line_height(renderer),
                        state,
                        renderer,
                        shell,
                    );
                    shell.capture_event();
                }
                Update::InputMethod(update) => match update {
                    Ime::Toggle(is_open) => {
                        state.preedit =
                            is_open.then(input_method::Preedit::new);

                        shell.request_redraw();
                    }
                    Ime::Preedit { content, selection } => {
                        state.preedit = Some(input_method::Preedit {
                            content,
                            selection,
                            text_size: self.text_size,
                        });

                        shell.request_redraw();
                    }
                    Ime::Commit(text) => {
                        shell.publish(on_edit(Action::Edit(Edit::Paste(
                            Arc::new(text),
                        ))));
                    }
                },
                Update::Binding(binding) => {
                    fn apply_binding<
                        H: text::Highlighter,
                        R: text::Renderer,
                        Message,
                    >(
                        binding: Binding<Message>,
                        content: &Content<R>,
                        state: &mut State<H>,
                        on_edit: &dyn Fn(Action) -> Message,
                        clipboard: &mut dyn Clipboard,
                        shell: &mut Shell<'_, Message>,
                    ) {
                        let mut publish =
                            |action| shell.publish(on_edit(action));

                        match binding {
                            Binding::Unfocus => {
                                state.focus = None;
                                state.selection_drag = None;
                            }
                            Binding::Copy => {
                                if let Some(selection) = content.selection() {
                                    clipboard.write(
                                        clipboard::Kind::Standard,
                                        selection,
                                    );
                                }
                            }
                            Binding::Cut => {
                                if let Some(selection) = content.selection() {
                                    clipboard.write(
                                        clipboard::Kind::Standard,
                                        selection,
                                    );

                                    publish(Action::Edit(Edit::Delete));
                                }
                            }
                            Binding::Paste => {
                                if let Some(contents) =
                                    clipboard.read(clipboard::Kind::Standard)
                                {
                                    publish(Action::Edit(Edit::Paste(
                                        Arc::new(contents),
                                    )));
                                }
                            }
                            Binding::Move(motion) => {
                                publish(Action::Move(motion));
                            }
                            Binding::Select(motion) => {
                                publish(Action::Select(motion));
                            }
                            Binding::SelectWord => {
                                publish(Action::SelectWord);
                            }
                            Binding::SelectLine => {
                                publish(Action::SelectLine);
                            }
                            Binding::SelectAll => {
                                publish(Action::SelectAll);
                            }
                            Binding::Insert(c) => {
                                publish(Action::Edit(Edit::Insert(c)));
                            }
                            Binding::Enter => {
                                publish(Action::Edit(Edit::Enter));
                            }
                            Binding::Backspace => {
                                publish(Action::Edit(Edit::Backspace));
                            }
                            Binding::Delete => {
                                publish(Action::Edit(Edit::Delete));
                            }
                            Binding::Sequence(sequence) => {
                                for binding in sequence {
                                    apply_binding(
                                        binding, content, state, on_edit,
                                        clipboard, shell,
                                    );
                                }
                            }
                            Binding::Custom(message) => {
                                shell.publish(message);
                            }
                        }
                    }

                    if !matches!(binding, Binding::Unfocus) {
                        shell.capture_event();
                    }

                    apply_binding(
                        binding,
                        self.content,
                        state,
                        on_edit,
                        clipboard,
                        shell,
                    );

                    if let Some(focus) = &mut state.focus {
                        focus.updated_at = Instant::now();
                    }
                }
            }
        }

        let status = {
            let is_disabled = self.on_edit.is_none();
            let is_hovered = cursor.is_over(layout.bounds());

            if is_disabled {
                Status::Disabled
            } else if state.focus.is_some() {
                Status::Focused { is_hovered }
            } else if is_hovered {
                Status::Hovered
            } else {
                Status::Active
            }
        };

        if is_redraw {
            self.last_status = Some(status);

            shell.request_input_method(
                &self.input_method(state, renderer, layout),
            );
        } else if self
            .last_status
            .is_some_and(|last_status| status != last_status)
        {
            shell.request_redraw();
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _defaults: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        let mut internal = self.content.0.borrow_mut();
        let state = tree.state.downcast_ref::<State<Highlighter>>();

        let font = self.font.unwrap_or_else(|| renderer.default_font());

        let theme_name = theme.name();

        if state
            .last_theme
            .borrow()
            .as_ref()
            .is_none_or(|last_theme| last_theme != theme_name)
        {
            state.highlighter.borrow_mut().change_line(0);
            let _ =
                state.last_theme.borrow_mut().replace(theme_name.to_owned());
        }

        internal.editor.highlight(
            font,
            state.highlighter.borrow_mut().deref_mut(),
            |highlight| (self.highlighter_format)(highlight, theme),
        );

        let style = theme
            .style(&self.class, self.last_status.unwrap_or(Status::Active));

        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: style.border,
                ..renderer::Quad::default()
            },
            style.background,
        );

        let text_bounds = bounds.shrink(self.padding);

        if internal.editor.is_empty() {
            if let Some(placeholder) = self.placeholder.clone() {
                renderer.fill_text(
                    Text {
                        content: placeholder.into_owned(),
                        bounds: text_bounds.size(),
                        size: self
                            .text_size
                            .unwrap_or_else(|| renderer.default_size()),
                        line_height: self.line_height,
                        font,
                        align_x: text::Alignment::Default,
                        align_y: alignment::Vertical::Top,
                        shaping: text::Shaping::Advanced,
                        wrapping: self.wrapping,
                    },
                    text_bounds.position(),
                    style.placeholder,
                    text_bounds,
                );
            }
        } else {
            renderer.fill_editor(
                &internal.editor,
                text_bounds.position(),
                style.value,
                text_clip(text_bounds),
            );
        }

        let translation = text_bounds.position() - Point::ORIGIN;

        if let Some(focus) = state.focus.as_ref() {
            match internal.editor.selection() {
                Selection::Caret(position) if focus.is_cursor_visible() => {
                    let cursor =
                        Rectangle::new(
                            position + translation,
                            Size::new(
                                1.0,
                                self.line_height
                                    .to_absolute(self.text_size.unwrap_or_else(
                                        || renderer.default_size(),
                                    ))
                                    .into(),
                            ),
                        );

                    if let Some(clipped_cursor) =
                        text_bounds.intersection(&cursor)
                    {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: clipped_cursor,
                                ..renderer::Quad::default()
                            },
                            style.value,
                        );
                    }
                }
                Selection::Range(ranges) => {
                    for range in ranges.into_iter().filter_map(|range| {
                        text_bounds.intersection(&(range + translation))
                    }) {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: range,
                                ..renderer::Quad::default()
                            },
                            style.selection,
                        );
                    }
                }
                Selection::Caret(_) => {}
            }
        }

        // Last, so the thumb floats over the text instead of under it.
        let now = Instant::now();
        let opacity = state.scrollbar.opacity(now);
        if opacity > 0.0 {
            let width = state.scrollbar.width(now);
            if let Some(thumb) =
                scrollbar_layout(&internal.editor, text_bounds, width).and_then(
                    |layout| layout.thumb.map(|thumb| (thumb, layout.radius())),
                )
            {
                let (bounds, radius) = thumb;
                renderer.fill_quad(
                    renderer::Quad {
                        bounds,
                        border: Border {
                            radius: radius.into(),
                            ..Border::default()
                        },
                        ..renderer::Quad::default()
                    },
                    Background::Color(
                        style.scrollbar_thumb.scale_alpha(opacity),
                    ),
                );
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let is_disabled = self.on_edit.is_none();
        let state = tree.state.downcast_ref::<State<Highlighter>>();
        let width = state.scrollbar.width(Instant::now());

        // An I-beam over the thumb would suggest the text underneath is what
        // the click lands on, and it isn't.
        if state.scrollbar.is_dragging()
            || self.is_over_thumb(layout, cursor, width)
        {
            return mouse::Interaction::Idle;
        }

        if cursor.is_over(layout.bounds()) {
            if is_disabled {
                mouse::Interaction::NotAllowed
            } else {
                mouse::Interaction::Text
            }
        } else {
            mouse::Interaction::default()
        }
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        _renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let state = tree.state.downcast_mut::<State<Highlighter>>();

        operation.focusable(self.id.as_ref(), layout.bounds(), state);
    }
}

impl<'a, Highlighter, Message, Theme, Renderer>
    From<TextEditor<'a, Highlighter, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Highlighter: text::Highlighter,
    Message: 'a,
    Theme: Catalog + 'a,
    Renderer:
        text::Renderer<Font = iced_core::Font, Editor = graphics::text::Editor>,
{
    fn from(
        text_editor: TextEditor<'a, Highlighter, Message, Theme, Renderer>,
    ) -> Self {
        Self::new(text_editor)
    }
}

/// A binding to an action in the [`TextEditor`].
#[derive(Debug, Clone, PartialEq)]
pub enum Binding<Message> {
    /// Unfocus the [`TextEditor`].
    Unfocus,
    /// Copy the selection of the [`TextEditor`].
    Copy,
    /// Cut the selection of the [`TextEditor`].
    Cut,
    /// Paste the clipboard contents in the [`TextEditor`].
    Paste,
    /// Apply a [`Motion`].
    Move(Motion),
    /// Select text with a given [`Motion`].
    Select(Motion),
    /// Select the word at the current cursor.
    SelectWord,
    /// Select the line at the current cursor.
    SelectLine,
    /// Select the entire buffer.
    SelectAll,
    /// Insert the given character.
    Insert(char),
    /// Break the current line.
    Enter,
    /// Delete the previous character.
    Backspace,
    /// Delete the next character.
    Delete,
    /// A sequence of bindings to execute.
    Sequence(Vec<Self>),
    /// Produce the given message.
    Custom(Message),
}

/// A key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    /// The original key pressed without modifiers applied to it.
    ///
    /// You should use this key for combinations (e.g. Ctrl+C).
    pub key: keyboard::Key,
    /// The key pressed with modifiers applied to it.
    ///
    /// You should use this key for any single key bindings (e.g. motions).
    pub modified_key: keyboard::Key,
    /// The physical key pressed.
    ///
    /// You should use this key for layout-independent bindings.
    pub physical_key: keyboard::key::Physical,
    /// The state of the keyboard modifiers.
    pub modifiers: keyboard::Modifiers,
    /// The text produced by the key press.
    pub text: Option<SmolStr>,
    /// The current [`Status`] of the [`TextEditor`].
    pub status: Status,
}

impl<Message> Binding<Message> {
    /// Returns the default [`Binding`] for the given key press.
    pub fn from_key_press(event: KeyPress) -> Option<Self> {
        let KeyPress {
            key,
            modified_key,
            physical_key,
            modifiers,
            text,
            status,
        } = event;

        if !matches!(status, Status::Focused { .. }) {
            return None;
        }

        let combination = match key.to_latin(physical_key) {
            Some('c') if modifiers.command() => Some(Self::Copy),
            Some('x') if modifiers.command() => Some(Self::Cut),
            Some('v') if modifiers.command() && !modifiers.alt() => {
                Some(Self::Paste)
            }
            Some('a') if modifiers.command() => Some(Self::SelectAll),
            _ => None,
        };

        if let Some(binding) = combination {
            return Some(binding);
        }

        #[cfg(target_os = "macos")]
        let modified_key =
            convert_macos_shortcut(&key, modifiers).unwrap_or(modified_key);

        match modified_key.as_ref() {
            keyboard::Key::Named(key::Named::Enter) => Some(Self::Enter),
            keyboard::Key::Named(key::Named::Backspace) => {
                Some(Self::Backspace)
            }
            keyboard::Key::Named(key::Named::Delete)
                if text.is_none() || text.as_deref() == Some("\u{7f}") =>
            {
                Some(Self::Delete)
            }
            keyboard::Key::Named(key::Named::Escape) => Some(Self::Unfocus),
            _ => {
                if let Some(text) = text {
                    let c = text.chars().find(|c| !c.is_control())?;

                    Some(Self::Insert(c))
                } else if let keyboard::Key::Named(named_key) = key.as_ref() {
                    let motion = motion(named_key)?;

                    let motion = if modifiers.macos_command() {
                        match motion {
                            Motion::Left => Motion::Home,
                            Motion::Right => Motion::End,
                            _ => motion,
                        }
                    } else {
                        motion
                    };

                    let motion = if modifiers.jump() {
                        motion.widen()
                    } else {
                        motion
                    };

                    Some(if modifiers.shift() {
                        Self::Select(motion)
                    } else {
                        Self::Move(motion)
                    })
                } else {
                    None
                }
            }
        }
    }
}

enum Update<Message> {
    Click(mouse::Click),
    Drag(Point),
    Release,
    Scroll(f32),
    InputMethod(Ime),
    Binding(Binding<Message>),
}

enum Ime {
    Toggle(bool),
    Preedit {
        content: String,
        selection: Option<ops::Range<usize>>,
    },
    Commit(String),
}

impl<Message> Update<Message> {
    fn from_event<H: Highlighter>(
        event: &Event,
        state: &State<H>,
        bounds: Rectangle,
        padding: Padding,
        cursor: mouse::Cursor,
        scroll_sensitivity: f32,
        key_binding: Option<&dyn Fn(KeyPress) -> Option<Binding<Message>>>,
    ) -> Option<Self> {
        let binding = |binding| Some(Update::Binding(binding));

        match event {
            Event::Mouse(event) => match event {
                mouse::Event::ButtonPressed(mouse::Button::Left) => {
                    if let Some(cursor_position) = cursor.position_in(bounds) {
                        let cursor_position = cursor_position
                            - Vector::new(padding.left, padding.top);

                        let click = mouse::Click::new(
                            cursor_position,
                            mouse::Button::Left,
                            state.last_click,
                        );

                        Some(Update::Click(click))
                    } else if state.focus.is_some() {
                        binding(Binding::Unfocus)
                    } else {
                        None
                    }
                }
                mouse::Event::ButtonReleased(mouse::Button::Left) => {
                    Some(Update::Release)
                }
                mouse::Event::CursorMoved { .. }
                    if state.selection_drag.is_some() =>
                {
                    Some(Update::Drag(text_position(cursor, bounds, padding)?))
                }
                mouse::Event::WheelScrolled { delta }
                    if cursor.is_over(bounds) =>
                {
                    Some(Update::Scroll(wheel_lines(
                        *delta,
                        scroll_sensitivity,
                    )))
                }
                _ => None,
            },
            Event::InputMethod(event) => match event {
                input_method::Event::Opened | input_method::Event::Closed => {
                    Some(Update::InputMethod(Ime::Toggle(matches!(
                        event,
                        input_method::Event::Opened
                    ))))
                }
                input_method::Event::Preedit(content, selection)
                    if state.focus.is_some() =>
                {
                    Some(Update::InputMethod(Ime::Preedit {
                        content: content.clone(),
                        selection: selection.clone(),
                    }))
                }
                input_method::Event::Commit(content)
                    if state.focus.is_some() =>
                {
                    Some(Update::InputMethod(Ime::Commit(content.clone())))
                }
                _ => None,
            },
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modified_key,
                physical_key,
                modifiers,
                text,
                ..
            }) => {
                let status = if state.focus.is_some() {
                    Status::Focused {
                        is_hovered: cursor.is_over(bounds),
                    }
                } else {
                    Status::Active
                };

                let key_press = KeyPress {
                    key: key.clone(),
                    modified_key: modified_key.clone(),
                    physical_key: *physical_key,
                    modifiers: *modifiers,
                    text: text.clone(),
                    status,
                };

                if let Some(key_binding) = key_binding {
                    key_binding(key_press)
                } else {
                    Binding::from_key_press(key_press)
                }
                .map(Self::Binding)
            }
            _ => None,
        }
    }
}

fn motion(key: key::Named) -> Option<Motion> {
    match key {
        key::Named::ArrowLeft => Some(Motion::Left),
        key::Named::ArrowRight => Some(Motion::Right),
        key::Named::ArrowUp => Some(Motion::Up),
        key::Named::ArrowDown => Some(Motion::Down),
        key::Named::Home => Some(Motion::Home),
        key::Named::End => Some(Motion::End),
        key::Named::PageUp => Some(Motion::PageUp),
        key::Named::PageDown => Some(Motion::PageDown),
        _ => None,
    }
}

/// The possible status of a [`TextEditor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The [`TextEditor`] can be interacted with.
    Active,
    /// The [`TextEditor`] is being hovered.
    Hovered,
    /// The [`TextEditor`] is focused.
    Focused {
        /// Whether the [`TextEditor`] is hovered, while focused.
        is_hovered: bool,
    },
    /// The [`TextEditor`] cannot be interacted with.
    Disabled,
}

/// The appearance of a text input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// The [`Background`] of the text input.
    pub background: Background,
    /// The [`Border`] of the text input.
    pub border: Border,
    /// The [`Color`] of the placeholder of the text input.
    pub placeholder: Color,
    /// The [`Color`] of the value of the text input.
    pub value: Color,
    /// The [`Color`] of the selection of the text input.
    pub selection: Color,
    /// The fill of the auto-hiding scrollbar's thumb, at full opacity.
    pub scrollbar_thumb: Color,
}

/// The theme catalog of a [`TextEditor`].
pub trait Catalog: theme::Base {
    /// The item class of the [`Catalog`].
    type Class<'a>;

    /// The default class produced by the [`Catalog`].
    fn default<'a>() -> Self::Class<'a>;

    /// The [`Style`] of a class with the given status.
    fn style(&self, class: &Self::Class<'_>, status: Status) -> Style;
}

/// A styling function for a [`TextEditor`].
pub type StyleFn<'a, Theme> = Box<dyn Fn(&Theme, Status) -> Style + 'a>;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(default)
    }

    fn style(&self, class: &Self::Class<'_>, status: Status) -> Style {
        class(self, status)
    }
}

/// The default style of a [`TextEditor`].
pub fn default(theme: &Theme, status: Status) -> Style {
    let palette = theme.extended_palette();

    let active = Style {
        background: Background::Color(palette.background.base.color),
        border: Border {
            radius: 2.0.into(),
            width: 1.0,
            color: palette.background.strong.color,
        },
        placeholder: palette.secondary.base.color,
        value: palette.background.base.text,
        selection: palette.primary.weak.color,
        scrollbar_thumb: palette.background.strong.color,
    };

    match status {
        Status::Active => active,
        Status::Hovered => Style {
            border: Border {
                color: palette.background.base.text,
                ..active.border
            },
            ..active
        },
        Status::Focused { .. } => Style {
            border: Border {
                color: palette.primary.strong.color,
                ..active.border
            },
            ..active
        },
        Status::Disabled => Style {
            background: Background::Color(palette.background.weak.color),
            value: active.placeholder,
            placeholder: palette.background.strongest.color,
            ..active
        },
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn convert_macos_shortcut(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<keyboard::Key> {
    if modifiers != keyboard::Modifiers::CTRL {
        return None;
    }

    let key = match key.as_ref() {
        keyboard::Key::Character("b") => key::Named::ArrowLeft,
        keyboard::Key::Character("f") => key::Named::ArrowRight,
        keyboard::Key::Character("a") => key::Named::Home,
        keyboard::Key::Character("e") => key::Named::End,
        keyboard::Key::Character("h") => key::Named::Backspace,
        keyboard::Key::Character("d") => key::Named::Delete,
        _ => return None,
    };

    Some(keyboard::Key::Named(key))
}

#[cfg(test)]
mod tests {
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
}
