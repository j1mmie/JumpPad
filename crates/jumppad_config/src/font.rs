use serde::{Deserialize, Serialize};

use crate::theme::ResolvedFont;

/// The typeface and text size one surface is drawn with. The same shape
/// serves the editor and the chrome; what differs is what the size means.
///
/// `family` names a font family already installed on the machine, spelled
/// the way the system lists it (`"JetBrains Mono"`, `"Cascadia Code"`).
/// Leaving it out draws with what the system offers - its monospace face
/// for documents, its interface face for the chrome. Nothing here is loaded
/// from a file: a family the machine doesn't have is reported at startup
/// and ignored.
///
/// `size` is the height of the editor's text in pixels; in the chrome it is
/// the tab titles' height, and the smaller text keeps its proportion to it,
/// so one number scales the whole frame. Both are matched and clamped where
/// applied, not here, so this crate doesn't need an `iced` dependency.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
}

impl FontConfig {
    /// This font with `base` supplying whichever of the two properties it
    /// doesn't name - so a base family and a theme's own size both survive.
    pub(crate) fn with_defaults_from(&self, base: &FontConfig) -> FontConfig {
        FontConfig {
            family: self.family.clone().or_else(|| base.family.clone()),
            size: self.size.or(base.size),
        }
    }

    pub(crate) fn resolved(&self) -> ResolvedFont {
        ResolvedFont {
            family: self.family.clone(),
            size: self.size.unwrap_or(DEFAULT_FONT_SIZE),
        }
    }
}

/// The text size a surface falls back to when `config.toml` doesn't name
/// one. Matches iced's own default text size, so an absent `size` draws
/// exactly what JumpPad drew before the setting existed.
pub const DEFAULT_FONT_SIZE: f32 = 16.0;

/// The opacity a surface falls back to when neither the theme nor the base
/// theme names one: fully solid, exactly what JumpPad drew before the setting
/// existed.
pub const DEFAULT_ALPHA: f32 = 1.0;

/// Whether a theme that says nothing about line numbers shows them. Off, so
/// a document looks exactly the way it did before the setting existed - and
/// because the numbers are a code editor's habit, not a notepad's.
pub const DEFAULT_LINE_NUMBERS_SHOWN: bool = false;

/// How much of the document's text color the line numbers are drawn at when
/// no theme names an alpha: enough to read, far enough back that the eye
/// goes to the text first.
pub const DEFAULT_LINE_NUMBERS_ALPHA: f32 = 0.45;
