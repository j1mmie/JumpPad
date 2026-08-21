//! Every icon JumpPad can draw, named once.
//!
//! Icons ship as a font rather than as images. iced already rasterizes and
//! caches a glyph for every label on screen, so an icon drawn as text is one
//! more entry in a cache the app is paying for anyway; drawing the same
//! shapes as geometry would want the multisampling `jumppad::run` turns off
//! on purpose, and a rasterized SVG would want `resvg` in the binary.
//!
//! This crate is the manifest and nothing else - it names each icon, points
//! at the drawing it comes from, and fixes the codepoint the UI asks for.
//! `icon_font_builder` reads the same list to produce the font, so the two
//! cannot disagree about what is in it. Adding an icon is one row in
//! [`ICONS`] plus one file in `assets/icons/`, then `cargo build_fonts`.
//!
//! The font itself is embedded by `jumppad`, not here. The builder depends
//! on this crate, so a font baked in here would have to exist before the
//! tool that writes it could compile.

/// One icon in the font: what the product calls it, what it is drawn from,
/// and how the UI asks for it.
///
/// `drawing` is a file stem in `assets/icons/`, which is also the [Lucide]
/// icon name, because that is where these are vendored from. It is separate
/// from `name` so an icon can be called what it does here while staying
/// traceable to the drawing upstream ships.
///
/// [Lucide]: https://lucide.dev
pub struct Icon {
    pub name: &'static str,
    pub drawing: &'static str,
    pub codepoint: char,
}

/// Dismisses something - closes a tab, clears the find field.
pub const CLOSE: char = '\u{E000}';

/// Creates something - opens a new tab.
pub const ADD: char = '\u{E001}';

/// Every icon in the font, in the order glyphs are laid out in it.
///
/// Codepoints are JumpPad's own, assigned from the start of the Unicode
/// Private Use Area rather than copied from Lucide's font. Lucide reassigns
/// those across releases; these never move, so a rebuilt font stays
/// compatible with UI code that already asks for a glyph by name.
pub const ICONS: &[Icon] = &[
    Icon {
        name: "close",
        drawing: "x",
        codepoint: CLOSE,
    },
    Icon {
        name: "add",
        drawing: "plus",
        codepoint: ADD,
    },
];

/// The family the font records, and so the name UI code passes to
/// `iced::Font::with_name` to select it.
pub const FAMILY_NAME: &str = "JumpPad Icons";
