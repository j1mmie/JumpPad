use serde::{Deserialize, Serialize};

use crate::font::{
    DEFAULT_LINE_NUMBERS_ALPHA, DEFAULT_LINE_NUMBERS_GAP,
    DEFAULT_LINE_NUMBERS_MIN_WIDTH, DEFAULT_LINE_NUMBERS_SHOWN, FontConfig,
};
use crate::theme::ResolvedLineNumbers;

/// A theme's half for the documents themselves.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub font: FontConfig,
    pub line_numbers: LineNumbersConfig,
}

/// The column of line numbers down the left of a document.
///
/// Off unless a theme asks for it: JumpPad opens notes and config files as
/// often as it opens code, and a number beside every line is noise there.
///
/// `alpha` scales the theme's own text color rather than naming a color of
/// its own, so the numbers read as a step back from the document in every
/// palette - and stay in step with a theme the user swaps underneath them.
///
/// `min_width` and `gap` are in **ems** - multiples of the document's text
/// size - so they keep their proportions at any size and mean the same thing
/// in any typeface. `min_width` is the narrowest the numbers themselves are
/// drawn, and `gap` is added on top of it, so widening the gap never eats
/// into the numbers.
///
/// Every property is matched and clamped where applied, not here, so this
/// crate doesn't need an `iced` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LineNumbersConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f32>,
}

impl LineNumbersConfig {
    /// These line numbers with `base` supplying whichever property they
    /// don't name - so a base alpha survives a theme that only turns the
    /// numbers off.
    pub(crate) fn with_defaults_from(
        &self,
        base: &LineNumbersConfig,
    ) -> LineNumbersConfig {
        LineNumbersConfig {
            enabled: self.enabled.or(base.enabled),
            alpha: self.alpha.or(base.alpha),
            min_width: self.min_width.or(base.min_width),
            gap: self.gap.or(base.gap),
        }
    }

    pub(crate) fn resolved(&self) -> ResolvedLineNumbers {
        ResolvedLineNumbers {
            enabled: self.enabled.unwrap_or(DEFAULT_LINE_NUMBERS_SHOWN),
            alpha: self.alpha.unwrap_or(DEFAULT_LINE_NUMBERS_ALPHA),
            min_width: self.min_width.unwrap_or(DEFAULT_LINE_NUMBERS_MIN_WIDTH),
            gap: self.gap.unwrap_or(DEFAULT_LINE_NUMBERS_GAP),
        }
    }
}

/// A theme's half for the app's own chrome around them - tab titles, the
/// find palette, dialogs.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub font: FontConfig,
}

/// Scrolling speed, as plain multipliers: `1.0` is the shipped speed, `2.0`
/// twice as fast, `0.5` half. `sensitivity` is the distance per unit of wheel
/// or trackpad input; `drag_speed` is how fast the view moves while a
/// selection is dragged past the top or bottom edge. Both are clamped where
/// applied, not here, so this crate doesn't need an `iced` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollConfig {
    pub sensitivity: f32,
    pub drag_speed: f32,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            sensitivity: 1.0,
            drag_speed: 1.0,
        }
    }
}

/// Undo history. A step is a burst of typing, not a keystroke. Clamped
/// where applied, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    pub depth: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self { depth: 200 }
    }
}

/// How JumpPad treats the files it has open.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesConfig {
    pub save_conflict_resolution: SaveConflictResolution,
}

/// What a save does when the file changed on disk since JumpPad last read
/// it. Mirrors VS Code's `files.saveConflictResolution`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum SaveConflictResolution {
    /// Prompt before overwriting someone else's changes.
    #[default]
    Ask,
    /// Saves always win, no prompt.
    Overwrite,
}

impl SaveConflictResolution {
    pub fn asks(self) -> bool {
        matches!(self, Self::Ask)
    }
}
