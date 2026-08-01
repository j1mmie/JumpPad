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

/// Visible lines left between the cursor and the edge it came in from when an
/// edit pulls an off-screen cursor back into view - so the cursor lands on the
/// sixth visible line rather than hard against the edge, which is where
/// cosmic-text leaves it.
const REVEAL_MARGIN_LINES: i32 = 5;

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

/// Shapes the editor and then hands an edit's cursor the view it wants.
///
/// Shaping is where cosmic-text reveals a cursor an edit left off screen, and
/// it stops the moment the cursor is barely visible; `scrolled_before` (where
/// the view sat going into the edit, `None` if no edit is pending) is what
/// turns that into `REVEAL_MARGIN_LINES` of context past it. The second shape
/// settles the extra scroll before the frame draws.
fn shape_and_reveal(
    editor: &mut graphics::text::Editor,
    scrolled_before: Option<f32>,
    text_bounds: Size,
    shape: impl Fn(&mut graphics::text::Editor),
) {
    shape(editor);

    let Some(scrolled_before) = scrolled_before else {
        return;
    };
    let Some(lines) = reveal_scroll(editor, scrolled_before, text_bounds)
    else {
        return;
    };

    editor.perform(Action::Scroll { lines });
    shape(editor);
}

/// The scroll an edit still owes the view, in lines, measured against where
/// the view sat before it.
fn reveal_scroll(
    editor: &graphics::text::Editor,
    scrolled_before: f32,
    text_bounds: Size,
) -> Option<i32> {
    let line_height = editor.buffer().metrics().line_height;
    let scrolled_after = scrolled_to(editor)?;

    // An `Indent` keeps its selection, so the caret is not always what the
    // reveal chased; the region at the edge the view moved toward stands in
    // for it.
    let cursor_y = match editor.selection() {
        Selection::Caret(position) => position.y,
        Selection::Range(regions) => {
            if scrolled_after < scrolled_before {
                regions.first()?.y
            } else {
                regions.last()?.y
            }
        }
    };

    reveal_offset(
        scrolled_before,
        scrolled_after,
        cursor_y / line_height,
        text_bounds.height / line_height,
    )
}

/// How far to scroll after an edit, in lines, given where the view sat before
/// it, where cosmic-text left it after, and which row the cursor ended up on.
///
/// cosmic-text reveals an off-screen cursor by scrolling the bare minimum,
/// leaving it on the first or last visible row; this backs the view off by
/// `REVEAL_MARGIN_LINES` so it lands inside the view instead. `None` leaves
/// the view alone - either it never moved (the cursor was still on screen
/// after the edit) or it moved without chasing the cursor, which is what a
/// document shrinking under a view anchored at its end does.
fn reveal_offset(
    scrolled_before: f32,
    scrolled_after: f32,
    cursor_row: f32,
    viewport_rows: f32,
) -> Option<i32> {
    // Never further than the opposite edge: a view shorter than the margin
    // would scroll the cursor straight back out of sight.
    let margin = REVEAL_MARGIN_LINES.min(viewport_rows as i32 - 1);
    if margin <= 0 {
        return None;
    }

    if scrolled_after < scrolled_before && cursor_row < 1.0 {
        Some(-margin)
    } else if scrolled_after > scrolled_before
        && cursor_row > viewport_rows - 2.0
    {
        Some(margin)
    } else {
        None
    }
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
    class: Theme::Class<'a>,
    #[allow(clippy::type_complexity)]
    key_binding: Option<Box<dyn Fn(KeyPress) -> Option<Binding<Message>> + 'a>>,
    on_edit: Option<Box<dyn Fn(Action) -> Message + 'a>>,
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
            class: <Theme as Catalog>::default(),
            key_binding: None,
            on_edit: None,
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
            class: self.class,
            key_binding: self.key_binding,
            on_edit: self.on_edit,
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
    /// Where the view sat just before the edit awaiting layout, if there is
    /// one. cosmic-text reveals the cursor lazily, on the next shape, so this
    /// is the only chance to record what the edit is about to scroll away
    /// from - `layout` compares against it and takes it back to `None`.
    scrolled_before_edit: Option<f32>,
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
            scrolled_before_edit: None,
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
// scroll an edit is about to move lives on the cosmic-text buffer, which only
// that editor exposes.
impl<R> Content<R>
where
    R: text::Renderer<Editor = graphics::text::Editor>,
{
    /// Performs an [`Action`] on the [`Content`].
    pub fn perform(&mut self, action: Action) {
        let internal = self.0.get_mut();

        if action.is_edit() {
            internal.scrolled_before_edit = scrolled_to(&internal.editor);
        }

        internal.editor.perform(action);
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
    drag_click: Option<mouse::click::Kind>,
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
    Renderer: text::Renderer<Font = iced_core::Font, Editor = graphics::text::Editor>,
{
    /// The scrollbar's geometry against the widget's laid-out bounds.
    fn scrollbar(&self, layout: Layout<'_>, width: f32) -> Option<scrollbar::Layout> {
        let text_bounds = layout.bounds().shrink(self.padding);
        scrollbar_layout(&self.content.0.borrow().editor, text_bounds, width)
    }

    fn is_over_thumb(&self, layout: Layout<'_>, cursor: mouse::Cursor, width: f32) -> bool {
        let Some(position) = cursor.position() else {
            return false;
        };
        self.scrollbar(layout, width)
            .and_then(|scrollbar| scrollbar.thumb)
            .is_some_and(|thumb| thumb.contains(position))
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
    Renderer: text::Renderer<Font = iced_core::Font, Editor = graphics::text::Editor>,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State<Highlighter>>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State {
            focus: None,
            preedit: None,
            last_click: None,
            drag_click: None,
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
        let shape = |editor: &mut Renderer::Editor| {
            editor.update(
                text_bounds,
                font,
                text_size,
                line_height,
                wrapping,
                state.highlighter.borrow_mut().deref_mut(),
            );
        };

        let scrolled_before = internal.scrolled_before_edit.take();
        shape_and_reveal(
            &mut internal.editor,
            scrolled_before,
            text_bounds,
            shape,
        );

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
                    let lines =
                        scrollbar.zip(cursor.position()).and_then(
                            |(scrollbar, position)| {
                                state
                                    .scrollbar
                                    .drag_to(position, scrollbar, now)
                            },
                        );

                    if let Some(lines) = lines {
                        shell.publish(on_edit(Action::Scroll { lines }));
                    }
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }

                let hovered = cursor.position().is_some_and(|position| {
                    scrollbar::Layout::is_in_reveal_strip(
                        text_bounds,
                        position,
                    )
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
                    state.drag_click = Some(click.kind());

                    shell.publish(on_edit(action));
                    shell.capture_event();
                }
                Update::Drag(position) => {
                    shell.publish(on_edit(Action::Drag(position)));
                }
                Update::Release => {
                    state.drag_click = None;
                }
                Update::Scroll(lines) => {
                    let bounds = self.content.0.borrow().editor.bounds();

                    if bounds.height >= i32::MAX as f32 {
                        return;
                    }

                    let lines = lines + state.partial_scroll;
                    state.partial_scroll = lines.fract();

                    shell.publish(on_edit(Action::Scroll {
                        lines: lines as i32,
                    }));
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
                                state.drag_click = None;
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
                text_bounds,
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
        if state.scrollbar.is_dragging() || self.is_over_thumb(layout, cursor, width) {
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
    Renderer: text::Renderer<Font = iced_core::Font, Editor = graphics::text::Editor>,
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
                mouse::Event::CursorMoved { .. } => match state.drag_click {
                    Some(mouse::click::Kind::Single) => {
                        let cursor_position = cursor.position_in(bounds)?
                            - Vector::new(padding.left, padding.top);

                        Some(Update::Drag(cursor_position))
                    }
                    _ => None,
                },
                mouse::Event::WheelScrolled { delta }
                    if cursor.is_over(bounds) =>
                {
                    Some(Update::Scroll(match delta {
                        mouse::ScrollDelta::Lines { y, .. } => {
                            if y.abs() > 0.0 {
                                y.signum() * -(y.abs() * 4.0).max(1.0)
                            } else {
                                0.0
                            }
                        }
                        mouse::ScrollDelta::Pixels { y, .. } => -y / 4.0,
                    }))
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
        let text = (0..DOCUMENT_LINES)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut editor = graphics::text::Editor::with_text(&text);
        let bounds = Size::new(400.0, VIEW_ROWS as f32 * LINE_HEIGHT);
        shape(&mut editor, bounds);

        (editor, bounds)
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
    fn perform(
        editor: &mut graphics::text::Editor,
        bounds: Size,
        edit: Edit,
    ) {
        let scrolled_before = scrolled_to(editor);

        editor.perform(Action::Edit(edit));
        shape_and_reveal(editor, scrolled_before, bounds, |editor| {
            shape(editor, bounds);
        });
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
    /// Below zero or past `VIEW_ROWS` means it is off screen.
    fn cursor_row(editor: &graphics::text::Editor) -> f32 {
        match editor.selection() {
            Selection::Caret(position) => position.y / LINE_HEIGHT,
            Selection::Range(_) => panic!("no selection is under test"),
        }
    }

    /// The row the reveal aims for, counting from whichever edge the cursor
    /// came in from.
    const REVEALED_ROW: f32 = REVEAL_MARGIN_LINES as f32;

    #[test]
    fn an_edit_with_the_cursor_on_screen_does_not_scroll() {
        let (mut editor, bounds) = document();
        scroll_away(&mut editor, bounds, 100, 40);
        // Back over the cursor, leaving it a few lines shy of either edge.
        editor.perform(Action::Scroll { lines: -25 });
        shape(&mut editor, bounds);

        let scrolled_before = scrolled_to(&editor);
        let row = cursor_row(&editor);
        assert!((0.0..VIEW_ROWS as f32).contains(&row), "cursor off screen");

        perform(&mut editor, bounds, Edit::Insert('x'));

        assert_eq!(scrolled_to(&editor), scrolled_before);
        assert_eq!(cursor_row(&editor), row);
    }

    #[test]
    fn an_edit_above_the_view_reveals_the_cursor_six_lines_from_the_top() {
        let (mut editor, bounds) = document();
        scroll_away(&mut editor, bounds, 100, 60);

        perform(&mut editor, bounds, Edit::Insert('x'));

        assert_eq!(cursor_row(&editor), REVEALED_ROW);
    }

    #[test]
    fn an_edit_below_the_view_reveals_the_cursor_six_lines_from_the_bottom() {
        let (mut editor, bounds) = document();
        scroll_away(&mut editor, bounds, 100, -60);

        perform(&mut editor, bounds, Edit::Insert('x'));

        assert_eq!(cursor_row(&editor), VIEW_ROWS as f32 - 1.0 - REVEALED_ROW);
    }

    #[test]
    fn an_edit_one_line_off_the_edge_still_gets_the_whole_margin() {
        let (mut editor, bounds) = document();
        // The case cosmic-text on its own would settle with a single line of
        // scroll, leaving the cursor hard against the bottom edge.
        scroll_away(&mut editor, bounds, 100, -1);

        perform(&mut editor, bounds, Edit::Insert('x'));

        assert_eq!(cursor_row(&editor), VIEW_ROWS as f32 - 1.0 - REVEALED_ROW);
    }

    #[test]
    fn a_paste_that_carries_the_cursor_off_the_bottom_reveals_it() {
        let (mut editor, bounds) = document();
        // The cursor is on screen going in - it is the pasted lines pushing
        // it past the bottom edge that has to be chased.
        scroll_away(&mut editor, bounds, 10, 0);
        assert_eq!(cursor_row(&editor), 10.0);

        perform(&mut editor, bounds, Edit::Paste(Arc::new("\n".repeat(30))));

        assert_eq!(cursor_row(&editor), VIEW_ROWS as f32 - 1.0 - REVEALED_ROW);
    }

    #[test]
    fn the_reveal_stops_at_the_top_of_the_document() {
        let (mut editor, bounds) = document();
        scroll_away(&mut editor, bounds, 2, 40);

        perform(&mut editor, bounds, Edit::Insert('x'));

        assert_eq!(scrolled_to(&editor), Some(0.0));
        assert_eq!(cursor_row(&editor), 2.0);
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
        assert_eq!(cursor_row(&editor), VIEW_ROWS as f32 - 2.0);
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
    fn a_cursor_revealed_at_an_edge_backs_off_by_the_margin() {
        assert_eq!(offset(-40.0, 0.0), Some(-REVEAL_MARGIN_LINES));
        assert_eq!(offset(40.0, 19.0), Some(REVEAL_MARGIN_LINES));
    }

    #[test]
    fn a_view_that_moved_without_chasing_the_cursor_stays_put() {
        // What a document shrinking under a view anchored at its end does:
        // the scroll clamps, but the cursor is nowhere near an edge.
        assert_eq!(offset(-3.0, 8.0), None);
        assert_eq!(offset(3.0, 8.0), None);
    }

    #[test]
    fn the_margin_never_scrolls_the_cursor_back_out_of_sight() {
        // Four rows of view, so the far edge is only three lines away.
        assert_eq!(reveal_offset(100.0, 140.0, 3.0, 4.0), Some(3));
        assert_eq!(reveal_offset(100.0, 60.0, 0.0, 4.0), Some(-3));
    }

    #[test]
    fn a_view_too_short_to_reveal_into_stays_put() {
        assert_eq!(reveal_offset(100.0, 140.0, 0.0, 1.0), None);
        assert_eq!(reveal_offset(100.0, 140.0, 0.0, 0.0), None);
    }
}
