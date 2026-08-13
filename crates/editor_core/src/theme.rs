use iced::{Color, Theme};

/// How opaque a [`darkening_wash`] is ever allowed to get. Themes whose
/// background is near-black hit this before reaching the requested step -
/// darkening a transparent window costs opacity, and a solid surface is a
/// worse trade than a shallower one.
pub const WASH_ALPHA_CEILING: f32 = 0.8;

/// How far toward black a surface that floats over document text sits - the
/// find palette and the scrollbar thumb. Deeper than a tab's shading because
/// it has to read as its own surface against the text, but still a wash, so a
/// transparent window stays transparent through it. Shared so the two keep
/// reading as the same material rather than drifting apart.
pub const FLOATING_SURFACE_DARKEN: f32 = 0.14;

/// How far the scrollbar thumb's wash pushes away from the background - its
/// own constant, decoupled from [`FLOATING_SURFACE_DARKEN`], since the thumb
/// no longer has to match the find palette's material.
pub const SCROLLBAR_THUMB_WASH: f32 = 0.14;

/// A translucent black overlay that darkens whatever it covers by roughly
/// `amount`, rather than a pre-darkened copy of the background - on a
/// transparent window a full-alpha copy stacks with the window background and
/// leaves a visible opacity step (see AGENTS.md's hairline-seam gotcha).
/// Solid, it matches the subtractive darkening it replaced on average, shading
/// channels proportionally instead of uniformly.
pub fn darkening_wash(theme: &Theme, amount: f32) -> Color {
    let base = theme.extended_palette().background.base.color;
    let luminance = (base.r + base.g + base.b) / 3.0;
    // A background not much brighter than `amount` can't give up that much
    // light without a solid wash, which would turn the surface into an opaque
    // bar on a transparent window. Those themes get a shallower step instead.
    let alpha = if luminance > 0.0 {
        (amount / luminance).clamp(0.0, WASH_ALPHA_CEILING)
    } else {
        0.0
    };
    Color {
        a: alpha,
        ..Color::BLACK
    }
}

/// The mirror of [`darkening_wash`] - a translucent white overlay, for themes
/// where a wash toward black has almost nothing left to darken.
fn lightening_wash(theme: &Theme, amount: f32) -> Color {
    let base = theme.extended_palette().background.base.color;
    let luminance = (base.r + base.g + base.b) / 3.0;
    let headroom = 1.0 - luminance;
    let alpha = if headroom > 0.0 {
        (amount / headroom).clamp(0.0, WASH_ALPHA_CEILING)
    } else {
        0.0
    };
    Color {
        a: alpha,
        ..Color::WHITE
    }
}

/// The scrollbar thumb's wash: lightens a dark theme, darkens a light one, via
/// `extended_palette().is_dark` (iced's own OKLCH-lightness threshold), so the
/// thumb always reads as a step away from the document without needing a
/// border to stay visible on dark themes.
pub fn scrollbar_wash(theme: &Theme, amount: f32) -> Color {
    if theme.extended_palette().is_dark {
        lightening_wash(theme, amount)
    } else {
        darkening_wash(theme, amount)
    }
}
