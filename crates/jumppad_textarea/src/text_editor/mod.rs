//! JumpPad's text area: a fork of `iced_widget` 0.14.2's `text_editor`
//! (MIT), maintained here rather than tracked against upstream.
//!
//! Forked because `Content` kept its `iced_graphics::text::Editor` behind a
//! private field, so nothing outside the widget could read the scroll offset
//! (see AGENTS.md). The same field is what blocks background highlighting for
//! find matches and multiple cursors, so those land here too.

mod binding;
mod builder;
mod content;
mod geometry;
mod scroll_restore;
mod state;
mod theme;

use std::ops::DerefMut;
use std::sync::Arc;

use iced::advanced::graphics;
use iced_core::alignment;
use iced_core::clipboard::{self, Clipboard};
use iced_core::layout::{self, Layout};
use iced_core::mouse;
use iced_core::renderer;
use iced_core::text::editor::Editor as _;
use iced_core::text::{self, Paragraph as _, Text};
use iced_core::widget::{self, Widget};
use iced_core::window;
use iced_core::{
    Background, Border, Element, Event, Length, Point, Rectangle, Shell, Size,
};

use crate::{drag_scroll, line_numbers};

use binding::{Ime, Update};
use geometry::{TextInset, scrollbar_layout, text_clip};
use scroll_restore::shape_and_reveal;
use state::{Focus, MeasuredColumn};

pub use binding::{Binding, KeyPress};
pub use builder::{TextEditor, text_editor};
pub use content::Content;
pub use scroll_restore::CapturedView;
pub use state::State;
pub use theme::{
    Catalog, DEFAULT_LINE_NUMBER_ALPHA, Status, Style, StyleFn, default,
};
pub use text::editor::{
    Action, Cursor, Direction, Edit, Line, LineEnding, Motion, Position,
    Selection,
};

/// One line's height in pixels, which is what turns a scroll measured in
/// lines into one measured in pixels.
impl<Highlighter, Message, Theme, Renderer>
    TextEditor<'_, Highlighter, Message, Theme, Renderer>
where
    Highlighter: text::Highlighter,
    Theme: Catalog,
    Renderer:
        text::Renderer<Font = iced_core::Font, Editor = graphics::text::Editor>,
{
    fn absolute_line_height(&self, renderer: &Renderer) -> f32 {
        self.line_height
            .to_absolute(
                self.text_size.unwrap_or_else(|| renderer.default_size()),
            )
            .0
    }

    /// The line-number column beside the document, or `None` when the
    /// document isn't numbered - or when the text area is too narrow to spare
    /// the room the numbers would take.
    ///
    /// Answered from the state's own cache. The width is asked for at least
    /// three times a frame - to wrap the text, to place a pointer in it, and
    /// to draw it - and answering it means shaping a row of digits, which is
    /// only worth doing again when the document has grown a digit or the face
    /// it is drawn in has changed.
    fn line_number_column(
        &self,
        state: &State<Highlighter>,
        renderer: &Renderer,
        line_count: usize,
        text_area_width: f32,
    ) -> Option<line_numbers::Column> {
        if !self.line_numbers {
            return None;
        }

        let font = self.font.unwrap_or_else(|| renderer.default_font());
        let text_size =
            self.text_size.unwrap_or_else(|| renderer.default_size());
        let digits = line_numbers::Column::digits_for(line_count);

        let column = match state.line_numbers.get() {
            Some(measured)
                if measured.still_stands_for(digits, font, text_size.0) =>
            {
                measured.column
            }
            _ => {
                let column = line_numbers::Column::new(
                    digits,
                    self.measure_digits(digits, font, text_size),
                );
                state.line_numbers.set(Some(MeasuredColumn {
                    digits,
                    font,
                    text_size: text_size.0,
                    column,
                }));

                column
            }
        };

        column.leaves_room_in(text_area_width).then_some(column)
    }

    /// How wide `digits` digits draw in the document's own face. Zeros
    /// because they are the widest digit in most faces and the same width as
    /// every other one in the rest, which is what lets one measurement stand
    /// for every number of that length.
    fn measure_digits(
        &self,
        digits: u32,
        font: iced_core::Font,
        text_size: iced_core::Pixels,
    ) -> f32 {
        Renderer::Paragraph::with_text(Text {
            content: "0".repeat(digits as usize).as_str(),
            bounds: Size::INFINITE,
            size: text_size,
            line_height: self.line_height,
            font,
            align_x: text::Alignment::Default,
            align_y: alignment::Vertical::Top,
            shaping: text::Shaping::Basic,
            wrapping: text::Wrapping::None,
        })
        .min_bounds()
        .width
    }

    /// How much of `bounds` sits above and to the left of the document's own
    /// text - see [`TextInset`], which is where every coordinate in the text
    /// is measured from.
    fn text_inset(
        &self,
        state: &State<Highlighter>,
        renderer: &Renderer,
        bounds: Rectangle,
    ) -> TextInset {
        TextInset::new(
            self.padding,
            self.line_number_column(
                state,
                renderer,
                self.content.line_count(),
                bounds.shrink(self.padding).width,
            ),
        )
    }

    /// The numbers themselves, right-aligned down the left of everything
    /// inside the widget's padding - whose top edge the text shares, so a
    /// row's own offset places its number without any further arithmetic.
    ///
    /// One number to a line rather than one to a row: a line long enough to
    /// wrap is numbered on the row it begins on and left blank down the rows
    /// it wrapped onto, so the numbers count the document rather than the
    /// screen. The line the caret is on is drawn at full strength, which is
    /// how the eye finds its place again after looking away.
    ///
    /// Nothing at all when the document isn't numbered.
    ///
    /// Drawn inside a layer of their own, and that is what clips them. The
    /// rows the top and bottom edges cut through are drawn whole - that is
    /// what scrolling by pixels means - so the overhang has to be masked off
    /// or it lands on whatever the widget sits under (see [`text_clip`], and
    /// AGENTS.md for what that costs on Windows). `fill_text` cannot do it:
    /// the software renderer measures a `Text` it is handed against the
    /// *layer* it is in rather than against the clip it was given, so a clip
    /// passed here alone would be read as advice and ignored. A layer a
    /// sliver shorter than the numbers may paint is what it does act on.
    fn draw_line_numbers(
        &self,
        renderer: &mut Renderer,
        editor: &graphics::text::Editor,
        style: &Style,
        inset: TextInset,
        bounds: Rectangle,
    ) {
        let Some(column) = inset.line_numbers() else {
            return;
        };

        let font = self.font.unwrap_or_else(|| renderer.default_font());
        let text_size =
            self.text_size.unwrap_or_else(|| renderer.default_size());
        let caret_line = editor.cursor().position.line;
        let inside = inset.inside(bounds);

        renderer.with_layer(text_clip(inside), |renderer| {
            for row in line_numbers::rows(editor.buffer())
                .filter(|row| row.starts_line)
            {
                let color = if row.line == caret_line {
                    style.value
                } else {
                    style.line_number
                };

                let number = (row.line + 1).to_string();
                let left =
                    column.number_left_edge(number.len() as u32, inside.x);

                renderer.fill_text(
                    Text {
                        content: number,
                        bounds: Size::new(column.width(), row.height),
                        size: text_size,
                        line_height: self.line_height,
                        font,
                        align_x: text::Alignment::Default,
                        align_y: alignment::Vertical::Top,
                        shaping: text::Shaping::Basic,
                        wrapping: text::Wrapping::None,
                    },
                    Point::new(left, inside.y + row.top),
                    color,
                    inside,
                );
            }
        });
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
    ///
    /// Measured against the text rather than the whole widget, so the rows it
    /// counts are the rows the text actually wraps to once the line-number
    /// column has taken its room. The track keeps the right edge either way -
    /// the column takes from the left.
    fn scrollbar(
        &self,
        state: &State<Highlighter>,
        layout: Layout<'_>,
        renderer: &Renderer,
        width: f32,
    ) -> Option<crate::scrollbar::Layout> {
        let text_bounds = self
            .text_inset(state, renderer, layout.bounds())
            .text_bounds(layout.bounds());

        scrollbar_layout(
            &state.scrollbar,
            &self.content.0.borrow().editor,
            text_bounds,
            width,
        )
    }

    fn is_over_thumb(
        &self,
        state: &State<Highlighter>,
        layout: Layout<'_>,
        renderer: &Renderer,
        cursor: mouse::Cursor,
        width: f32,
    ) -> bool {
        let Some(position) = cursor.position() else {
            return false;
        };
        self.scrollbar(state, layout, renderer, width)
            .and_then(|scrollbar| scrollbar.thumb)
            .is_some_and(|thumb| thumb.contains(position))
    }

    /// Whether the pointer is on the line numbers rather than on the text -
    /// the strip between the widget's own left padding and the first
    /// character of every line.
    fn is_over_line_numbers(
        &self,
        state: &State<Highlighter>,
        layout: Layout<'_>,
        renderer: &Renderer,
        cursor: mouse::Cursor,
    ) -> bool {
        let bounds = layout.bounds();
        let inset = self.text_inset(state, renderer, bounds);

        cursor.is_over(Rectangle {
            width: inset.line_numbers_width(),
            ..inset.inside(bounds)
        })
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
        now: iced_core::time::Instant,
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

    /// Carries a scrollbar-thumb drag forward by one frame: the view moves
    /// toward wherever the pointer is holding the thumb, measured against
    /// where the view actually ended up rather than where the frame before
    /// asked it to go.
    ///
    /// Asks for another frame whenever it moved the view, since the rows a
    /// scroll crosses are estimated and one frame's can land short of the row
    /// it was aimed at - and a pointer sitting still sends nothing of its own
    /// to finish on. `now` is the frame's own instant, which is what keeps a
    /// frame that re-runs in it from asking for a second scroll.
    fn advance_scrollbar_drag(
        &self,
        state: &mut State<Highlighter>,
        scrollbar: Option<crate::scrollbar::Layout>,
        now: iced_core::time::Instant,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let Some(pixels) = scrollbar.and_then(|scrollbar| {
            state.scrollbar.scroll_to_pointer(scrollbar, now)
        }) else {
            return;
        };

        self.publish_scroll(pixels, state, renderer, shell);
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
            scrollbar: crate::scrollbar::State::default(),
            line_numbers: std::cell::Cell::default(),
            last_theme: std::cell::RefCell::default(),
            highlighter: std::cell::RefCell::new(Highlighter::new(
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

        // Before the document is borrowed: measuring the column reads the
        // line count off it, and shaping below holds it mutably.
        let inside = limits.shrink(self.padding).max();
        let column = self.line_number_column(
            state,
            renderer,
            self.content.line_count(),
            inside.width,
        );

        let mut internal = self.content.0.borrow_mut();

        // The column takes its room out of the width the text wraps at, so a
        // numbered document wraps where it draws.
        let text_bounds = Size {
            width: column
                .map_or(inside.width, |column| column.text_width(inside.width)),
            ..inside
        };
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
        // The frame's own instant, not a fresh reading of the clock: iced
        // re-runs this event at the same instant after a widget publishes
        // anything, and that is how the work below tells a second pass over a
        // frame from the frame after it.
        let redrawing_at = match event {
            Event::Window(window::Event::RedrawRequested(now)) => Some(*now),
            _ => None,
        };
        let is_redraw = redrawing_at.is_some();

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
                    focus.updated_at = iced_core::time::Instant::now();

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
                            + iced_core::time::Duration::from_millis(
                                millis_until_redraw as u64,
                            ),
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
        let now = iced_core::time::Instant::now();
        let inset = self.text_inset(state, renderer, layout.bounds());
        let text_bounds = inset.text_bounds(layout.bounds());
        let width = state.scrollbar.width(now);
        let scrollbar = self.scrollbar(state, layout, renderer, width);

        if let Some(redrawing_at) = redrawing_at {
            if let Some(scrollbar) = scrollbar {
                // Catches the wheel and cursor-driven auto-scroll alike -
                // the latter happens inside cosmic-text and is invisible
                // from anywhere else.
                state.scrollbar.note_scroll(scrollbar.position(), now);
            }
            if let Some(at) = state.scrollbar.next_redraw(now) {
                shell.request_redraw_at(at);
            }

            self.advance_scrollbar_drag(
                state,
                scrollbar,
                redrawing_at,
                renderer,
                shell,
            );
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
                    if let Some(position) = cursor.position() {
                        state.scrollbar.drag_to(position, now);
                    }

                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }

                let hovered = cursor.position().is_some_and(|position| {
                    crate::scrollbar::Layout::is_in_reveal_strip(
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
            inset,
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
                Update::SelectLineAt(position) => {
                    state.focus = Some(Focus::now());
                    // A press on a number is not a click in the text, so it
                    // ends whatever multi-click run was in progress rather
                    // than counting toward the next one.
                    state.last_click = None;
                    state.selection_drag =
                        Some(drag_scroll::Drag::new(position, now));

                    // The click puts the caret on the line pressed, and
                    // `SelectLine` takes the whole of it - whole logical
                    // lines, so a wrapped line comes as one. The drag that
                    // follows goes through the ordinary path: cosmic-text
                    // holds a line selection at line granularity, so it
                    // keeps taking whole lines however far it is dragged.
                    shell.publish(on_edit(Action::Click(position)));
                    shell.publish(on_edit(Action::SelectLine));
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
                        state.preedit = is_open
                            .then(iced_core::input_method::Preedit::new);

                        shell.request_redraw();
                    }
                    Ime::Preedit { content, selection } => {
                        state.preedit =
                            Some(iced_core::input_method::Preedit {
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
                        focus.updated_at = iced_core::time::Instant::now();
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
                &self.input_method(state, renderer, text_bounds),
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

        let state = tree.state.downcast_ref::<State<Highlighter>>();

        // Before the document is borrowed, the same as in `layout`: measuring
        // the line numbers reads the line count off it.
        let inset = self.text_inset(state, renderer, bounds);

        let mut internal = self.content.0.borrow_mut();

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

        let text_bounds = inset.text_bounds(bounds);

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

        self.draw_line_numbers(
            renderer,
            &internal.editor,
            &style,
            inset,
            bounds,
        );

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
        let now = iced_core::time::Instant::now();
        let opacity = state.scrollbar.opacity(now);
        if opacity > 0.0 {
            let width = state.scrollbar.width(now);
            if let Some(thumb) = scrollbar_layout(
                &state.scrollbar,
                &internal.editor,
                text_bounds,
                width,
            )
            .and_then(|layout| {
                layout.thumb.map(|thumb| (thumb, layout.radius()))
            }) {
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
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let is_disabled = self.on_edit.is_none();
        let state = tree.state.downcast_ref::<State<Highlighter>>();
        let width = state.scrollbar.width(iced_core::time::Instant::now());

        // An I-beam over the thumb would suggest the text underneath is what
        // the click lands on, and it isn't.
        if state.scrollbar.is_dragging()
            || self.is_over_thumb(state, layout, renderer, cursor, width)
        {
            return mouse::Interaction::Idle;
        }

        if cursor.is_over(layout.bounds()) {
            if is_disabled {
                mouse::Interaction::NotAllowed
            } else if self.is_over_line_numbers(state, layout, renderer, cursor)
            {
                // Same reason as the thumb: a press on a number takes the
                // whole line, it doesn't put a caret between two characters.
                mouse::Interaction::default()
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

// Test-only: these live in sibling modules and are otherwise reached only
// through fully-qualified paths, but `text_editor_tests.rs`'s `use super::*`
// needs them bound here by name, the same way it needs everything else in
// this file (same pattern as the other phases' splits).
#[cfg(test)]
use crate::safe_area::SafeArea;
#[cfg(test)]
use crate::scrollbar;
#[cfg(test)]
use geometry::{
    LINES_PER_WHEEL_NOTCH, PIXELS_PER_LINE, SCROLL_MULTIPLIER_RANGE,
    clamp_scroll_multiplier, text_position, wheel_lines,
};
#[cfg(test)]
use iced_core::text::Wrapping;
#[cfg(test)]
use iced_core::text::highlighter;
#[cfg(test)]
use iced_core::text::LineHeight;
#[cfg(test)]
use iced_core::time::{Duration, Instant};
#[cfg(test)]
use iced_core::text::Highlighter;
#[cfg(test)]
use iced_core::{Padding, Pixels};
#[cfg(test)]
use scroll_restore::{
    PendingView, cursor_row, restore_offset, reveal_offset, scrolled_to,
};

#[cfg(test)]
#[path = "text_editor_tests.rs"]
mod tests;
