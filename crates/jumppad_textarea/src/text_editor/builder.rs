use iced_core::text::editor::Editor as _;
use iced_core::widget;
use iced_core::{
    InputMethod, Length, Padding, Pixels, Point, Rectangle, input_method, text,
};

use super::binding::{Binding, KeyPress};
use super::content::Content;
use super::geometry::clamp_scroll_multiplier;
use super::state::{Focus, State};
use super::theme::{Catalog, Status, Style};

/// Creates a new [`TextEditor`]. Upstream this lives in `iced_widget::helpers`.
pub fn text_editor<'a, Message, Theme, Renderer>(
    content: &'a Content<Renderer>,
) -> TextEditor<'a, text::highlighter::PlainText, Message, Theme, Renderer>
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
    pub(super) id: Option<widget::Id>,
    pub(super) content: &'a Content<Renderer>,
    pub(super) placeholder: Option<text::Fragment<'a>>,
    pub(super) font: Option<Renderer::Font>,
    pub(super) text_size: Option<Pixels>,
    pub(super) line_height: text::LineHeight,
    pub(super) width: Length,
    pub(super) height: Length,
    pub(super) min_height: f32,
    pub(super) max_height: f32,
    pub(super) padding: Padding,
    pub(super) wrapping: text::Wrapping,
    /// Multiplier on wheel and trackpad scroll distance - see
    /// [`TextEditor::scroll_sensitivity`].
    pub(super) scroll_sensitivity: f32,
    /// Multiplier on how fast a selection drag held past an edge walks the
    /// view - see [`TextEditor::drag_speed`].
    pub(super) drag_speed: f32,
    /// Columns between tab stops, for drawing - see
    /// [`TextEditor::tab_width`].
    pub(super) tab_width: u16,
    pub(super) class: Theme::Class<'a>,
    #[allow(clippy::type_complexity)]
    pub(super) key_binding:
        Option<Box<dyn Fn(KeyPress) -> Option<Binding<Message>> + 'a>>,
    pub(super) on_edit: Option<Box<dyn Fn(text::editor::Action) -> Message + 'a>>,
    /// Pixel scrolls, which `Action` can't carry - see
    /// [`TextEditor::on_scroll`]. Without one set, the wheel falls back to
    /// whole lines through `on_edit`.
    #[allow(clippy::type_complexity)]
    pub(super) on_scroll: Option<Box<dyn Fn(f32) -> Message + 'a>>,
    pub(super) highlighter_settings: Highlighter::Settings,
    pub(super) highlighter_format: fn(
        &Highlighter::Highlight,
        &Theme,
    ) -> text::highlighter::Format<Renderer::Font>,
    pub(super) last_status: Option<Status>,
}

impl<'a, Message, Theme, Renderer>
    TextEditor<'a, text::highlighter::PlainText, Message, Theme, Renderer>
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
            line_height: text::LineHeight::default(),
            width: Length::Fill,
            height: Length::Shrink,
            min_height: 0.0,
            max_height: f32::INFINITY,
            padding: Padding::new(5.0),
            wrapping: text::Wrapping::default(),
            scroll_sensitivity: 1.0,
            drag_speed: 1.0,
            tab_width: crate::indent::DEFAULT_WIDTH,
            class: <Theme as Catalog>::default(),
            key_binding: None,
            on_edit: None,
            on_scroll: None,
            highlighter_settings: (),
            highlighter_format: |_highlight, _theme| {
                text::highlighter::Format::default()
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
        on_edit: impl Fn(text::editor::Action) -> Message + 'a,
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

    /// Sets the [`Wrapping`](text::Wrapping) strategy of the [`TextEditor`].
    pub fn wrapping(mut self, wrapping: text::Wrapping) -> Self {
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
        ) -> text::highlighter::Format<Renderer::Font>,
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
        Theme::Class<'a>: From<super::theme::StyleFn<'a, Theme>>,
    {
        self.class = (Box::new(style) as super::theme::StyleFn<'a, Theme>).into();
        self
    }

    /// Sets the style class of the [`TextEditor`].
    #[must_use]
    pub fn class(mut self, class: impl Into<Theme::Class<'a>>) -> Self {
        self.class = class.into();
        self
    }

    pub(super) fn input_method<'b>(
        &self,
        state: &'b State<Highlighter>,
        renderer: &Renderer,
        layout: iced_core::layout::Layout<'_>,
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
            text::editor::Selection::Caret(position) => position,
            text::editor::Selection::Range(ranges) => {
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
                iced_core::Size::new(1.0, f32::from(line_height)),
            ),
            purpose: input_method::Purpose::Normal,
            preedit: state.preedit.as_ref().map(input_method::Preedit::as_ref),
        }
    }
}
