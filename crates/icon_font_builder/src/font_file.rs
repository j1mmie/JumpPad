//! Assembles glyph outlines into a TrueType file.
//!
//! Only the tables needed to select the family by name and find a glyph from
//! a codepoint. Nothing describes how glyphs sit next to each other, because
//! icons are drawn one at a time and never as a run, so kerning and shaping
//! data would be weight with no reader.

use crate::icon_outline::{ASCENT, DESCENT, UNITS_PER_EM};
use kurbo::BezPath;
use std::error::Error;
use write_fonts::{
    FontBuilder,
    tables::{
        cmap::Cmap,
        glyf::{self, Bbox, GlyfLocaBuilder, SimpleGlyph},
        head::Head,
        hhea::Hhea,
        hmtx::{Hmtx, LongMetric},
        loca::{Loca, LocaFormat},
        maxp::Maxp,
        name::{Name, NameRecord},
        os2::{Os2, SelectionFlags},
        post::Post,
    },
    types::{FWord, Fixed, GlyphId, NameId, Tag, UfWord},
};

/// The name a glyph goes by inside the font, before it has a codepoint.
/// TrueType calls glyph 0 `.notdef` and draws it when nothing else matches.
const NOTDEF: &str = ".notdef";

/// One icon, ready to become a glyph.
pub struct Glyph {
    pub name: &'static str,
    pub codepoint: char,
    pub outline: BezPath,
}

pub fn assemble(icons: &[Glyph]) -> Result<Vec<u8>, Box<dyn Error>> {
    let shapes = shapes_of(icons)?;
    let (glyf, loca, loca_format) = glyf_and_loca(&shapes)?;

    let mut font = FontBuilder::new();
    font.add_table(&head(&shapes, loca_format))?;
    font.add_table(&hhea(&shapes))?;
    font.add_table(&maxp(shapes.len()))?;
    font.add_table(&hmtx(&shapes))?;
    font.add_table(&cmap(icons)?)?;
    font.add_table(&name())?;
    font.add_table(&os2(icons))?;
    font.add_table(&post(icons))?;
    font.add_table(&glyf)?;
    font.add_table(&loca)?;
    Ok(font.build())
}

/// Every glyph in the font, `.notdef` first as TrueType requires. It is left
/// empty: an icon font asked for a codepoint it does not have should draw
/// nothing rather than the usual box, which would read as a rendering fault.
fn shapes_of(icons: &[Glyph]) -> Result<Vec<glyf::Glyph>, Box<dyn Error>> {
    let mut shapes = vec![glyf::Glyph::Empty];
    for icon in icons {
        let outline = SimpleGlyph::from_bezpath(&icon.outline)
            .map_err(|error| format!("{}: {error:?}", icon.name))?;
        shapes.push(glyf::Glyph::Simple(outline));
    }
    Ok(shapes)
}

fn glyf_and_loca(
    shapes: &[glyf::Glyph],
) -> Result<(glyf::Glyf, Loca, LocaFormat), Box<dyn Error>> {
    let mut builder = GlyfLocaBuilder::new();
    for shape in shapes {
        builder.add_glyph(shape)?;
    }
    Ok(builder.build())
}

fn bounds(shapes: &[glyf::Glyph]) -> Bbox {
    shapes
        .iter()
        .filter_map(glyf::Glyph::bbox)
        .reduce(Bbox::union)
        .unwrap_or_default()
}

/// Timestamps are pinned to the TrueType epoch rather than the clock. The
/// font is committed beside the drawings it is built from, and one that
/// changed on every build would turn every rebuild into a diff.
fn head(shapes: &[glyf::Glyph], loca_format: LocaFormat) -> Head {
    let bounds = bounds(shapes);
    Head {
        font_revision: Fixed::from_f64(1.0),
        units_per_em: UNITS_PER_EM,
        x_min: bounds.x_min,
        y_min: bounds.y_min,
        x_max: bounds.x_max,
        y_max: bounds.y_max,
        lowest_rec_ppem: 8,
        index_to_loc_format: loca_format as i16,
        ..Default::default()
    }
}

fn hhea(shapes: &[glyf::Glyph]) -> Hhea {
    let bounds = bounds(shapes);
    Hhea {
        ascender: FWord::new(ASCENT),
        descender: FWord::new(DESCENT),
        line_gap: FWord::new(0),
        advance_width_max: UfWord::new(UNITS_PER_EM),
        min_left_side_bearing: FWord::new(bounds.x_min),
        min_right_side_bearing: FWord::new(UNITS_PER_EM as i16 - bounds.x_max),
        x_max_extent: FWord::new(bounds.x_max),
        caret_slope_rise: 1,
        number_of_h_metrics: shapes.len() as u16,
        ..Default::default()
    }
}

fn maxp(glyph_count: usize) -> Maxp {
    Maxp {
        num_glyphs: glyph_count as u16,
        ..Default::default()
    }
}

/// Every glyph is one em wide, because every drawing fills the same square
/// box. That is what keeps a column of icons aligned without the UI
/// measuring anything.
fn hmtx(shapes: &[glyf::Glyph]) -> Hmtx {
    let metrics = shapes
        .iter()
        .map(|shape| LongMetric {
            advance: UNITS_PER_EM,
            side_bearing: shape.bbox().map(|bbox| bbox.x_min).unwrap_or(0),
        })
        .collect();
    Hmtx::new(metrics, Vec::new())
}

fn cmap(icons: &[Glyph]) -> Result<Cmap, Box<dyn Error>> {
    let mappings = icons
        .iter()
        .enumerate()
        .map(|(index, icon)| (icon.codepoint, GlyphId::new(index as u32 + 1)));
    Ok(Cmap::from_mappings(mappings)?)
}

/// The Windows/Unicode/English records every toolkit looks for. iced matches
/// on the family, so that record is the one that has to be right.
fn name() -> Name {
    let family = jumppad_icons::FAMILY_NAME;
    let postscript = family.replace(' ', "");
    Name::new(vec![
        english(NameId::FAMILY_NAME, family.to_owned()),
        english(NameId::SUBFAMILY_NAME, "Regular".to_owned()),
        english(NameId::UNIQUE_ID, format!("{family} Regular")),
        english(NameId::FULL_NAME, format!("{family} Regular")),
        english(NameId::VERSION_STRING, "Version 1.000".to_owned()),
        english(NameId::POSTSCRIPT_NAME, format!("{postscript}-Regular")),
    ])
}

fn english(name_id: NameId, text: String) -> NameRecord {
    const WINDOWS: u16 = 3;
    const UNICODE_BMP: u16 = 1;
    const AMERICAN_ENGLISH: u16 = 0x0409;
    NameRecord::new(
        WINDOWS,
        UNICODE_BMP,
        AMERICAN_ENGLISH,
        name_id,
        text.into(),
    )
}

fn os2(icons: &[Glyph]) -> Os2 {
    /// Bit 60 of the Unicode ranges, counted from the start of the second
    /// word, marks a font that covers the Private Use Area.
    const PRIVATE_USE_AREA: u32 = 1 << 28;
    let codepoints = icons.iter().map(|icon| icon.codepoint as u16);
    Os2 {
        us_weight_class: 400,
        us_width_class: 5,
        us_first_char_index: codepoints.clone().min().unwrap_or(0),
        us_last_char_index: codepoints.max().unwrap_or(0),
        ul_unicode_range_2: PRIVATE_USE_AREA,
        s_typo_ascender: ASCENT,
        s_typo_descender: DESCENT,
        s_typo_line_gap: 0,
        us_win_ascent: ASCENT as u16,
        us_win_descent: DESCENT.unsigned_abs(),
        fs_selection: SelectionFlags::REGULAR,
        ach_vend_id: Tag::new(b"JMPD"),
        ..Default::default()
    }
}

/// Version 2 keeps the glyph names, which is what lets a font editor show
/// `close` and `add` rather than two anonymous outlines when the font is
/// opened to check it.
fn post(icons: &[Glyph]) -> Post {
    let names =
        std::iter::once(NOTDEF).chain(icons.iter().map(|icon| icon.name));
    Post {
        italic_angle: Fixed::ZERO,
        underline_position: FWord::new(DESCENT),
        underline_thickness: FWord::new(0),
        ..Post::new_v2(names)
    }
}
