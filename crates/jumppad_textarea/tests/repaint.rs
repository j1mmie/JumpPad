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
    /// One entry per frame, newest first - the compositor's own record of
    /// what each buffer in rotation still holds.
    layer_stack: Vec<Vec<iced_tiny_skia::Layer>>,
    /// How many frames back the buffer this frame lands in was last painted,
    /// which is what `softbuffer` reports as its age. Windows and X11 say 1.
    age: usize,
    theme: Theme,
}

impl Window {
    fn new() -> Self {
        Self::with_buffers(1)
    }

    /// A window whose platform presents through `age` buffers in rotation.
    fn with_buffers(age: usize) -> Self {
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
            layer_stack: Vec::new(),
            age,
            theme: Theme::Dark,
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
                &self.theme,
                &core_renderer::Style {
                    text_color: Color::WHITE,
                },
                Layout::with_offset(ORIGIN, &self.node),
                mouse::Cursor::Unavailable,
                &Rectangle::with_size(Size::INFINITE),
            );
        }

        let layers = self.renderer.layers().to_vec();
        // What the buffer this frame lands in still holds. Nothing, until the
        // rotation has come all the way round once - and then everything is
        // painted, as it is after a resize.
        let damage = match self.layer_stack.get(self.age - 1) {
            None => vec![Rectangle::with_size(self.viewport.logical_size())],
            Some(held) => damage::diff(
                held,
                &layers,
                |layer| vec![layer.bounds],
                iced_tiny_skia::Layer::damage,
            ),
        };
        self.layer_stack.insert(0, layers);
        self.layer_stack.truncate(self.age);

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

/// The failure the redraw nudge in `app.rs` used to exist for: a different
/// document under the same widget has the same font, bounds and metrics, and
/// those three are all `Internal` compares - so iced's editor comparison could
/// conclude nothing changed and repaint nothing.
///
/// The fork answers it on which editor the reference points at now
/// (`PartialEq for Weak`). This is the tripwire for that going away.
#[test]
fn a_different_document_under_the_same_widget_repaints() {
    assert_repaints_a_document_swap(Window::new());
}

/// The same, on a platform that presents through several buffers: the one a
/// frame lands in was last painted three frames ago, so that is what its
/// damage is measured against. The nudge scaled itself by a frame counter for
/// exactly this reason.
#[test]
fn a_document_swap_repaints_through_a_rotating_swapchain() {
    let mut window = Window::with_buffers(3);
    for _ in 0..4 {
        let _ = window.present();
    }

    assert_repaints_a_document_swap(window);
}

/// A palette swap moves no widget, which is the other thing the nudge covered:
/// the colors have to be enough on their own.
#[test]
fn a_theme_change_repaints_the_editor() {
    let mut window = Window::new();
    let _ = window.present();
    let before = window.pixels.clone();

    window.theme = Theme::SolarizedLight;
    let damage = window.present();

    assert!(
        !damage.is_empty(),
        "a palette swap should ask for a repaint"
    );
    assert!(
        changed_pixels(&before, &window.pixels) > 1_000,
        "the new palette should be on screen"
    );
}

/// A tab switch, which is not the swap above however much it looks like one -
/// and the case that got past it. The tab being left keeps its `Content`, so
/// the weak reference last frame's primitive holds still upgrades, and the
/// comparison the swap above never reaches runs after all. Two documents in
/// the same widget answer font, bounds and metrics identically, so the editor
/// asked for no damage whatsoever: the old text stayed on screen until the
/// next event laid the widget out again, which made switching tabs look like
/// a delay rather than a wrong picture.
#[test]
fn a_switch_between_two_live_documents_repaints() {
    let mut window = Window::new();
    let _ = window.present();
    let before = window.pixels.clone();

    // The tab being switched away from, still open behind the new one.
    let _switched_away = std::mem::replace(
        &mut window.content,
        Content::with_text(&other_document()),
    );
    let damage = window.present();

    assert!(
        damage.iter().any(|region| region.width > SIZE.width / 2.0),
        "the editor should be asking for a repaint: {damage:?}"
    );
    let repainted = changed_pixels(&before, &window.pixels);
    assert!(
        repainted > 1_000,
        "the new document should be on screen, not the old one: \
         {repainted} pixels changed"
    );
}

fn assert_repaints_a_document_swap(mut window: Window) {
    let _ = window.present();
    let before = window.pixels.clone();

    window.content = Content::with_text(&other_document());
    let damage = window.present();

    assert!(
        damage.iter().any(|region| region.width > SIZE.width / 2.0),
        "the editor should be asking for a repaint: {damage:?}"
    );
    let repainted = changed_pixels(&before, &window.pixels);
    assert!(
        repainted > 1_000,
        "the new document should be on screen, not the old one: \
         {repainted} pixels changed"
    );
}

fn changed_pixels(before: &[u8], after: &[u8]) -> usize {
    before
        .chunks(4)
        .zip(after.chunks(4))
        .filter(|(was, now)| was != now)
        .count()
}
