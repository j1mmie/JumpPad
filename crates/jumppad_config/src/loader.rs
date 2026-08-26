use std::path::PathBuf;

use crate::{Config, KeybindsConfig};

/// Where `config.toml` is looked for: next to the running executable, then
/// `./config.toml` (a `cargo run` convenience).
fn config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        paths.push(dir.join("config.toml"));
    }
    paths.push(PathBuf::from("config.toml"));
    paths
}

/// The file a reload would read: the first existing candidate, in the same
/// order `load()` searches. `None` if no config file exists yet.
pub fn config_file() -> Option<PathBuf> {
    config_paths().into_iter().find(|path| path.is_file())
}

/// The directories a config file can live in, deduped, in search order -
/// what a file watcher should watch. Canonicalized so the exe's directory
/// and the cwd collapse into one entry when they're the same place.
pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for path in config_paths() {
        let dir = match path.parent() {
            // The cwd candidate is the bare relative `config.toml`, whose
            // parent is the empty path.
            Some(dir) if dir.as_os_str().is_empty() => PathBuf::from("."),
            Some(dir) => dir.to_path_buf(),
            None => continue,
        };
        let Ok(dir) = std::fs::canonicalize(&dir) else {
            continue;
        };
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Same, for `keybinds.toml`.
pub fn keybinds_file() -> Option<PathBuf> {
    keybind_paths().into_iter().find(|path| path.is_file())
}

/// Why a reload attempt produced nothing.
#[derive(Debug)]
pub enum ReloadError {
    /// No candidate file exists (deleted since the last load).
    Missing,
    Io(std::io::ErrorKind),
    Parse(String),
}

impl std::fmt::Display for ReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReloadError::Missing => write!(f, "file not found"),
            ReloadError::Io(kind) => write!(f, "{kind}"),
            ReloadError::Parse(message) => write!(f, "{message}"),
        }
    }
}

/// Fallible sibling of [`load`]: never writes a default file and never
/// falls back to `Default`, so a half-edited file on disk keeps the
/// caller's last good config live instead of resetting it.
pub fn try_load() -> Result<Config, ReloadError> {
    try_parse(&config_paths())
}

/// Fallible sibling of [`load_keybinds`] - see [`try_load`].
pub fn try_load_keybinds() -> Result<KeybindsConfig, ReloadError> {
    try_parse(&keybind_paths())
}

pub(crate) fn try_parse<T: serde::de::DeserializeOwned>(
    paths: &[PathBuf],
) -> Result<T, ReloadError> {
    for path in paths {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(ReloadError::Io(err.kind())),
        };
        return toml::from_str(&text)
            .map_err(|err| ReloadError::Parse(err.to_string()));
    }
    Err(ReloadError::Missing)
}

/// Loads the config from disk, writing a default file on first run. Never
/// fails the caller - a broken config falls back to in-memory defaults.
pub fn load() -> Config {
    let paths = config_paths();

    for path in &paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        log::debug!("found config at {}", path.display());
        // The whole file at `trace`, not `debug`: which file was picked up
        // is the startup fact worth having every run, and a hundred lines
        // of its contents is a thing you go looking for.
        log::trace!("--- contents of {} ---", path.display());
        log::trace!("{text}");
        log::trace!("--- end contents ---");
        return match toml::from_str(&text) {
            Ok(config) => config,
            Err(err) => {
                log::warn!(
                    "{}: {err}, using built-in defaults instead",
                    path.display()
                );
                Config::default()
            }
        };
    }

    log::info!(
        "no config file found (checked: {}), writing built-in defaults",
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
        log::warn!("couldn't create {}: {err}", parent.display());
        return;
    }
    match toml::to_string_pretty(config) {
        Ok(text) => {
            if let Err(err) = std::fs::write(path, text) {
                log::warn!(
                    "couldn't write default config to {}: {err}",
                    path.display()
                );
            }
        }
        Err(err) => log::warn!("couldn't serialize default config: {err}"),
    }
}

/// Where `keybinds.toml` is looked for - same search order as `config_paths()`.
fn keybind_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        paths.push(dir.join("keybinds.toml"));
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
        log::debug!("found keybinds at {}", path.display());
        return match toml::from_str(&text) {
            Ok(keybinds) => keybinds,
            Err(err) => {
                log::warn!(
                    "{}: {err}, using built-in default keybinds instead",
                    path.display()
                );
                KeybindsConfig::default()
            }
        };
    }

    log::info!(
        "no keybinds file found (checked: {}), writing built-in defaults",
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
        log::warn!("couldn't create {}: {err}", parent.display());
        return;
    }
    match toml::to_string_pretty(keybinds) {
        Ok(text) => {
            if let Err(err) = std::fs::write(path, text) {
                log::warn!(
                    "couldn't write default keybinds to {}: {err}",
                    path.display()
                );
            }
        }
        Err(err) => log::warn!("couldn't serialize default keybinds: {err}"),
    }
}
