//! A selection drag, driven through the widget the way a window drives it:
//! press, pointer moves, frames. What these cover is the pointer leaving the
//! text - the editor has to keep selecting toward a pointer it can no longer
//! draw under, and walk the view while that pointer sits past an edge.

mod headless;

use headless::Renderer;
use iced_core::clipboard;
use iced_core::layout::{self, Layout};
use iced_core::text::LineHeight;
use iced_core::text::highlighter;
use iced_core::time::{Duration, Instant};
use iced_core::widget::{Tree, Widget};
use iced_core::{
    Event, Font, Point, Rectangle, Shell, Size, Theme, Vector, mouse, window,
};

use jumppad_textarea::text_editor::{Action, Content, TextEditor, text_editor};

/// Where the editor sits in the window. Deliberately not the corner - a tab
/// bar's worth of space above it - so a position in the text can't be
/// mistaken for a position in the window.
const ORIGIN: Vector = Vector::new(30.0, 50.0);
const SIZE: Size = Size::new(400.0, 300.0);
const PADDING: f32 = 5.0;
const LINE_HEIGHT: f32 = 20.0;
/// Long enough that the view can walk a long way without running out.
const DOCUMENT_LINES: usize = 400;
/// A frame at 60fps, the pace the editor asks for while it is walking.
const FRAME_MILLIS: u64 = 16;

#[derive(Debug, Clone, PartialEq)]
enum Message {
    Action(Action),
    Scroll(f32),
}

/// One editor and the events a window would put through it, with the
/// messages it publishes applied to the document the way the app applies
/// them - so each event sees the document the last one left behind.
struct Window {
    content: Content<Renderer>,
    tree: Tree,
    node: layout::Node,
    pointer: Point,
    now: Instant,
    /// When the editor asked to be drawn again, as of the last event.
    redraw_request: window::RedrawRequest,
    /// `[scroll] drag_speed`, as the app would have set it.
    drag_speed: f32,
}

impl Window {
    fn new() -> Self {
        Self::at_drag_speed(1.0)
    }

    fn at_drag_speed(drag_speed: f32) -> Self {
        let content = Content::with_text(&document());
        let tree = Tree::new(&editor(&content, drag_speed)
            as &dyn Widget<Message, Theme, Renderer>);

        let mut window = Self {
            content,
            tree,
            node: layout::Node::new(SIZE),
            pointer: Point::ORIGIN,
            now: Instant::now(),
            redraw_request: window::RedrawRequest::Wait,
            drag_speed,
        };
        // The first shape, which is what gives the document the line metrics
        // that hit tests and scrolls are measured against.
        window.lay_out();

        window
    }

    /// Everything the editor published for one event, in order.
    fn feed(&mut self, event: Event) -> Vec<Message> {
        let mut messages = Vec::new();

        {
            let mut shell = Shell::new(&mut messages);
            let mut widget = editor(&self.content, self.drag_speed);

            widget.update(
                &mut self.tree,
                &event,
                Layout::with_offset(ORIGIN, &self.node),
                mouse::Cursor::Available(self.pointer),
                &Renderer,
                &mut clipboard::Null,
                &mut shell,
                &Rectangle::with_size(Size::INFINITE),
            );

            self.redraw_request = shell.redraw_request();
        }

        for message in &messages {
            match message {
                Message::Action(action) => {
                    self.content.perform(action.clone());
                }
                Message::Scroll(pixels) => self.content.scroll_by(*pixels),
            }
        }
        self.lay_out();

        messages
    }

    fn lay_out(&mut self) {
        let node = {
            let mut widget = editor(&self.content, self.drag_speed);

            widget.layout(
                &mut self.tree,
                &Renderer,
                &layout::Limits::new(Size::ZERO, SIZE),
            )
        };

        self.node = node;
    }

    /// Moves the pointer to a position in the *window*, wherever that leaves
    /// it relative to the editor.
    fn move_pointer_to(&mut self, x: f32, y: f32) -> Vec<Message> {
        self.pointer = Point::new(x, y);

        self.feed(Event::Mouse(mouse::Event::CursorMoved {
            position: self.pointer,
        }))
    }

    fn press(&mut self) -> Vec<Message> {
        self.feed(Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Left,
        )))
    }

    fn release(&mut self) -> Vec<Message> {
        self.feed(Event::Mouse(mouse::Event::ButtonReleased(
            mouse::Button::Left,
        )))
    }

    /// Draws a frame, one 60fps tick after the last one.
    fn frame(&mut self) -> Vec<Message> {
        self.now += Duration::from_millis(FRAME_MILLIS);

        self.feed(Event::Window(window::Event::RedrawRequested(self.now)))
    }

    /// Draws the frame again at the same instant, the way the runtime does
    /// when a widget publishes something mid-frame.
    fn repeat_frame(&mut self) -> Vec<Message> {
        self.feed(Event::Window(window::Event::RedrawRequested(self.now)))
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            let _ = self.frame();
        }
    }

    fn unfocus_window(&mut self) -> Vec<Message> {
        self.feed(Event::Window(window::Event::Unfocused))
    }

    /// A press inside the text, which is what starts a selection drag.
    ///
    /// The widget stamps the drag with the real clock as the press lands, so
    /// the frame clock starts from there - after that the frames are the only
    /// thing that moves time, and a step is exactly as long as it looks.
    fn start_drag(&mut self) {
        self.pointer = Point::new(ORIGIN.x + 50.0, ORIGIN.y + 50.0);
        let _ = self.press();
        self.now = Instant::now();
    }

    fn hold_pointer_below_the_text(&mut self) {
        let _ = self.move_pointer_to(ORIGIN.x + 50.0, ORIGIN.y + SIZE.height);
    }

    fn scrolled_to(&self) -> f32 {
        self.content.scrolled_to().expect("a shaped document")
    }

    fn selection(&self) -> String {
        self.content.selection().expect("a selection")
    }
}

fn document() -> String {
    (0..DOCUMENT_LINES)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn editor(
    content: &Content<Renderer>,
    drag_speed: f32,
) -> TextEditor<'_, highlighter::PlainText, Message, Theme, Renderer> {
    text_editor(content)
        .drag_speed(drag_speed)
        .font(Font::MONOSPACE)
        .size(14.0)
        .line_height(LineHeight::Absolute(LINE_HEIGHT.into()))
        .padding(PADDING)
        .height(SIZE.height)
        .on_action(Message::Action)
        .on_scroll(Message::Scroll)
}

fn dragged_to(messages: &[Message]) -> Option<Point> {
    messages.iter().find_map(|message| match message {
        Message::Action(Action::Drag(position)) => Some(*position),
        _ => None,
    })
}

fn scrolled_by(messages: &[Message]) -> Option<f32> {
    messages.iter().find_map(|message| match message {
        Message::Scroll(pixels) => Some(*pixels),
        _ => None,
    })
}

#[test]
fn a_pointer_off_the_side_of_the_window_keeps_selecting() {
    let mut window = Window::new();
    window.start_drag();

    // Out past the left edge of the window, then up - which is the report
    // this all comes from: the selection has to follow the pointer's height
    // while the pointer is nowhere near the text.
    let dragged = window.move_pointer_to(-200.0, ORIGIN.y + 100.0);

    assert_eq!(
        dragged_to(&dragged),
        Some(Point::new(-235.0, 95.0)),
        "the drag should carry the pointer's real position, off-widget \
         coordinates and all"
    );
    // Level with row 4 and far to the left of it, so the selection runs
    // down to where that row starts.
    assert!(
        window.selection().ends_with("line 3\n"),
        "the selection should follow the pointer down the rows: {:?}",
        window.selection()
    );
}

#[test]
fn a_pointer_held_below_the_text_walks_the_view_down() {
    let mut window = Window::new();
    window.start_drag();
    window.hold_pointer_below_the_text();

    let before = window.scrolled_to();
    let advanced = window.frame();

    assert!(
        scrolled_by(&advanced).is_some_and(|pixels| pixels > 0.0),
        "a frame with the pointer past the bottom edge should move the view \
         down: {advanced:?}"
    );
    assert!(
        dragged_to(&advanced).is_some(),
        "and drag again, so the selection takes in what it revealed: \
         {advanced:?}"
    );
    assert!(
        window.scrolled_to() > before,
        "the document should have moved under the drag"
    );
}

#[test]
fn a_pointer_held_above_the_text_walks_the_view_back_up() {
    let mut window = Window::new();
    window.start_drag();
    window.hold_pointer_below_the_text();
    window.frames(20);

    let from_below = window.scrolled_to();
    assert!(from_below > 0.0, "the view should have walked down first");

    let _ = window.move_pointer_to(ORIGIN.x + 50.0, 0.0);
    let _ = window.frame();

    assert!(
        window.scrolled_to() < from_below,
        "the view should turn around with the pointer"
    );
}

#[test]
fn the_view_holds_still_while_the_pointer_is_level_with_the_text() {
    let mut window = Window::new();
    window.start_drag();

    let _ = window.move_pointer_to(-200.0, ORIGIN.y + 100.0);
    let advanced = window.frame();

    assert_eq!(
        scrolled_by(&advanced),
        None,
        "off to the side is not past an edge: {advanced:?}"
    );
}

#[test]
fn walking_the_view_carries_the_selection_with_it() {
    let mut window = Window::new();
    window.start_drag();
    window.hold_pointer_below_the_text();
    window.frames(40);

    let selected_lines = window.selection().lines().count();
    let rows_in_view = ((SIZE.height - 2.0 * PADDING) / LINE_HEIGHT) as usize;

    assert!(
        selected_lines > rows_in_view,
        "the selection should have grown past the screenful it started in: \
         {selected_lines} lines against {rows_in_view} on screen"
    );
}

#[test]
fn the_walk_stops_at_the_end_of_the_document() {
    let mut window = Window::new();
    window.start_drag();

    let _ = window.move_pointer_to(ORIGIN.x + 50.0, 10_000.0);
    window.frames(400);

    let rows_in_view = (SIZE.height - 2.0 * PADDING) / LINE_HEIGHT;
    let last_view = DOCUMENT_LINES as f32 - rows_in_view;

    assert!(
        (window.scrolled_to() - last_view).abs() < 1.0,
        "the view should come to rest at the end of the document rather \
         than run past it: {} against {last_view}",
        window.scrolled_to()
    );
    assert!(
        window
            .selection()
            .ends_with(&format!("line {}", DOCUMENT_LINES - 1)),
        "and the selection should reach the last line"
    );
}

/// The runtime keeps the *last* answer a frame gives it, and a frame that
/// re-runs in the same instant has earned no pixels - so asking for the next
/// frame only when the view moved would leave the walk waiting on the caret
/// blink for its next chance.
#[test]
fn a_frame_too_short_to_move_the_view_still_asks_for_the_next_one() {
    let mut window = Window::new();
    window.start_drag();
    window.hold_pointer_below_the_text();

    let _ = window.frame();
    let repeated = window.repeat_frame();

    assert_eq!(scrolled_by(&repeated), None, "no time has passed");
    assert_eq!(window.redraw_request, window::RedrawRequest::NextFrame);
}

#[test]
fn a_drag_on_the_text_asks_for_nothing_but_the_caret_blink() {
    let mut window = Window::new();
    window.start_drag();

    let _ = window.move_pointer_to(ORIGIN.x + 100.0, ORIGIN.y + 100.0);
    let _ = window.frame();

    assert!(
        matches!(window.redraw_request, window::RedrawRequest::At(_)),
        "a pointer on the text leaves the editor idle between blinks: {:?}",
        window.redraw_request
    );
}

#[test]
fn releasing_the_button_ends_the_drag() {
    let mut window = Window::new();
    window.start_drag();
    window.hold_pointer_below_the_text();

    let _ = window.release();
    let after_release = window.frame();

    assert_eq!(scrolled_by(&after_release), None);
    assert_eq!(
        window.move_pointer_to(ORIGIN.x + 50.0, ORIGIN.y + 200.0),
        Vec::new(),
        "a pointer move after the release is not a drag any more"
    );
}

#[test]
fn losing_the_window_ends_the_drag() {
    // The system takes the pointer grab back along with the focus, so no
    // release is coming - and a drag left running would walk on forever.
    let mut window = Window::new();
    window.start_drag();
    window.hold_pointer_below_the_text();

    let _ = window.unfocus_window();
    let after_unfocus = window.frame();

    assert_eq!(scrolled_by(&after_unfocus), None);
    assert_eq!(
        window.move_pointer_to(ORIGIN.x + 50.0, ORIGIN.y + 200.0),
        Vec::new(),
    );
}

#[test]
fn a_double_click_does_not_start_a_drag() {
    let mut window = Window::new();
    window.start_drag();
    let _ = window.release();

    // The same spot, straight away: a second click there reads as a double
    // click, which selects a word outright rather than dragging.
    let selected = window.press();

    assert!(
        matches!(selected.as_slice(), [Message::Action(Action::SelectWord)]),
        "{selected:?}"
    );
    assert_eq!(
        window.move_pointer_to(ORIGIN.x + 50.0, ORIGIN.y + 200.0),
        Vec::new(),
    );
}

/// The platform behaviour the whole feature leans on: the editor resolves a
/// position outside the text against its nearest row, which is what makes an
/// off-window pointer a usable selection target.
#[test]
fn a_position_far_above_the_text_selects_up_to_the_top_row() {
    let mut window = Window::new();
    window.start_drag();

    // Up and out of the window's top left corner, so the row it reaches is
    // taken in whole rather than cut at whatever column the pointer is over.
    let _ = window.move_pointer_to(-200.0, -500.0);

    assert!(
        window.selection().starts_with("line 0"),
        "the selection should reach the top visible line: {:?}",
        window.selection()
    );
}

/// The trap the walk fell into: dragging at the pointer's own height puts
/// the caret on the row the bottom edge is cutting through, and cosmic-text
/// scrolls to reveal a caret it has clipped - so the view moved a whole line
/// per frame however few pixels the walk had asked for, however far out the
/// pointer was.
#[test]
fn the_view_moves_by_what_the_walk_asked_for() {
    for past_edge in [5.0, 60.0, 240.0] {
        let mut window = Window::new();
        window.start_drag();
        let _ = window.move_pointer_to(
            ORIGIN.x + 50.0,
            ORIGIN.y + SIZE.height + past_edge,
        );

        for _ in 0..12 {
            let before = window.scrolled_to();
            let asked = scrolled_by(&window.frame()).expect("a scroll");
            let moved = (window.scrolled_to() - before) * LINE_HEIGHT;

            assert!(
                (moved - asked).abs() < 0.01,
                "{past_edge}px past the edge: asked for {asked}px, \
                 the view moved {moved}px"
            );
        }
    }
}

#[test]
fn the_further_out_the_pointer_the_faster_the_view_moves() {
    let mut walked = Vec::new();

    for past_edge in [5.0, 60.0, 240.0] {
        let mut window = Window::new();
        window.start_drag();
        let _ = window.move_pointer_to(
            ORIGIN.x + 50.0,
            ORIGIN.y + SIZE.height + past_edge,
        );
        window.frames(10);

        walked.push(window.scrolled_to());
    }

    assert!(
        walked.windows(2).all(|pair| pair[1] > pair[0] * 1.5),
        "each step out should walk clearly further in the same ten frames: \
         {walked:?}"
    );
}

#[test]
fn a_halved_drag_speed_walks_half_as_far() {
    let mut walked = Vec::new();

    for speed in [1.0, 0.5] {
        let mut window = Window::at_drag_speed(speed);
        window.start_drag();
        window.hold_pointer_below_the_text();
        // One frame to take up the slack between the press and the frame
        // clock, so what follows is ten frames of exactly one tick each.
        let _ = window.frame();
        let from = window.scrolled_to();
        window.frames(10);

        walked.push(window.scrolled_to() - from);
    }

    let [full, half] = walked[..] else {
        unreachable!()
    };
    assert!(
        (half - full / 2.0).abs() < full / 100.0,
        "half the speed should walk half as far: {half} against {full}"
    );
}
