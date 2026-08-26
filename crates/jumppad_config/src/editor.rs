use serde::{Deserialize, Serialize};

use crate::font::FontConfig;

/// A theme's half for the documents themselves.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub font: FontConfig,
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
