mod defaults;
mod keybind_overrides;

use std::collections::HashMap;
use std::path::PathBuf;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use serde::{Deserialize, Serialize};

pub use keybind_overrides::ResolvedKeybind;

/// JumpPad's user-editable settings. Each concern gets its own section so a
/// missing section falls back to its own defaults rather than failing the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The display name of an `iced::Theme` variant (e.g. `"Dracula"`),
    /// matched case-insensitively - kept as a plain string so this crate
    /// doesn't need to depend on `iced`.
    pub theme: String,
    pub visor: VisorConfig,
    pub alpha: AlphaConfig,
    pub window: WindowConfig,
    pub scroll: ScrollConfig,
    pub files: FilesConfig,
    /// `[[languages]]` entries; last so the array-of-tables lands at the
    /// end of the written default file.
    pub languages: Vec<LanguageConfig>,
}

impl Config {
    /// Extension -> grammar name, for the syntax registry. An entry without
    /// a `syntax` contributes nothing; a later entry wins an extension.
    pub fn extension_to_grammar(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for language in &self.languages {
            let Some(syntax) = &language.syntax else {
                continue;
            };
            for extension in &language.extensions {
                map.insert(extension.clone(), syntax.clone());
            }
        }
        map
    }

    /// Extension (lowercased) -> comment style, for toggle-comment; a later
    /// entry wins an extension.
    pub fn comment_styles_by_extension(
        &self,
    ) -> HashMap<String, CommentSyntax> {
        let mut map = HashMap::new();
        for language in &self.languages {
            let Some(comment) = &language.comment else {
                continue;
            };
            for extension in &language.extensions {
                map.insert(extension.to_lowercase(), comment.clone());
            }
        }
        map
    }
}

/// One `[[languages]]` entry: file extensions plus an optional grammar and
/// an optional toggle-comment style. `name` is for the file's readability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageConfig {
    pub name: String,
    /// The `<syntax>.wasm` grammar these extensions highlight with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax: Option<String>,
    pub extensions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<CommentSyntax>,
}

/// A language's comment syntax: exactly one of `comment.single` or
/// `comment.multi` - defining both fails the whole file's parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCommentSyntax", into = "RawCommentSyntax")]
pub enum CommentSyntax {
    Single(String),
    Multi { left: String, right: String },
}

/// The TOML-facing shape `CommentSyntax` validates from.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommentSyntax {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    single: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    multi: Option<RawMultiComment>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMultiComment {
    left: String,
    right: String,
}

impl TryFrom<RawCommentSyntax> for CommentSyntax {
    type Error = String;

    fn try_from(raw: RawCommentSyntax) -> Result<Self, String> {
        match (raw.single, raw.multi) {
            (Some(prefix), None) => Ok(Self::Single(prefix)),
            (None, Some(multi)) => Ok(Self::Multi { left: multi.left, right: multi.right }),
            (Some(_), Some(_)) => Err(
                "comment.single and comment.multi are mutually exclusive - keep exactly one"
                    .to_string(),
            ),
            (None, None) => Err(
                "comment must set comment.single or comment.multi (or be removed)".to_string(),
            ),
        }
    }
}

impl From<CommentSyntax> for RawCommentSyntax {
    fn from(comment: CommentSyntax) -> Self {
        match comment {
            CommentSyntax::Single(prefix) => Self {
                single: Some(prefix),
                multi: None,
            },
            CommentSyntax::Multi { left, right } => Self {
                single: None,
                multi: Some(RawMultiComment { left, right }),
            },
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        defaults::config()
    }
}

/// Controls whether JumpPad runs as a drop-down "visor" (undecorated,
/// always-on-top, hidden until summoned) or as an ordinary window.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VisorConfig {
    pub enabled: bool,
}

/// Window frame options. Visor mode overrides `decorations` - a drop-down
/// visor is undecorated by definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    /// The OS titlebar and frame.
    pub decorations: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { decorations: true }
    }
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

/// Mouse-wheel and trackpad scrolling. `sensitivity` is a plain multiplier
/// on the distance JumpPad scrolls per unit of wheel input: `1.0` is the
/// shipped speed, `2.0` twice as far, `0.5` half. Clamped where applied,
/// not here, so this crate doesn't need an `iced` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollConfig {
    pub sensitivity: f32,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self { sensitivity: 1.0 }
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

/// JumpPad's global keybindings, loaded from `keybinds.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindsConfig {
    /// Shows/hides the visor from anywhere, even without focus.
    pub toggle: HotKey,
    /// Command-name -> key-chord overrides for JumpPad's in-app shortcuts,
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

fn try_parse<T: serde::de::DeserializeOwned>(
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
        eprintln!("jumppad_config: found config at {}", path.display());
        eprintln!("jumppad_config: --- contents of {} ---", path.display());
        eprintln!("{text}");
        eprintln!("jumppad_config: --- end contents ---");
        return match toml::from_str(&text) {
            Ok(config) => config,
            Err(err) => {
                eprintln!(
                    "jumppad_config: {}: {err}, using built-in defaults instead",
                    path.display()
                );
                Config::default()
            }
        };
    }

    eprintln!(
        "jumppad_config: no config file found (checked: {}), writing built-in defaults",
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
        eprintln!(
            "jumppad_config: couldn't create {}: {err}",
            parent.display()
        );
        return;
    }
    match toml::to_string_pretty(config) {
        Ok(text) => {
            if let Err(err) = std::fs::write(path, text) {
                eprintln!(
                    "jumppad_config: couldn't write default config to {}: {err}",
                    path.display()
                );
            }
        }
        Err(err) => eprintln!(
            "jumppad_config: couldn't serialize default config: {err}"
        ),
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
        eprintln!("jumppad_config: found keybinds at {}", path.display());
        return match toml::from_str(&text) {
            Ok(keybinds) => keybinds,
            Err(err) => {
                eprintln!(
                    "jumppad_config: {}: {err}, using built-in default keybinds instead",
                    path.display()
                );
                KeybindsConfig::default()
            }
        };
    }

    eprintln!(
        "jumppad_config: no keybinds file found (checked: {}), writing built-in defaults",
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
        eprintln!(
            "jumppad_config: couldn't create {}: {err}",
            parent.display()
        );
        return;
    }
    match toml::to_string_pretty(keybinds) {
        Ok(text) => {
            if let Err(err) = std::fs::write(path, text) {
                eprintln!(
                    "jumppad_config: couldn't write default keybinds to {}: {err}",
                    path.display()
                );
            }
        }
        Err(err) => eprintln!(
            "jumppad_config: couldn't serialize default keybinds: {err}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybinds_toml_with_no_overrides_table_parses_as_empty() {
        let keybinds: KeybindsConfig =
            toml::from_str(r#"toggle = "control+Backquote""#).unwrap();
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
                modifiers: iced_core::keyboard::Modifiers::CTRL
                    | iced_core::keyboard::Modifiers::ALT,
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

    /// A throwaway file that cleans up after itself, so the `try_parse`
    /// tests can exercise the real read path.
    struct TempFile(PathBuf);

    impl TempFile {
        fn with_contents(name: &str, contents: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("jumppad_config_test_{name}"));
            std::fs::write(&path, contents).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn try_parse_reads_the_first_existing_candidate() {
        let file =
            TempFile::with_contents("valid.toml", r#"theme = "Dracula""#);
        let missing = PathBuf::from("does_not_exist/config.toml");
        let config: Config = try_parse(&[missing, file.0.clone()]).unwrap();
        assert_eq!(config.theme, "Dracula");
    }

    #[test]
    fn try_parse_with_no_existing_candidate_is_missing() {
        let result: Result<Config, _> =
            try_parse(&[PathBuf::from("does_not_exist/config.toml")]);
        assert!(matches!(result, Err(ReloadError::Missing)));
    }

    /// The deliberate contrast with `load()`: a broken file is an error the
    /// caller can react to, not a silent reset to defaults.
    #[test]
    fn try_parse_surfaces_a_parse_error_instead_of_defaulting() {
        let file = TempFile::with_contents("broken.toml", "theme = ");
        let result: Result<Config, _> =
            try_parse(std::slice::from_ref(&file.0));
        assert!(matches!(result, Err(ReloadError::Parse(_))));
    }

    #[test]
    fn try_parse_surfaces_a_malformed_chord_as_a_parse_error() {
        let file = TempFile::with_contents(
            "bad_chord.toml",
            r#"
            toggle = "control+Backquote"

            [overrides]
            new_tab = "not a real chord"
            "#,
        );
        let result: Result<KeybindsConfig, _> =
            try_parse(std::slice::from_ref(&file.0));
        assert!(matches!(result, Err(ReloadError::Parse(_))));
    }

    #[test]
    fn default_alpha_is_fully_solid() {
        assert_eq!(
            AlphaConfig::default(),
            AlphaConfig {
                background: 1.0,
                foreground: 1.0
            }
        );
    }

    #[test]
    fn config_toml_with_no_window_section_keeps_decorations() {
        // Old config files predate the section and must stay valid.
        let config: Config = toml::from_str(r#"theme = "Light""#).unwrap();
        assert_eq!(config.window, WindowConfig::default());
        assert!(config.window.decorations);
    }

    #[test]
    fn config_toml_can_turn_decorations_off() {
        let config: Config = toml::from_str(
            r#"
            theme = "Light"

            [window]
            decorations = false
            "#,
        )
        .unwrap();
        assert!(!config.window.decorations);
    }

    #[test]
    fn config_toml_with_no_languages_keeps_the_builtin_defaults() {
        let config: Config = toml::from_str(r#"theme = "Light""#).unwrap();
        let styles = config.comment_styles_by_extension();
        assert_eq!(
            styles.get("yaml"),
            Some(&CommentSyntax::Single("# ".to_string()))
        );
        assert_eq!(
            styles.get("yml"),
            Some(&CommentSyntax::Single("# ".to_string()))
        );
        assert_eq!(
            styles.get("html"),
            Some(&CommentSyntax::Multi {
                left: "<!--".to_string(),
                right: "-->".to_string()
            })
        );
        assert_eq!(
            config.extension_to_grammar().get("yml").map(String::as_str),
            Some("yaml")
        );
    }

    #[test]
    fn a_languages_section_replaces_the_defaults_wholesale() {
        let config: Config = toml::from_str(
            r#"
            theme = "Light"

            [[languages]]
            name = "TOML"
            syntax = "toml"
            extensions = ["toml"]
            comment.single = "// "
            "#,
        )
        .unwrap();
        let styles = config.comment_styles_by_extension();
        assert_eq!(
            styles.get("toml"),
            Some(&CommentSyntax::Single("// ".to_string()))
        );
        assert_eq!(styles.get("yaml"), None, "built-in defaults are gone");
        assert_eq!(config.extension_to_grammar().len(), 1);
    }

    #[test]
    fn comment_single_and_multi_together_fail_the_parse() {
        let result: Result<Config, _> = toml::from_str(
            r#"
            [[languages]]
            name = "Broken"
            extensions = ["x"]
            comment.single = "// "
            comment.multi.left = "<!--"
            comment.multi.right = "-->"
            "#,
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("mutually exclusive"), "got: {error}");
    }

    #[test]
    fn an_empty_comment_table_fails_the_parse() {
        let result: Result<Config, _> = toml::from_str(
            r#"
            [[languages]]
            name = "Broken"
            extensions = ["x"]

            [languages.comment]
            "#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn a_language_without_a_comment_key_parses_as_none() {
        let config: Config = toml::from_str(
            r#"
            [[languages]]
            name = "Plain"
            extensions = ["txt"]
            "#,
        )
        .unwrap();
        assert_eq!(config.languages[0].comment, None);
        assert_eq!(config.languages[0].syntax, None);
        assert!(config.comment_styles_by_extension().is_empty());
    }

    #[test]
    fn comment_multi_parses_the_dotted_key_form() {
        let config: Config = toml::from_str(
            r#"
            [[languages]]
            name = "HTML"
            syntax = "html"
            extensions = ["htm", "html", "xhtml"]
            comment.multi.left = "<!--"
            comment.multi.right = "-->"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.languages[0].comment,
            Some(CommentSyntax::Multi {
                left: "<!--".to_string(),
                right: "-->".to_string()
            })
        );
    }

    #[test]
    fn the_flatteners_lowercase_and_let_a_later_entry_win() {
        let config = Config {
            languages: vec![
                LanguageConfig {
                    name: "C++".to_string(),
                    syntax: Some("cpp".to_string()),
                    extensions: vec!["cpp".to_string(), "HPP".to_string()],
                    comment: Some(CommentSyntax::Single("// ".to_string())),
                },
                LanguageConfig {
                    name: "Rewrap".to_string(),
                    syntax: None,
                    extensions: vec!["cpp".to_string()],
                    comment: Some(CommentSyntax::Single("# ".to_string())),
                },
            ],
            ..Default::default()
        };
        let styles = config.comment_styles_by_extension();
        assert_eq!(
            styles.get("cpp"),
            Some(&CommentSyntax::Single("# ".to_string()))
        );
        assert_eq!(
            styles.get("hpp"),
            Some(&CommentSyntax::Single("// ".to_string()))
        );
        // The grammar map keeps configured casing and skips syntax-less entries.
        let grammars = config.extension_to_grammar();
        assert_eq!(grammars.get("cpp").map(String::as_str), Some("cpp"));
        assert_eq!(grammars.get("HPP").map(String::as_str), Some("cpp"));
        assert_eq!(grammars.len(), 2);
    }

    #[test]
    fn an_old_config_with_syntaxes_and_comment_styles_still_parses() {
        // Pre-[[languages]] sections are ignored unknowns: the file loads,
        // and those customizations fall back to the built-in defaults.
        let config: Config = toml::from_str(
            r#"
            theme = "Dracula"

            [syntaxes]
            toml = ["toml"]

            [[comment_styles]]
            syntaxes = ["toml"]
            prefix = "; "
            "#,
        )
        .unwrap();
        assert_eq!(config.theme, "Dracula");
        assert_eq!(
            config.comment_styles_by_extension().get("toml"),
            Some(&CommentSyntax::Single("# ".to_string()))
        );
    }

    /// Guards the first-run `write_default` path: the default config -
    /// array-of-tables included - must serialize and parse back unchanged.
    #[test]
    fn default_config_round_trips_through_toml() {
        let written = toml::to_string_pretty(&Config::default()).unwrap();
        let reparsed: Config = toml::from_str(&written).unwrap();
        assert_eq!(reparsed, Config::default());
    }

    #[test]
    fn the_sample_files_parse() {
        let config: Config =
            toml::from_str(include_str!("../../../config/config.sample.toml"))
                .unwrap();
        assert!(!config.languages.is_empty());
        let _: KeybindsConfig = toml::from_str(include_str!(
            "../../../config/keybinds.sample.toml"
        ))
        .unwrap();
    }

    #[test]
    fn sample_keybinds_document_every_action() {
        // The sample file lists the names a user may bind. Generated by hand,
        // checked by machine: an action added to the registry without being
        // documented fails here rather than being undiscoverable.
        let sample = include_str!("../../../config/keybinds.sample.toml");
        for action in jumppad_actions::Action::ALL {
            assert!(
                sample.contains(action.name()),
                "keybinds.sample.toml never mentions {action}"
            );
        }
    }

    #[test]
    fn every_documented_name_is_a_real_action() {
        // The other direction: a name that survived a rename would send
        // users chasing a binding that silently does nothing.
        let sample = include_str!("../../../config/keybinds.sample.toml");
        // The list runs from "Valid names:" to the bare `#` that closes the
        // block - past that is prose, where "toggle_comment's" is a sentence
        // rather than a name.
        let names = sample
            .lines()
            .skip_while(|line| !line.contains("Valid names:"))
            .take_while(|line| line.trim_end() != "#")
            .flat_map(|line| line.split([' ', ',', '#']))
            .filter(|word| word.contains('_'));
        for name in names {
            assert!(
                jumppad_actions::Action::from_name(name).is_some(),
                "keybinds.sample.toml documents {name:?}, which is not an action"
            );
        }
    }

    #[test]
    fn config_toml_with_no_scroll_section_falls_back_to_the_shipped_speed() {
        let config: Config = toml::from_str(r#"theme = "Light""#).unwrap();
        assert_eq!(config.scroll, ScrollConfig::default());
        assert_eq!(config.scroll.sensitivity, 1.0);
    }

    #[test]
    fn config_toml_with_a_scroll_section_parses() {
        let config: Config = toml::from_str(
            r#"
            theme = "Light"

            [scroll]
            sensitivity = 2.5
            "#,
        )
        .unwrap();
        assert_eq!(config.scroll.sensitivity, 2.5);
    }

    #[test]
    fn config_toml_with_no_files_section_asks_before_overwriting() {
        let config: Config = toml::from_str(r#"theme = "Light""#).unwrap();
        assert_eq!(config.files, FilesConfig::default());
        assert!(config.files.save_conflict_resolution.asks());
    }

    #[test]
    fn config_toml_can_ask_for_saves_that_always_win() {
        let config: Config = toml::from_str(
            r#"
            theme = "Light"

            [files]
            save_conflict_resolution = "overwrite"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.files.save_conflict_resolution,
            SaveConflictResolution::Overwrite
        );
        assert!(!config.files.save_conflict_resolution.asks());
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
        assert_eq!(
            config.alpha,
            AlphaConfig {
                background: 0.7,
                foreground: 0.9
            }
        );
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
        assert_eq!(
            config.alpha,
            AlphaConfig {
                background: 0.5,
                foreground: 1.0
            }
        );
    }
}
