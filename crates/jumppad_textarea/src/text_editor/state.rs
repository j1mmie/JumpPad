use std::cell::{Cell, RefCell};

use iced_core::time::Instant;
use iced_core::{Font, input_method, mouse, text, widget::operation};

use crate::{drag_scroll, line_numbers, scrollbar};

/// The state of a [`super::TextEditor`].
#[derive(Debug)]
pub struct State<Highlighter: text::Highlighter> {
    pub(super) focus: Option<Focus>,
    pub(super) preedit: Option<input_method::Preedit>,
    pub(super) last_click: Option<mouse::Click>,
    /// The selection drag in progress, if any. Set by a single click, and the
    /// only thing that makes a pointer move count as a drag - a double or
    /// triple click selects outright and leaves this empty.
    pub(super) selection_drag: Option<drag_scroll::Drag>,
    pub(super) partial_scroll: f32,
    pub(super) scrollbar: scrollbar::State,
    /// The line-number column, kept until the document or the face it is
    /// drawn in moves. Measuring it means shaping a row of digits, and every
    /// frame asks for the width at least three times - to wrap the text, to
    /// hit-test a pointer and to draw.
    pub(super) line_numbers: Cell<Option<MeasuredColumn>>,
    pub(super) last_theme: RefCell<Option<String>>,
    pub(super) highlighter: RefCell<Highlighter>,
    pub(super) highlighter_settings: Highlighter::Settings,
    pub(super) highlighter_format_address: usize,
}

/// A [`line_numbers::Column`] and what it was measured against, so a later
/// frame can tell whether the measurement still stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MeasuredColumn {
    pub(super) digits: u32,
    pub(super) font: Font,
    pub(super) text_size: f32,
    pub(super) column: line_numbers::Column,
}

impl MeasuredColumn {
    pub(super) fn still_stands_for(
        &self,
        digits: u32,
        font: Font,
        text_size: f32,
    ) -> bool {
        self.digits == digits
            && self.font == font
            && self.text_size == text_size
    }
}

#[derive(Debug, Clone)]
pub(super) struct Focus {
    pub(super) updated_at: Instant,
    pub(super) now: Instant,
    pub(super) is_window_focused: bool,
}

impl Focus {
    pub(super) const CURSOR_BLINK_INTERVAL_MILLIS: u128 = 500;

    pub(super) fn now() -> Self {
        let now = Instant::now();

        Self {
            updated_at: now,
            now,
            is_window_focused: true,
        }
    }

    pub(super) fn is_cursor_visible(&self) -> bool {
        self.is_window_focused
            && ((self.now - self.updated_at).as_millis()
                / Self::CURSOR_BLINK_INTERVAL_MILLIS)
                .is_multiple_of(2)
    }
}

impl<Highlighter: text::Highlighter> State<Highlighter> {
    /// Returns whether the [`super::TextEditor`] is currently focused or not.
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
