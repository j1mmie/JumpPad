//! Frames painted through the software renderer, the way the Windows build
//! paints them: `softbuffer` hands back the previous frame's buffer there
//! (`age() == 1`), so `iced_tiny_skia` repaints only what its damage tracking
//! says changed, and anything painted outside a damaged region stays on
//! screen until something else happens to cover it.

use iced_core::layout::{self, Layout};
use iced_core::text::LineHeight;
use iced_core::text::highlighter;
use iced_core::widget::{Tree, Widget};
use iced_core::{
    Color, Font, Pixels, Rectangle, Renderer as _, Size, Theme, Vector, mouse,
    renderer as core_renderer,
};
use iced_tiny_skia::Renderer;
use iced_tiny_skia::graphics::{Viewport, damage};

use jumppad_textarea::text_editor::{Content, TextEditor, text_editor};

const WINDOW: Size<u32> = Size::new(500, 400);
/// Where the editor sits, with room above it for something else - a tab bar,
/// in the app.
const ORIGIN: Vector = Vector::new(20.0, 60.0);
const SIZE: Size = Size::new(460.0, 300.0);
const PADDING: f32 = 5.0;
const LINE_HEIGHT: f32 = 20.0;
const DOCUMENT_LINES: usize = 200;
const BACKGROUND: Color = Color::BLACK;

#[derive(Debug, Clone)]
enum Message {}

/// One window's worth of software rendering: the pixels, the layers the last
/// frame left behind, and the damage tracking that decides what gets painted.
struct Window {
    content: Content<Renderer>,
    tree: Tree,
    renderer: Renderer,
    node: layout::Node,
    pixels: Vec<u8>,
    mask: tiny_skia::Mask,
    viewport: Viewport,
    last_layers: Option<Vec<iced_tiny_skia::Layer>>,
}

impl Window {
    fn new() -> Self {
        let content = Content::with_text(&document());
        let renderer = Renderer::new(Font::MONOSPACE, Pixels(14.0));
        let tree =
            Tree::new(&editor(&content) as &dyn Widget<Message, Theme, _>);

        let mut window = Self {
            content,
            tree,
            renderer,
            node: layout::Node::new(SIZE),
            pixels: vec![0; WINDOW.width as usize * WINDOW.height as usize * 4],
            mask: tiny_skia::Mask::new(WINDOW.width, WINDOW.height)
                .expect("a clip mask"),
            viewport: Viewport::with_physical_size(WINDOW, 1.0),
            last_layers: None,
        };
        window.lay_out();

        window
    }

    fn lay_out(&mut self) {
        let node = {
            let mut widget = editor(&self.content);

            widget.layout(
                &mut self.tree,
                &self.renderer,
                &layout::Limits::new(Size::ZERO, SIZE),
            )
        };

        self.node = node;
    }

    /// Paints a frame the way `iced_tiny_skia`'s compositor does on Windows,
    /// and reports the regions it decided to repaint.
    fn present(&mut self) -> Vec<Rectangle> {
        self.lay_out();
        self.renderer
            .reset(Rectangle::with_size(self.viewport.logical_size()));

        {
            let widget = editor(&self.content);

            widget.draw(
                &self.tree,
                &mut self.renderer,
                &Theme::Dark,
                &core_renderer::Style {
                    text_color: Color::WHITE,
                },
                Layout::with_offset(ORIGIN, &self.node),
                mouse::Cursor::Unavailable,
                &Rectangle::with_size(Size::INFINITE),
            );
        }

        let layers = self.renderer.layers().to_vec();
        let damage = match &self.last_layers {
            // The first frame has nothing to compare against, so everything
            // is painted - as it is after a resize, or whenever the app's
            // background color changes (the redraw nudge).
            None => vec![Rectangle::with_size(self.viewport.logical_size())],
            Some(last) => damage::diff(
                last,
                &layers,
                |layer| vec![layer.bounds],
                iced_tiny_skia::Layer::damage,
            ),
        };
        self.last_layers = Some(layers);

        let damage = damage::group(
            damage,
            Rectangle::with_size(self.viewport.logical_size()),
        );

        if !damage.is_empty() {
            let mut pixels = tiny_skia::PixmapMut::from_bytes(
                &mut self.pixels,
                WINDOW.width,
                WINDOW.height,
            )
            .expect("a pixel map");

            self.renderer.draw(
                &mut pixels,
                &mut self.mask,
                &self.viewport,
                &damage,
                BACKGROUND,
            );
        }

        damage
    }

    /// Pixels painted over the background in the band the app puts its tab
    /// bar in - above the editor's own rectangle entirely.
    fn painted_above_the_widget(&self) -> Vec<(u32, u32)> {
        self.painted_between(0, ORIGIN.y as u32)
    }

    /// The same below it, where the app has its window edge.
    fn painted_below_the_widget(&self) -> Vec<(u32, u32)> {
        self.painted_between((ORIGIN.y + SIZE.height) as u32 + 1, WINDOW.height)
    }

    /// Pixels of text in the padding, between the editor's own edge and the
    /// text area inside it - the near half of the same overhang, landing on
    /// the widget's background instead of on the tab bar.
    fn painted_on_the_padding(&self) -> Vec<(u32, u32)> {
        // Inside the widget's rounded border, which is a color of its own.
        let inset = 3;
        let left = ORIGIN.x as u32 + inset;
        let right = (ORIGIN.x + SIZE.width) as u32 - inset;
        let background =
            self.pixel(left, (ORIGIN.y + SIZE.height / 2.0) as u32);

        let bands = [
            (ORIGIN.y as u32 + inset, (ORIGIN.y + PADDING) as u32),
            (
                (ORIGIN.y + SIZE.height - PADDING) as u32,
                (ORIGIN.y + SIZE.height) as u32 - inset,
            ),
        ];

        bands
            .into_iter()
            .flat_map(|(from, to)| {
                (from..to).flat_map(move |y| (left..right).map(move |x| (x, y)))
            })
            .filter(|(x, y)| self.pixel(*x, *y) != background)
            .collect()
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * WINDOW.width as usize + x as usize) * 4;

        self.pixels[at..at + 4].try_into().expect("four channels")
    }

    fn painted_inside_the_editor(&self) -> usize {
        self.painted_between(ORIGIN.y as u32 + 40, ORIGIN.y as u32 + 200)
            .len()
    }

    /// Every pixel between two rows that isn't the background color.
    fn painted_between(&self, from: u32, to: u32) -> Vec<(u32, u32)> {
        let mut painted = Vec::new();

        for y in from..to {
            for x in 0..WINDOW.width {
                let at = (y as usize * WINDOW.width as usize + x as usize) * 4;
                // The background is black, so any color at all is paint.
                if self.pixels[at..at + 3].iter().any(|channel| *channel != 0) {
                    painted.push((x, y));
                }
            }
        }

        painted
    }
}

fn other_document() -> String {
    (0..DOCUMENT_LINES)
        .map(|line| format!("a different line {line}, with other words"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn document() -> String {
    (0..DOCUMENT_LINES)
        .map(|line| format!("line {line} with some words on it"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn editor(
    content: &Content<Renderer>,
) -> TextEditor<'_, highlighter::PlainText, Message, Theme, Renderer> {
    text_editor(content)
        .font(Font::MONOSPACE)
        .size(14.0)
        .line_height(LineHeight::Absolute(Pixels(LINE_HEIGHT)))
        .padding(PADDING)
        .height(SIZE.height)
}

/// The editor draws the rows the top and bottom edges cut through *whole* -
/// that is what scrolling by pixels means - so its own rectangle is the only
/// thing standing between that overhang and the tab bar above it.
#[test]
fn a_pixel_scrolled_editor_paints_nothing_outside_itself() {
    let mut window = Window::new();
    let _ = window.present();

    window.content.scroll_by(LINE_HEIGHT / 2.0);
    let _ = window.present();

    assert_eq!(window.painted_above_the_widget(), Vec::new());
    assert_eq!(window.painted_below_the_widget(), Vec::new());
    assert_eq!(window.painted_on_the_padding(), Vec::new());
}

/// A view whose height is not a whole number of rows has a cut row at the
/// bottom whatever it is scrolled to, so this one holds even at rest.
#[test]
fn an_editor_at_the_top_of_its_document_paints_nothing_outside_itself() {
    let mut window = Window::new();
    let _ = window.present();

    assert_eq!(window.painted_above_the_widget(), Vec::new());
    assert_eq!(window.painted_below_the_widget(), Vec::new());
    assert_eq!(window.painted_on_the_padding(), Vec::new());
}

/// The failure this is really about. Windows keeps the last frame's buffer,
/// so only damaged regions are repainted - and the band above the editor is
/// damaged by nothing. Anything painted up there stays, and a scroll's worth
/// of overhang piles up into a smear of old text.
#[test]
fn the_band_above_the_editor_stays_clean_across_a_long_scroll() {
    let mut window = Window::new();
    let _ = window.present();

    for _ in 0..30 {
        window.content.scroll_by(LINE_HEIGHT / 3.0);
        let _ = window.present();
    }

    let smeared = window.painted_above_the_widget();
    assert!(
        smeared.is_empty(),
        "{} pixels of old text left above the editor",
        smeared.len()
    );
}

/// A repaint of only part of the editor must stay inside that part, for the
/// same reason: what it paints outside is never cleaned up.
#[test]
fn a_partial_repaint_paints_nothing_outside_itself() {
    let mut window = Window::new();
    let _ = window.present();

    window.content.scroll_by(LINE_HEIGHT / 2.0);
    let damage = window.present();

    assert!(
        damage.iter().all(|region| region.y >= ORIGIN.y),
        "the editor should not be asking for the band above it: {damage:?}"
    );
}

/// The clip is a sliver shorter than the text area; the sliver must not cost
/// a pixel of the text itself.
#[test]
fn the_editor_still_paints_its_text() {
    let mut window = Window::new();
    let _ = window.present();

    assert!(
        window.painted_inside_the_editor() > 1_000,
        "the document should be on screen"
    );
}

/// The failure the redraw nudge in `app.rs` exists for: switching tabs puts a
/// different document under the same widget, with the same font, bounds and
/// metrics - and those three are all iced's editor comparison looks at, so it
/// can conclude nothing changed and repaint nothing.
///
/// It does repaint today, but by accident rather than by that comparison:
/// `Editor::update` mints a fresh `Arc` on every layout, which leaves the
/// previous frame's weak reference dangling and makes the comparison fail
/// before it can compare anything. This is the tripwire for that accident
/// going away.
#[test]
fn a_different_document_under_the_same_widget_repaints() {
    let mut window = Window::new();
    let _ = window.present();
    let before = window.pixels.clone();

    window.content = Content::with_text(&other_document());
    let damage = window.present();

    assert!(
        damage.iter().any(|region| region.width > SIZE.width / 2.0),
        "the editor should be asking for a repaint: {damage:?}"
    );
    let repainted = before
        .chunks(4)
        .zip(window.pixels.chunks(4))
        .filter(|(was, now)| was != now)
        .count();
    assert!(
        repainted > 1_000,
        "the new document should be on screen, not the old one: \
         {repainted} pixels changed"
    );
}
