use editor_core::{SCROLLBAR_THUMB_WASH, scrollbar_wash};
use iced::{Background, Border, Color, Theme};

use crate::text_editor;

/// Scales `color`'s alpha by `alpha`, skipping the multiply at `1.0`.
pub(crate) fn apply_alpha(color: iced::Color, alpha: f32) -> iced::Color {
    if alpha >= 1.0 {
        color
    } else {
        color.scale_alpha(alpha)
    }
}

/// iced's default `text_editor` style draws a border that changes color on
/// hover/focus - dropped here so there's no color-change effect to notice.
/// Also drops its background on a transparent window and scales the base text
/// color by `foreground_alpha`, and the line numbers by `line_numbers_alpha`
/// on top of that - so the numbers stay a step back from the text however
/// faint the text itself is set. All plain parameters, for testability;
/// syntax-highlighted text instead goes through `color_for`.
pub(crate) fn editor_style(
    theme: &Theme,
    status: text_editor::Status,
    background_alpha: f32,
    foreground_alpha: f32,
    line_numbers_alpha: f32,
) -> text_editor::Style {
    let default = text_editor::default(theme, status);
    // The window background is already this exact color; repainting it
    // translucent would just stack a second layer (see AGENTS.md's
    // hairline-seam gotcha).
    let background = if background_alpha >= 1.0 {
        default.background
    } else {
        Background::Color(Color::TRANSPARENT)
    };
    let value = apply_alpha(default.value, foreground_alpha);
    text_editor::Style {
        border: Border {
            width: 0.0,
            ..Border::default()
        },
        background,
        value,
        line_number: apply_alpha(value, line_numbers_alpha),
        scrollbar_thumb: scrollbar_thumb_style(theme),
        ..default
    }
}

/// The scrollbar thumb's fill: a wash toward white on a dark theme, toward
/// black on a light one (see `editor_core::scrollbar_wash`), so the thumb
/// always reads as a step away from the document without needing a border to
/// stay visible on dark themes.
pub fn scrollbar_thumb_style(theme: &Theme) -> Color {
    scrollbar_wash(theme, SCROLLBAR_THUMB_WASH)
}
