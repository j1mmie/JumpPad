mod bundle;
mod defaults;
mod editor;
mod font;
mod gpu;
mod indentation;
mod keybind_overrides;
mod keybinds;
mod language;
mod languages;
mod loader;
mod theme;
mod window;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use editor::{
    EditorConfig, FilesConfig, HistoryConfig, LineNumbersConfig,
    SaveConflictResolution, ScrollConfig, UiConfig,
};
pub use font::{
    FontConfig, DEFAULT_ALPHA, DEFAULT_FONT_SIZE, DEFAULT_LINE_NUMBERS_ALPHA,
    DEFAULT_LINE_NUMBERS_SHOWN,
};
pub use gpu::{GpuConfig, GpuPower};
pub use indentation::{
    IndentationConfig, IndentationStyle, WordsConfig,
    DEFAULT_WORD_SEPARATORS,
};
pub use bundle::{discover as discover_bundles, SyntaxBundle};
pub use keybind_overrides::ResolvedKeybind;
pub use keybinds::KeybindsConfig;
pub use language::{CommentSyntax, LanguageConfig};
pub use languages::{Languages, ResolvedLanguage};
pub use loader::{
    candidate_dirs, config_file, keybinds_file, load, load_keybinds,
    try_load, try_load_keybinds, ReloadError,
};
pub use theme::{
    Appearance, Detection, ModeConfig, ResolvedFont, ResolvedLineNumbers,
    ResolvedTheme, ThemeConfig, ThemeSlots,
};
pub use window::{
    BackgroundConfig, Blur, ForegroundConfig, VisorConfig, WindowConfig,
    DEFAULT_BLUR,
};

// Not part of any one domain's public API, but needed here (`Config` itself)
// and by the sibling test module (`use super::*` in lib_tests.rs) - a plain
// `use` rather than `pub use` keeps them crate-internal.
#[cfg(test)]
use std::path::PathBuf;

#[cfg(test)]
use loader::try_parse;
use theme::BASE_THEME;

/// JumpPad's user-editable settings. Each concern gets its own section so a
/// missing section falls back to its own defaults rather than failing the file.
///
/// A section still sitting on its defaults is left out of the file written on
/// first run, so that file shows what actually differs and nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip_serializing_if = "is_default")]
    pub mode: ModeConfig,
    /// The themes `[mode]` can choose between, by name, alongside
    /// `[themes.base]` - the defaults the rest of them inherit. A `BTreeMap`
    /// rather than a `HashMap` because this one is written back out as the
    /// default file, and a stable key order keeps that file from reshuffling
    /// itself.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub themes: BTreeMap<String, ThemeConfig>,
    #[serde(skip_serializing_if = "is_default")]
    pub visor: VisorConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub window: WindowConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub gpu: GpuConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub scroll: ScrollConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub history: HistoryConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub files: FilesConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub indentation: IndentationConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub words: WordsConfig,
    /// `[[languages]]` entries, each one a patch over the bundle of the
    /// same name under `syntaxes/` - see [`Languages::resolve`]. Last so
    /// the array-of-tables lands at the end of a written file, and
    /// skipped when empty because the bundles are the defaults now.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<LanguageConfig>,
}

/// A section already sitting on its default says nothing the app doesn't do
/// anyway, which is why the file written on first run leaves it out.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

impl Config {
    /// The theme to draw with in the given appearance: the slot's theme, over
    /// `[themes.base]`, over JumpPad's own defaults.
    pub fn theme_for(&self, showing: Appearance) -> ResolvedTheme {
        let name = match showing {
            Appearance::Light => &self.mode.theme.light,
            Appearance::Dark => &self.mode.theme.dark,
        };

        self.theme_named(name)
            .with_defaults_from(self.themes.get(BASE_THEME))
            .resolved(showing.default_palette())
    }

    /// The theme a slot's name selects.
    ///
    /// A name with no `[themes]` entry behind it is read as a theme naming
    /// that palette and nothing else, so choosing colors doesn't require
    /// defining a whole theme - and so the slot's palette outranks the base
    /// theme's the same way any other named property does. Themes are looked
    /// up first, so one named after a palette wins its own name.
    fn theme_named(&self, name: &str) -> ThemeConfig {
        match self.themes.get(name) {
            Some(theme) => theme.clone(),
            None => ThemeConfig {
                palette: Some(name.to_string()),
                ..ThemeConfig::default()
            },
        }
    }

    /// Whether any theme this file can show asks for a translucent window.
    ///
    /// Asked once, at startup: transparency is fixed when the window is
    /// created, so a window that any theme might want to see through has to
    /// be born that way for every theme to apply without a restart.
    ///
    /// The base theme is not merged in first, and doesn't need to be: it is
    /// itself one of these entries, so a translucency only it names is
    /// already counted, and every other theme's alpha is either its own or
    /// the one base already contributed.
    pub fn wants_transparency(&self) -> bool {
        self.themes.values().any(|theme| {
            theme.background.alpha.is_some_and(|alpha| alpha < 1.0)
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        defaults::config()
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
