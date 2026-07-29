mod defaults;
mod keybind_overrides;

use std::collections::HashMap;
use std::path::PathBuf;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use serde::{Deserialize, Serialize};

pub use keybind_overrides::ResolvedKeybind;

/// xizor's user-editable settings. Each concern gets its own section so a
/// missing section falls back to its own defaults rather than failing the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub syntaxes: SyntaxesConfig,
    /// The display name of an `iced::Theme` variant (e.g. `"Dracula"`),
    /// matched case-insensitively - kept as a plain string so this crate
    /// doesn't need to depend on `iced`.
    pub theme: String,
    pub visor: VisorConfig,
    pub alpha: AlphaConfig,
}

impl Default for Config {
    fn default() -> Self {
        defaults::config()
    }
}

/// Controls whether xizor runs as a drop-down "visor" (undecorated,
/// always-on-top, hidden until summoned) or as an ordinary window.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VisorConfig {
    pub enabled: bool,
}

/// Independent transparency for the editor's background versus its text -
/// `1.0` is fully solid, `0.0` fully invisible. Clamped where applied, not
/// here, so this crate doesn't need an `iced` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlphaConfig {
    pub background: f32,
    pub foreground: f32,
}

impl Default for AlphaConfig {
    fn default() -> Self {
        Self {
            background: 1.0,
            foreground: 1.0,
        }
    }
}

/// xizor's global keybindings, loaded from `keybinds.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindsConfig {
    /// Shows/hides the visor from anywhere, even without focus.
    pub toggle: HotKey,
    /// Command-name -> key-chord overrides for xizor's in-app shortcuts,
    /// e.g. `new_tab = "control+alt+n"` - takes precedence over the
    /// hardcoded default when present. An unrecognized name is silently
    /// ignored (logged once at startup).
    #[serde(default)]
    pub overrides: HashMap<String, HotKey>,
}

impl Default for KeybindsConfig {
    fn default() -> Self {
        Self {
            toggle: HotKey::new(Some(Modifiers::CONTROL), Code::Backquote),
            overrides: HashMap::new(),
        }
    }
}

impl KeybindsConfig {
    /// Resolves `overrides` into iced-native types, ready to compare against incoming key events.
    pub fn resolved_overrides(&self) -> HashMap<String, ResolvedKeybind> {
        keybind_overrides::resolved_overrides(&self.overrides)
    }
}

/// Maps a grammar's name (the `<name>.wasm` file to look for) to the file
/// extensions that should use it, e.g. `"yaml" -> ["yaml", "yml"]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SyntaxesConfig(pub HashMap<String, Vec<String>>);

impl SyntaxesConfig {
    /// Inverts the grammar -> extensions mapping into extension -> grammar.
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

/// Where `config.toml` is looked for: next to the running executable, then
/// `./config.toml` (a `cargo run` convenience).
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

/// Loads the config from disk, writing a default file on first run. Never
/// fails the caller - a broken config falls back to in-memory defaults.
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

/// Where `keybinds.toml` is looked for - same search order as `config_paths()`.
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

/// Loads `keybinds.toml` from disk, writing a default file on first run.
/// Mirrors `load()`'s never-fail behavior.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybinds_toml_with_no_overrides_table_parses_as_empty() {
        let keybinds: KeybindsConfig = toml::from_str(r#"toggle = "control+Backquote""#).unwrap();
        assert!(keybinds.overrides.is_empty());
    }

    #[test]
    fn keybinds_toml_with_overrides_table_parses_and_resolves() {
        let keybinds: KeybindsConfig = toml::from_str(
            r#"
            toggle = "control+Backquote"

            [overrides]
            new_tab = "control+alt+n"
            undo = "control+z"
            "#,
        )
        .unwrap();
        assert_eq!(keybinds.overrides.len(), 2);

        let resolved = keybinds.resolved_overrides();
        assert_eq!(
            resolved.get("new_tab"),
            Some(&ResolvedKeybind {
                modifiers: iced_core::keyboard::Modifiers::CTRL | iced_core::keyboard::Modifiers::ALT,
                code: iced_core::keyboard::key::Code::KeyN,
            })
        );
        assert_eq!(
            resolved.get("undo"),
            Some(&ResolvedKeybind {
                modifiers: iced_core::keyboard::Modifiers::CTRL,
                code: iced_core::keyboard::key::Code::KeyZ,
            })
        );
    }

    #[test]
    fn a_malformed_chord_string_fails_the_whole_file_not_just_that_entry() {
        let result: Result<KeybindsConfig, _> = toml::from_str(
            r#"
            toggle = "control+Backquote"

            [overrides]
            new_tab = "not a real chord"
            "#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn default_keybinds_config_has_no_overrides() {
        assert!(KeybindsConfig::default().overrides.is_empty());
    }

    #[test]
    fn default_alpha_is_fully_solid() {
        assert_eq!(AlphaConfig::default(), AlphaConfig { background: 1.0, foreground: 1.0 });
    }

    #[test]
    fn config_toml_with_no_alpha_section_falls_back_to_solid() {
        let config: Config = toml::from_str(r#"theme = "Light""#).unwrap();
        assert_eq!(config.alpha, AlphaConfig::default());
    }

    #[test]
    fn config_toml_with_an_alpha_section_parses() {
        let config: Config = toml::from_str(
            r#"
            theme = "Light"

            [alpha]
            background = 0.7
            foreground = 0.9
            "#,
        )
        .unwrap();
        assert_eq!(config.alpha, AlphaConfig { background: 0.7, foreground: 0.9 });
    }

    #[test]
    fn alpha_section_with_only_one_field_defaults_the_other() {
        let config: Config = toml::from_str(
            r#"
            theme = "Light"

            [alpha]
            background = 0.5
            "#,
        )
        .unwrap();
        assert_eq!(config.alpha, AlphaConfig { background: 0.5, foreground: 1.0 });
    }
}
