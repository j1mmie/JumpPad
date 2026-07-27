mod defaults;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// xizor's user-editable settings. Expected to grow (keybindings, theme,
/// editor behavior, ...) - each concern gets its own section so old config
/// files stay valid as new ones are added, and a missing section just
/// falls back to that section's defaults rather than failing the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub syntaxes: SyntaxesConfig,
    pub renderer: RendererConfig,
    /// The display name of an `iced::Theme` variant (e.g. `"Dracula"`,
    /// `"Solarized Light"`, matched case-insensitively) - kept as a plain
    /// string rather than an enum here so this crate doesn't need to depend
    /// on `iced` or track its theme list. Resolved against the real
    /// `iced::Theme::ALL` in the `xizor` crate, where `iced` is already a
    /// dependency; an unrecognized name falls back to the default theme.
    pub theme: String,
}

impl Default for Config {
    fn default() -> Self {
        defaults::config()
    }
}

/// Which iced compositor backend to render with. Both are compiled in
/// (`iced`'s `wgpu` and `tiny-skia` features are both enabled) so this can
/// be a runtime choice - the tradeoff is that keeping both means the binary
/// still carries wgpu's size cost even when `tiny-skia` is selected.
///
/// `tiny-skia` is the default: it's a pure-software rasterizer with a
/// fraction of wgpu's idle memory footprint (measured ~22MB vs. ~146MB for
/// this app), and a text editor has no real need for GPU-accelerated
/// rendering. `Wgpu`/`Auto` are there for anyone who wants GPU acceleration
/// back without recompiling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RendererConfig {
    /// Let iced pick: tries `wgpu` first, only falls back to `tiny-skia` if
    /// `wgpu` fails to initialize (e.g. no usable graphics adapter).
    Auto,
    Wgpu,
    #[default]
    TinySkia,
}

impl RendererConfig {
    /// The value iced's `ICED_BACKEND` environment variable expects, or
    /// `None` for `Auto` (leaving the variable unset, which is iced's own
    /// try-wgpu-then-fall-back behavior).
    pub fn iced_backend_env_value(self) -> Option<&'static str> {
        match self {
            RendererConfig::Auto => None,
            RendererConfig::Wgpu => Some("wgpu"),
            RendererConfig::TinySkia => Some("tiny-skia"),
        }
    }
}

/// Maps a grammar's name (the `<name>.wasm` file to look for) to the file
/// extensions that should use it, e.g. `"yaml" -> ["yaml", "yml"]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SyntaxesConfig(pub HashMap<String, Vec<String>>);

impl SyntaxesConfig {
    /// Inverts the grammar -> extensions mapping into extension -> grammar,
    /// which is what actually looking up "what grammar does this open file
    /// need" wants. If the same extension is listed under more than one
    /// grammar, the last one encountered (arbitrary map iteration order)
    /// wins - duplicate mappings aren't supported yet.
    pub fn extension_to_grammar(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for (grammar, extensions) in &self.0 {
            for extension in extensions {
                map.insert(extension.clone(), grammar.clone());
            }
        }
        map
    }
}

/// Where `config.toml` is looked for, in order: next to the running
/// executable (the "real" location - works the same for a dev build and a
/// shipped one), then a `config.toml` relative to the current directory as
/// a convenience for `cargo run` (where the exe lives buried in
/// `target/debug/`, far from the workspace root).
fn config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("config.toml"));
        }
    }
    paths.push(PathBuf::from("config.toml"));
    paths
}

/// Loads the config from disk, writing out a default file (next to the
/// executable) on first run. Never fails the caller - an unreadable file
/// or malformed TOML falls back to in-memory defaults (and gets logged),
/// since a broken config file should never stop the editor from starting.
pub fn load() -> Config {
    let paths = config_paths();

    for path in &paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        eprintln!("xizor_config: found config at {}", path.display());
        eprintln!("xizor_config: --- contents of {} ---", path.display());
        eprintln!("{text}");
        eprintln!("xizor_config: --- end contents ---");
        return match toml::from_str(&text) {
            Ok(config) => config,
            Err(err) => {
                eprintln!(
                    "xizor_config: {}: {err}, using built-in defaults instead",
                    path.display()
                );
                Config::default()
            }
        };
    }

    eprintln!(
        "xizor_config: no config file found (checked: {}), writing built-in defaults",
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let config = Config::default();
    if let Some(path) = paths.first() {
        write_default(path, &config);
    }
    config
}

fn write_default(path: &std::path::Path, config: &Config) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(err) = std::fs::create_dir_all(parent) {
        eprintln!("xizor_config: couldn't create {}: {err}", parent.display());
        return;
    }
    match toml::to_string_pretty(config) {
        Ok(text) => {
            if let Err(err) = std::fs::write(path, text) {
                eprintln!(
                    "xizor_config: couldn't write default config to {}: {err}",
                    path.display()
                );
            }
        }
        Err(err) => eprintln!("xizor_config: couldn't serialize default config: {err}"),
    }
}
