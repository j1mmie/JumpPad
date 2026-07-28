mod defaults;

use std::collections::HashMap;
use std::path::PathBuf;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use serde::{Deserialize, Serialize};

/// xizor's user-editable settings. Expected to grow (keybindings, theme,
/// editor behavior, ...) - each concern gets its own section so old config
/// files stay valid as new ones are added, and a missing section just
/// falls back to that section's defaults rather than failing the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub syntaxes: SyntaxesConfig,
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

/// xizor's global keybindings, loaded from `keybinds.toml` (separate from
/// `config.toml` - see `load_keybinds()`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindsConfig {
    /// Shows/hides the visor from anywhere, even while another application
    /// has focus. Parsed and validated by `global_hotkey::hotkey::HotKey`'s
    /// own `Deserialize` impl, so a combo like `"ctrl+alt+7"` or
    /// `"cmd+shift+down"` in the TOML file just works - no custom parsing
    /// needed here.
    pub toggle: HotKey,
}

impl Default for KeybindsConfig {
    fn default() -> Self {
        Self {
            toggle: HotKey::new(Some(Modifiers::CONTROL), Code::Backquote),
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

/// Where `keybinds.toml` is looked for - same search order as
/// `config_paths()` (next to the executable, then the current directory for
/// `cargo run`), just a different filename.
fn keybind_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("keybinds.toml"));
        }
    }
    paths.push(PathBuf::from("keybinds.toml"));
    paths
}

/// Loads `keybinds.toml` from disk, writing out a default file (next to the
/// executable) on first run. Mirrors `load()`'s never-fail behavior: an
/// unreadable file, malformed TOML, or an unparseable hotkey spec (e.g. a
/// typo'd key name) all fall back to the built-in default (`Ctrl+\``) rather
/// than stopping the editor from starting.
pub fn load_keybinds() -> KeybindsConfig {
    let paths = keybind_paths();

    for path in &paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        eprintln!("xizor_config: found keybinds at {}", path.display());
        return match toml::from_str(&text) {
            Ok(keybinds) => keybinds,
            Err(err) => {
                eprintln!(
                    "xizor_config: {}: {err}, using built-in default keybinds instead",
                    path.display()
                );
                KeybindsConfig::default()
            }
        };
    }

    eprintln!(
        "xizor_config: no keybinds file found (checked: {}), writing built-in defaults",
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let keybinds = KeybindsConfig::default();
    if let Some(path) = paths.first() {
        write_default_keybinds(path, &keybinds);
    }
    keybinds
}

fn write_default_keybinds(path: &std::path::Path, keybinds: &KeybindsConfig) {
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(err) = std::fs::create_dir_all(parent) {
        eprintln!("xizor_config: couldn't create {}: {err}", parent.display());
        return;
    }
    match toml::to_string_pretty(keybinds) {
        Ok(text) => {
            if let Err(err) = std::fs::write(path, text) {
                eprintln!(
                    "xizor_config: couldn't write default keybinds to {}: {err}",
                    path.display()
                );
            }
        }
        Err(err) => eprintln!("xizor_config: couldn't serialize default keybinds: {err}"),
    }
}
