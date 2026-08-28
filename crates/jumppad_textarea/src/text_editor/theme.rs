use iced_core::{Background, Border, Color, Theme, theme};

/// The possible status of a [`super::TextEditor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The [`super::TextEditor`] can be interacted with.
    Active,
    /// The [`super::TextEditor`] is being hovered.
    Hovered,
    /// The [`super::TextEditor`] is focused.
    Focused {
        /// Whether the [`super::TextEditor`] is hovered, while focused.
        is_hovered: bool,
    },
    /// The [`super::TextEditor`] cannot be interacted with.
    Disabled,
}

/// The appearance of a text input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// The [`Background`] of the text input.
    pub background: Background,
    /// The [`Border`] of the text input.
    pub border: Border,
    /// The [`Color`] of the placeholder of the text input.
    pub placeholder: Color,
    /// The [`Color`] of the value of the text input.
    pub value: Color,
    /// The [`Color`] of the selection of the text input.
    pub selection: Color,
    /// The fill of the auto-hiding scrollbar's thumb, at full opacity.
    pub scrollbar_thumb: Color,
    /// The [`Color`] of the line numbers down the left, on every line but the
    /// one the caret is on - that one is drawn in [`Self::value`], so the
    /// caret's place in the document reads at a glance.
    pub line_number: Color,
}

/// How much of the document's text color the line numbers are drawn at by
/// default: enough to read, far enough back that the eye goes to the text
/// first.
pub const DEFAULT_LINE_NUMBER_ALPHA: f32 = 0.45;

/// The theme catalog of a [`super::TextEditor`].
pub trait Catalog: theme::Base {
    /// The item class of the [`Catalog`].
    type Class<'a>;

    /// The default class produced by the [`Catalog`].
    fn default<'a>() -> Self::Class<'a>;

    /// The [`Style`] of a class with the given status.
    fn style(&self, class: &Self::Class<'_>, status: Status) -> Style;
}

/// A styling function for a [`super::TextEditor`].
pub type StyleFn<'a, Theme> = Box<dyn Fn(&Theme, Status) -> Style + 'a>;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(default)
    }

    fn style(&self, class: &Self::Class<'_>, status: Status) -> Style {
        class(self, status)
    }
}

/// The default style of a [`super::TextEditor`].
pub fn default(theme: &Theme, status: Status) -> Style {
    let palette = theme.extended_palette();

    let active = Style {
        background: Background::Color(palette.background.base.color),
        border: Border {
            radius: 2.0.into(),
            width: 1.0,
            color: palette.background.strong.color,
        },
        placeholder: palette.secondary.base.color,
        value: palette.background.base.text,
        selection: palette.primary.weak.color,
        scrollbar_thumb: palette.background.strong.color,
        line_number: palette
            .background
            .base
            .text
            .scale_alpha(DEFAULT_LINE_NUMBER_ALPHA),
    };

    match status {
        Status::Active => active,
        Status::Hovered => Style {
            border: Border {
                color: palette.background.base.text,
                ..active.border
            },
            ..active
        },
        Status::Focused { .. } => Style {
            border: Border {
                color: palette.primary.strong.color,
                ..active.border
            },
            ..active
        },
        Status::Disabled => Style {
            background: Background::Color(palette.background.weak.color),
            value: active.placeholder,
            placeholder: palette.background.strongest.color,
            ..active
        },
    }
}
