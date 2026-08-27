use std::collections::BTreeMap;

use crate::{
    Config, FilesConfig, GpuConfig, HistoryConfig, IndentationConfig,
    ModeConfig, ScrollConfig, VisorConfig, WindowConfig, WordsConfig,
};

/// Built-in defaults, written out as the config file on first run.
///
/// No languages: those ship as `syntaxes/<grammar>/config.toml` bundles, so
/// a fresh config file is silent about them and a `[[languages]]` entry in
/// one means "change this about the bundle", not "here is the whole list".
pub(crate) fn config() -> Config {
    Config {
        // No `[themes]` of its own: the slots default to "light" and
        // "dark", which resolve as palettes without a theme behind them.
        mode: ModeConfig::default(),
        themes: BTreeMap::new(),
        visor: VisorConfig::default(),
        window: WindowConfig::default(),
        gpu: GpuConfig::default(),
        scroll: ScrollConfig::default(),
        history: HistoryConfig::default(),
        files: FilesConfig::default(),
        indentation: IndentationConfig::default(),
        words: WordsConfig::default(),
        languages: Vec::new(),
    }
}
