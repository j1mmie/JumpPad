use serde::{Deserialize, Serialize};

use crate::editor::{EditorConfig, UiConfig};
use crate::font::DEFAULT_ALPHA;
use crate::is_default;
use crate::window::{BackgroundConfig, Blur, ForegroundConfig, DEFAULT_BLUR};

/// Which of the two theme slots is showing. Called an appearance rather than
/// a mode because `iced::window::Mode` is windowed/fullscreen/hidden, and the
/// app deals in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    /// The palette a slot falls back to when nothing names one - and the name
    /// an unconfigured slot carries, so a file of themes alone needs no
    /// `[mode]` section. Spelled to match `[themes.light]` and
    /// `[themes.dark]`; palettes are matched case-insensitively where they
    /// are applied, so one spelling serves both readings.
    pub fn default_palette(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// Which theme is showing, and whether the OS gets to decide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeConfig {
    #[serde(skip_serializing_if = "is_default")]
    pub detection: Detection,
    #[serde(skip_serializing_if = "is_default")]
    pub theme: ThemeSlots,
}

impl Default for ModeConfig {
    fn default() -> Self {
        Self {
            detection: Detection::Auto,
            theme: ThemeSlots::default(),
        }
    }
}

/// Which theme fills each of the two slots. A name here is a `[themes]`
/// entry - or, when no theme carries it, a palette to show that slot in.
///
/// Unnamed, the slots are `light` and `dark`, which a `[themes.light]` and a
/// `[themes.dark]` entry fill: a file that names its themes after the slots
/// they belong in needs no `[mode]` section at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSlots {
    pub light: String,
    pub dark: String,
}

impl Default for ThemeSlots {
    fn default() -> Self {
        Self {
            light: Appearance::Light.default_palette().to_string(),
            dark: Appearance::Dark.default_palette().to_string(),
        }
    }
}

impl ModeConfig {
    /// The slot to show. `Auto` follows the OS, and takes the light slot
    /// when the OS has no preference or couldn't be asked.
    pub fn showing(&self, os: Option<Appearance>) -> Appearance {
        match self.detection {
            Detection::Light => Appearance::Light,
            Detection::Dark => Appearance::Dark,
            Detection::Auto => os.unwrap_or(Appearance::Light),
        }
    }

    /// The slot this config holds the app to whatever the OS says, or `None`
    /// while it is following the OS.
    pub fn pinned(&self) -> Option<Appearance> {
        match self.detection {
            Detection::Light => Some(Appearance::Light),
            Detection::Dark => Some(Appearance::Dark),
            Detection::Auto => None,
        }
    }
}

/// Where the choice between the light and dark themes comes from.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Detection {
    /// Follow the OS, and keep following it while the app runs.
    #[default]
    Auto,
    /// Ignore the OS and stay on one slot.
    Light,
    Dark,
}

/// The theme every other theme takes its unnamed properties from. An ordinary
/// `[themes]` entry, so a slot can show it like any other.
pub(crate) const BASE_THEME: &str = "base";

/// One named theme: everything about JumpPad's appearance that can differ
/// between light and dark. A palette supplies the colors; the rest is what
/// JumpPad layers on top of them.
///
/// Every property is optional at every depth, and that is what makes
/// `[themes.base]` work: what a theme doesn't name comes from the base theme,
/// what the base theme doesn't name comes from JumpPad's own defaults, and
/// naming a property that happens to equal one of those defaults still counts
/// as naming it. See [`crate::Config::theme_for`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// The display name of an `iced::Theme` variant (e.g. `"Dracula"`),
    /// matched case-insensitively where it's applied - kept as a plain
    /// string so this crate doesn't need to depend on `iced`. Unnamed means
    /// the base theme's palette, and failing that the slot's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
    pub background: BackgroundConfig,
    pub foreground: ForegroundConfig,
    pub editor: EditorConfig,
    pub ui: UiConfig,
}

impl ThemeConfig {
    /// This theme with `base` supplying every property it doesn't name,
    /// property by property rather than section by section. Still optional at
    /// every leaf: what neither theme names is settled by [`Self::resolved`].
    pub(crate) fn with_defaults_from(
        &self,
        base: Option<&ThemeConfig>,
    ) -> ThemeConfig {
        let Some(base) = base else {
            return self.clone();
        };

        ThemeConfig {
            palette: self.palette.clone().or_else(|| base.palette.clone()),
            background: BackgroundConfig {
                alpha: self.background.alpha.or(base.background.alpha),
                blur: self.background.blur.or(base.background.blur),
            },
            foreground: ForegroundConfig {
                alpha: self.foreground.alpha.or(base.foreground.alpha),
            },
            editor: EditorConfig {
                font: self.editor.font.with_defaults_from(&base.editor.font),
                line_numbers: self
                    .editor
                    .line_numbers
                    .with_defaults_from(&base.editor.line_numbers),
            },
            ui: UiConfig {
                font: self.ui.font.with_defaults_from(&base.ui.font),
            },
        }
    }

    /// Every property settled against JumpPad's own defaults, and an unnamed
    /// palette against `fallback_palette` - the palette of the slot this
    /// theme is being shown in.
    pub(crate) fn resolved(&self, fallback_palette: &str) -> ResolvedTheme {
        ResolvedTheme {
            palette: self
                .palette
                .clone()
                .unwrap_or_else(|| fallback_palette.to_string()),
            background_alpha: self.background.alpha.unwrap_or(DEFAULT_ALPHA),
            background_blur: self.background.blur.unwrap_or(DEFAULT_BLUR),
            foreground_alpha: self.foreground.alpha.unwrap_or(DEFAULT_ALPHA),
            editor_font: self.editor.font.resolved(),
            line_numbers: self.editor.line_numbers.resolved(),
            ui_font: self.ui.font.resolved(),
        }
    }
}

/// A [`ThemeConfig`] with every fallback already applied - what the app
/// actually draws with. Flat, because nothing here is optional any more: the
/// sections a theme is written in shape the config file, not the values that
/// come out of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTheme {
    pub palette: String,
    pub background_alpha: f32,
    pub background_blur: Blur,
    pub foreground_alpha: f32,
    pub editor_font: ResolvedFont,
    pub line_numbers: ResolvedLineNumbers,
    pub ui_font: ResolvedFont,
}

/// Whether a document shows line numbers, and how far back from its text
/// they are drawn. Settled, like the rest of a [`ResolvedTheme`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedLineNumbers {
    pub enabled: bool,
    pub alpha: f32,
}

impl Default for ResolvedLineNumbers {
    fn default() -> Self {
        Self {
            enabled: crate::font::DEFAULT_LINE_NUMBERS_SHOWN,
            alpha: crate::font::DEFAULT_LINE_NUMBERS_ALPHA,
        }
    }
}

/// One surface's typeface and text size, settled. `family` stays optional
/// because an unnamed family is an answer the app acts on - it draws with the
/// system's own face for that surface.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFont {
    pub family: Option<String>,
    pub size: f32,
}

impl Default for ResolvedFont {
    fn default() -> Self {
        Self {
            family: None,
            size: crate::font::DEFAULT_FONT_SIZE,
        }
    }
}
