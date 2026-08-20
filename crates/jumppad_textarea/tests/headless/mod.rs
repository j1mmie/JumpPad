//! A renderer that draws nothing, so the editor widget can be driven by a
//! test without a window or a graphics backend.
//!
//! Only the drawing is stubbed out. Text still goes through
//! `iced_graphics`'s cosmic-text editor - the same one the app uses - which
//! is what makes shaping, hit testing and scrolling real here.

use iced::advanced::graphics;
use iced::advanced::text::{self, Text};
use iced::{Background, Color, Font, Pixels, Point, Rectangle, Transformation};
use iced_core::image;
use iced_core::renderer;

pub struct Renderer;

impl renderer::Renderer for Renderer {
    fn start_layer(&mut self, _bounds: Rectangle) {}

    fn end_layer(&mut self) {}

    fn start_transformation(&mut self, _transformation: Transformation) {}

    fn end_transformation(&mut self) {}

    fn reset(&mut self, _new_bounds: Rectangle) {}

    fn fill_quad(
        &mut self,
        _quad: renderer::Quad,
        _background: impl Into<Background>,
    ) {
    }

    fn allocate_image(
        &mut self,
        _handle: &image::Handle,
        _callback: impl FnOnce(Result<image::Allocation, image::Error>)
        + Send
        + 'static,
    ) {
        unimplemented!("the editor draws no images")
    }
}

impl text::Renderer for Renderer {
    type Font = Font;
    type Paragraph = graphics::text::Paragraph;
    type Editor = graphics::text::Editor;

    const ICON_FONT: Font = Font::DEFAULT;
    const CHECKMARK_ICON: char = '0';
    const ARROW_DOWN_ICON: char = '0';
    const SCROLL_UP_ICON: char = '0';
    const SCROLL_DOWN_ICON: char = '0';
    const SCROLL_LEFT_ICON: char = '0';
    const SCROLL_RIGHT_ICON: char = '0';
    const ICED_LOGO: char = '0';

    fn default_font(&self) -> Self::Font {
        Font::MONOSPACE
    }

    fn default_size(&self) -> Pixels {
        Pixels(14.0)
    }

    fn fill_paragraph(
        &mut self,
        _paragraph: &Self::Paragraph,
        _position: Point,
        _color: Color,
        _clip_bounds: Rectangle,
    ) {
    }

    fn fill_editor(
        &mut self,
        _editor: &Self::Editor,
        _position: Point,
        _color: Color,
        _clip_bounds: Rectangle,
    ) {
    }

    fn fill_text(
        &mut self,
        _text: Text<String, Self::Font>,
        _position: Point,
        _color: Color,
        _clip_bounds: Rectangle,
    ) {
    }
}
