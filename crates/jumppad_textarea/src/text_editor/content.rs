use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;

use iced_core::text::editor::{Action, Cursor, Editor as _, Line, LineEnding};
use iced_core::text;

use super::scroll_restore::{
    CapturedView, PendingView, cursor_row, scrolled_to,
};

/// The content of a [`super::TextEditor`].
pub struct Content<R = iced::Renderer>(pub(super) RefCell<Internal<R>>)
where
    R: text::Renderer;

pub(super) struct Internal<R>
where
    R: text::Renderer,
{
    pub(super) editor: R::Editor,
    /// The change awaiting layout, if there is one. Everything that settles
    /// the view around a cursor happens on the next shape, so this is the only
    /// chance to record where the view sat when the change was made - `layout`
    /// acts on it and takes it back to `None`.
    pub(super) pending_view: Option<PendingView>,
    /// Whether `layout` has shaped this editor even once. Nothing that reads a
    /// laid-out position may be asked before that - cosmic-text panics on a
    /// line whose layout it has not cached yet - which rules out the cursor's
    /// row, and so [`Content::capture_view`].
    pub(super) shaped: bool,
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
    R: text::Renderer<Editor = iced::advanced::graphics::text::Editor>,
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
