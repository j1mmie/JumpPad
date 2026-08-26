use iced::widget::column;

use super::*;

/// The text the app draws around the editor: one font and one size, with
/// the smaller sizes derived from that size so the whole frame scales
/// together. The proportions are the ones the sizes had when they were
/// hardcoded at a 16px base.
///
/// The line heights are absolute so the tab strip's height - and with it
/// every horizontal quad edge in it - lands on a whole pixel. iced's default
/// `LineHeight::Relative(1.3)` would put the strip at 6 + 20.8 + 6 = 32.8px,
/// and a quad edge mid-pixel gets antialiased into a visible seam on a
/// transparent window (see AGENTS.md). Only holds at integer scale factors;
/// nothing chosen in logical pixels survives a 1.25x or 1.5x display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UiText {
    pub(super) font: Font,
    /// Tab titles draw at this; everything smaller is a fraction of it.
    pub(super) base: f32,
}

impl UiText {
    pub(super) fn new(font: Font, size: f32) -> Self {
        Self {
            font,
            base: jumppad_textarea::font::clamp_size(size),
        }
    }

    /// Tab titles, the new-tab button, and the filler that has to match
    /// their height.
    pub(super) fn tab_text<'a>(
        self,
        content: impl iced::widget::text::IntoFragment<'a>,
    ) -> iced::widget::Text<'a> {
        text(content)
            .font(self.font)
            .size(self.base)
            .line_height(Pixels((self.base * 1.3).floor()))
    }

    /// The compact controls: a tab's close button, and the find palette's
    /// and changed-on-disk bar's labels and buttons.
    pub(super) fn control_text<'a>(
        self,
        content: impl iced::widget::text::IntoFragment<'a>,
    ) -> iced::widget::Text<'a> {
        let size = self.control_size();

        text(content)
            .font(self.font)
            .size(size)
            .line_height(Pixels((size * 1.3).ceil()))
    }

    fn control_size(self) -> f32 {
        self.base * 0.75
    }

    /// The round button behind a tab's close icon. Only a little wider than
    /// the glyph, so it reads as a target around the icon rather than a
    /// button the icon happens to sit in, and rounded to whole pixels so its
    /// edge lands on the grid.
    /// How tall the tab strip stands: a title's line box plus the padding
    /// above and below it.
    pub(super) fn strip_height(self) -> f32 {
        TAB_VERTICAL_PADDING * 2.0 + (self.base * 1.3).floor()
    }

    pub(super) fn close_button_diameter(self) -> f32 {
        (self.control_size() * ICON_SCALE * 1.7).round()
    }

    /// The new-tab button's, a quarter wider again. It is the strip's one
    /// standing control rather than something that appears beside a title,
    /// so it can carry more weight.
    pub(super) fn new_tab_button_diameter(self) -> f32 {
        (self.close_button_diameter() * 1.25).round()
    }

    /// An icon at tab-title size. The face is the icon font rather than the
    /// configured UI one, because these codepoints sit in the Private Use
    /// Area, where every other face draws nothing.
    pub(super) fn tab_icon<'a>(self, icon: char) -> Text<'a> {
        self.tab_text(icon.to_string())
            .font(ICON_FONT)
            .size(self.base * ICON_SCALE)
    }

    /// An icon at the compact controls' size.
    pub(super) fn control_icon<'a>(self, icon: char) -> Text<'a> {
        self.control_text(icon.to_string())
            .font(ICON_FONT)
            .size(self.control_size() * ICON_SCALE)
    }

    /// Full sentences - dialog prompts, the error banner, the empty and
    /// drop-target states - which run at the default relative line height.
    pub(super) fn body_text<'a>(
        self,
        content: impl iced::widget::text::IntoFragment<'a>,
    ) -> iced::widget::Text<'a> {
        text(content).font(self.font).size(self.base)
    }

    /// The find palette's query field, which takes a bare size rather than
    /// a `Text`.
    pub(super) fn input_size(self) -> f32 {
        self.base * 0.875
    }
}

/// Matches a config-file font family against the families installed on this
/// machine, case-insensitively so hand-edited TOML doesn't have to get the
/// exact casing right. An unnamed or unavailable family takes `fallback` -
/// and logs why - rather than letting a name nothing provides draw text in
/// whatever face the platform substitutes for it.
pub(crate) fn resolve_font(family: Option<&str>, fallback: Font) -> Font {
    let Some(name) = family.map(str::trim).filter(|name| !name.is_empty())
    else {
        return fallback;
    };

    jumppad_textarea::font::installed(name).unwrap_or_else(|| {
        log::warn!(
            "font family {name:?} isn't installed, using the default font"
        );
        fallback
    })
}

/// The chrome's text for a theme's `ui.font` section. Its fallback is the default
/// sans face rather than the editor's monospace one: chrome set in a
/// monospaced face because a family was misspelled would look like a
/// different bug than the one it is.
pub(super) fn ui_text(font: &jumppad_config::ResolvedFont) -> UiText {
    UiText::new(
        resolve_font(font.family.as_deref(), Font::DEFAULT),
        font.size,
    )
}

/// How far toward black to shade a tab-bar surface, as an absolute drop in
/// brightness rather than a percentage - a percentage step is too small to see
/// against very dark themes.
const INACTIVE_TAB_DARKEN: f32 = 0.035; // ~9/255 per channel
pub(super) const TAB_ROW_DARKEN: f32 = 0.09; // ~23/255 per channel

/// A tab's text color - shared by the title button and the close button so
/// they always agree exactly, rather than each computing it separately and
/// risking drift.
fn tab_text_color(theme: &Theme, is_active: bool) -> Color {
    let text = theme.extended_palette().background.base.text;
    if is_active {
        text
    } else {
        text.scale_alpha(0.7)
    }
}

/// A tab's title button: always transparent - the enclosing `tab_frame_style`
/// container is what paints the tab's background, so title and close never
/// have to agree on a box size to look like one continuous surface.
pub(super) fn tab_title_style(
    theme: &Theme,
    _status: button::Status,
    is_active: bool,
) -> button::Style {
    let text_color = tab_text_color(theme, is_active);
    button::Style {
        background: None,
        text_color,
        ..button::Style::default()
    }
}

/// A tab's close button: transparent at rest (same reasoning as
/// `tab_title_style`), with a faint highlight only on hover/press as the
/// only background it ever paints itself.
pub(super) fn tab_close_style(
    theme: &Theme,
    status: button::Status,
    is_active: bool,
    diameter: f32,
) -> button::Style {
    let text_color = tab_text_color(theme, is_active);
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => {
            Some(text_color.scale_alpha(0.15).into())
        }
        _ => None,
    };
    button::Style {
        background,
        text_color,
        border: Border::default().rounded(diameter / 2.0),
        ..button::Style::default()
    }
}

/// The frame behind a tab's title+close row - the only thing that paints a
/// tab's background. The active tab paints nothing: the window background
/// already *is* the editor's background, so matching it seamlessly means
/// adding no layer at all. Inactive tabs get a darkening wash instead of a
/// border.
pub(super) fn tab_frame_style(
    theme: &Theme,
    is_active: bool,
) -> container::Style {
    if is_active {
        container::Style::default()
    } else {
        container::Style::default()
            .background(darkening_wash(theme, INACTIVE_TAB_DARKEN))
    }
}

/// The new-tab button: transparent and dim at rest so it doesn't compete
/// with the tabs themselves, taking the same round highlight a close button
/// does on hover. It paints nothing of its own at rest, so what shows
/// through is the tab row's background - which is why it sits in a container
/// styled with `tab_bar_style` rather than directly on the window.
pub(super) fn new_tab_style(
    theme: &Theme,
    status: button::Status,
    diameter: f32,
) -> button::Style {
    let text = theme.extended_palette().background.base.text;
    let (text_color, background) = match status {
        button::Status::Hovered | button::Status::Pressed => {
            (text, Some(text.scale_alpha(0.15).into()))
        }
        _ => (text.scale_alpha(0.4), None),
    };
    button::Style {
        background,
        text_color,
        border: Border::default().rounded(diameter / 2.0),
        ..button::Style::default()
    }
}

/// The tab row's own background - a wash darker than even an inactive tab, so
/// empty space past the last tab reads as a frame, not a gap.
pub(super) fn tab_bar_style(theme: &Theme) -> container::Style {
    container::Style::default()
        .background(darkening_wash(theme, TAB_ROW_DARKEN))
}

/// A round icon button: a circle of `diameter` with the glyph centred in
/// it, which is the shape its hover highlight takes. Sized rather than
/// padded, because padding around a glyph whose own box is taller than it is
/// wide would give an oval.
pub(super) fn round_icon_button<'a>(
    icon: Text<'a>,
    diameter: f32,
) -> button::Button<'a, Message> {
    button(icon.width(Fill).height(Fill).center())
        .width(diameter)
        .height(diameter)
        .padding(0)
}

/// One modal choice button, shared by both dialogs - they differ only in
/// their labels and the message each choice sends. The caller adds that with
/// `.on_press`, so a click resolves the dialog the same way Enter does.
pub(super) fn modal_choice(
    ui: UiText,
    label: &'static str,
    is_focused: bool,
) -> button::Button<'static, Message> {
    button(ui.body_text(label))
        .padding([6, 14])
        .style(move |theme, status| {
            modal_button_style(theme, status, is_focused)
        })
}

/// A modal's box: one line of prompt over a row of choices.
pub(super) fn modal_dialog<'a>(
    ui: UiText,
    prompt: String,
    choices: iced::widget::Row<'a, Message>,
) -> container::Container<'a, Message> {
    container(
        column![ui.body_text(prompt), choices.spacing(10)]
            .spacing(16)
            .padding(20),
    )
    .style(modal_dialog_style)
}

/// One of a modal's three choices - a colored border marks whichever one
/// keyboard nav currently has focused.
fn modal_button_style(
    theme: &Theme,
    status: button::Status,
    is_focused: bool,
) -> button::Style {
    let palette = theme.extended_palette();
    let border = if is_focused {
        iced::Border {
            color: palette.primary.strong.color,
            width: 2.0,
            radius: 4.0.into(),
        }
    } else {
        iced::Border {
            radius: 4.0.into(),
            ..iced::Border::default()
        }
    };
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => {
            Some(palette.background.weak.color.into())
        }
        _ => Some(palette.background.base.color.into()),
    };
    button::Style {
        background,
        text_color: palette.background.base.text,
        border,
        ..button::Style::default()
    }
}

pub(super) fn find_palette_style(theme: &Theme) -> container::Style {
    container::Style::default()
        .background(darkening_wash(theme, FLOATING_SURFACE_DARKEN))
}

/// How far the drag-and-drop overlay dims the document underneath. Lighter
/// than a floating surface - it covers the whole editor, and the text below it
/// should still read as text.
const DROP_OVERLAY_DARKEN: f32 = 0.08;

/// The overlay shown while files are dragged over the window. A wash, not a
/// pre-darkened background copy, so a transparent window stays transparent
/// through it (see AGENTS.md); the accent border is what carries the cue on
/// the macOS software build, which is always opaque.
pub(super) fn drop_overlay_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style::default()
        .background(darkening_wash(theme, DROP_OVERLAY_DARKEN))
        .border(iced::Border {
            color: palette.primary.base.color,
            width: 2.0,
            radius: 6.0.into(),
        })
}

pub(super) fn find_input_style(
    theme: &Theme,
    status: text_input::Status,
) -> text_input::Style {
    let palette = theme.extended_palette();
    let default = text_input::default(theme, status);
    text_input::Style {
        // The palette behind it is already a distinct surface; a second
        // filled quad on top would just compound opacity (see AGENTS.md).
        background: Color::TRANSPARENT.into(),
        border: iced::Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..default
    }
}

pub(super) fn find_button_style(
    theme: &Theme,
    status: button::Status,
) -> button::Style {
    let text_color = theme.extended_palette().background.base.text;
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => {
            Some(text_color.scale_alpha(0.15).into())
        }
        _ => None,
    };
    button::Style {
        background,
        text_color,
        ..button::Style::default()
    }
}

/// The modal's own dialog box - opaque, so it reads as a real window
/// sitting on top of the scrim rather than another translucent layer.
fn modal_dialog_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style::default()
        .background(palette.background.base.color)
        .border(iced::Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 8.0.into(),
        })
}

/// The full-window backdrop behind the modal dialog, dark and translucent to
/// show the rest of the app is blocked.
pub(super) fn modal_scrim_style(_theme: &Theme) -> container::Style {
    container::Style::default().background(Color::from_rgba(0.0, 0.0, 0.0, 0.5))
}

/// Whether this build has to premultiply the window's clear color before
/// handing it to iced - see `premultiply` for what goes wrong without it.
///
/// Two conditions have to hold. The backend must be `wgpu`, since only its
/// clear-color path writes straight alpha (`tiny-skia` premultiplies
/// internally, so feeding it a premultiplied color would double-darken).
/// And the platform compositor must read the presented surface as
/// premultiplied - confirmed on the macOS window server and on Windows' DWM,
/// each by the same symptom: light themes going opaque while dark themes
/// looked fine.
///
/// Linux is the one holdout. Wayland and compositing X11 are premultiplied
/// too, so it very likely belongs here, but nobody has reproduced the
/// symptom there and a wrong guess costs opacity on a window that currently
/// looks right. The tell to watch for is a *light* theme, not a dark one.
pub(super) const CLEAR_COLOR_NEEDS_PREMULTIPLY: bool = cfg!(all(
    any(target_os = "macos", target_os = "windows"),
    feature = "wgpu"
));

/// Premultiplies a color's RGB by its alpha, on the sRGB-encoded channel
/// values - the space desktop compositors composite in.
///
/// **The solid-white-window fix.** The compositor composites the surface
/// as *premultiplied* alpha - `src + (1 - a) * desktop` - but iced's clear
/// color is written straight, so a straight white background saturates to
/// solid white at any alpha, while a near-black one (rgb ~ 0) happens to
/// look right; only light themes ever look broken. Quads and glyphs are
/// unaffected - iced's shaders premultiply before writing (see AGENTS.md).
///
/// That asymmetry is the whole diagnostic. `src_rgb` saturates the channel
/// on its own once `rgb` is near 1, so alpha stops mattering entirely and a
/// light theme reads as an opaque window at *any* configured alpha - even
/// 0.1. A dark theme at the same alpha looks nearly right. "Light themes are
/// opaque, dark themes are fine" means this bug; "everything is uniformly
/// too dark" means the opposite mistake, premultiplying where it isn't
/// wanted.
///
/// Not in linear space: the composite runs on encoded values, and
/// `encode(linear * a) > encode(linear) * a`, so premultiplying before the
/// sRGB encode over-brightens - white at alpha 0.1 came out ~3.5x too bright.
/// This also lands the wgpu build on exactly the bytes `tiny-skia` presents:
/// `iced_wgpu`'s clear color round-trips through `Color::into_linear()` and
/// back out through the sRGB surface's encode, so the encoded value written
/// is the one passed in here.
pub(super) fn premultiply(color: Color) -> Color {
    Color {
        r: color.r * color.a,
        g: color.g * color.a,
        b: color.b * color.a,
        a: color.a,
    }
}
