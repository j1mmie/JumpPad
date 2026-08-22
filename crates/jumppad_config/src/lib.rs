mod defaults;
mod keybind_overrides;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use serde::{Deserialize, Serialize};

pub use keybind_overrides::ResolvedKeybind;

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
    pub scroll: ScrollConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub history: HistoryConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub files: FilesConfig,
    #[serde(skip_serializing_if = "is_default")]
    pub indentation: IndentationConfig,
    /// `[[languages]]` entries; last so the array-of-tables lands at the
    /// end of the written default file.
    pub languages: Vec<LanguageConfig>,
}

/// A section already sitting on its default says nothing the app doesn't do
/// anyway, which is why the file written on first run leaves it out.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
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

/// Which of the two theme slots is showing. Called an appearance rather than
/// a mode because `iced::window::Mode` is windowed/fullscreen/hidden, and the
/// app deals in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    /// The palette a slot falls back to when nothing names one - and the name
    /// an unconfigured slot carries, so a file of themes alone needs no
    /// `[mode]` section. Spelled to match `[themes.light]` and
    /// `[themes.dark]`; palettes are matched case-insensitively where they
    /// are applied, so one spelling serves both readings.
    pub fn default_palette(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// Which theme is showing, and whether the OS gets to decide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeConfig {
    #[serde(skip_serializing_if = "is_default")]
    pub detection: Detection,
    #[serde(skip_serializing_if = "is_default")]
    pub theme: ThemeSlots,
}

impl Default for ModeConfig {
    fn default() -> Self {
        Self {
            detection: Detection::Auto,
            theme: ThemeSlots::default(),
        }
    }
}

/// Which theme fills each of the two slots. A name here is a `[themes]`
/// entry - or, when no theme carries it, a palette to show that slot in.
///
/// Unnamed, the slots are `light` and `dark`, which a `[themes.light]` and a
/// `[themes.dark]` entry fill: a file that names its themes after the slots
/// they belong in needs no `[mode]` section at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSlots {
    pub light: String,
    pub dark: String,
}

impl Default for ThemeSlots {
    fn default() -> Self {
        Self {
            light: Appearance::Light.default_palette().to_string(),
            dark: Appearance::Dark.default_palette().to_string(),
        }
    }
}

impl ModeConfig {
    /// The slot to show. `Auto` follows the OS, and takes the light slot
    /// when the OS has no preference or couldn't be asked.
    pub fn showing(&self, os: Option<Appearance>) -> Appearance {
        match self.detection {
            Detection::Light => Appearance::Light,
            Detection::Dark => Appearance::Dark,
            Detection::Auto => os.unwrap_or(Appearance::Light),
        }
    }

    /// The slot this config holds the app to whatever the OS says, or `None`
    /// while it is following the OS.
    pub fn pinned(&self) -> Option<Appearance> {
        match self.detection {
            Detection::Light => Some(Appearance::Light),
            Detection::Dark => Some(Appearance::Dark),
            Detection::Auto => None,
        }
    }
}

/// Where the choice between the light and dark themes comes from.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Detection {
    /// Follow the OS, and keep following it while the app runs.
    #[default]
    Auto,
    /// Ignore the OS and stay on one slot.
    Light,
    Dark,
}

/// The theme every other theme takes its unnamed properties from. An ordinary
/// `[themes]` entry, so a slot can show it like any other.
const BASE_THEME: &str = "base";

/// One named theme: everything about JumpPad's appearance that can differ
/// between light and dark. A palette supplies the colors; the rest is what
/// JumpPad layers on top of them.
///
/// Every property is optional at every depth, and that is what makes
/// `[themes.base]` work: what a theme doesn't name comes from the base theme,
/// what the base theme doesn't name comes from JumpPad's own defaults, and
/// naming a property that happens to equal one of those defaults still counts
/// as naming it. See [`Config::theme_for`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// The display name of an `iced::Theme` variant (e.g. `"Dracula"`),
    /// matched case-insensitively where it's applied - kept as a plain
    /// string so this crate doesn't need to depend on `iced`. Unnamed means
    /// the base theme's palette, and failing that the slot's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<String>,
    pub background: BackgroundConfig,
    pub foreground: ForegroundConfig,
    pub editor: EditorConfig,
    pub ui: UiConfig,
}

impl ThemeConfig {
    /// This theme with `base` supplying every property it doesn't name,
    /// property by property rather than section by section. Still optional at
    /// every leaf: what neither theme names is settled by [`Self::resolved`].
    fn with_defaults_from(&self, base: Option<&ThemeConfig>) -> ThemeConfig {
        let Some(base) = base else {
            return self.clone();
        };

        ThemeConfig {
            palette: self.palette.clone().or_else(|| base.palette.clone()),
            background: BackgroundConfig {
                alpha: self.background.alpha.or(base.background.alpha),
                blur: self.background.blur.or(base.background.blur),
            },
            foreground: ForegroundConfig {
                alpha: self.foreground.alpha.or(base.foreground.alpha),
            },
            editor: EditorConfig {
                font: self.editor.font.with_defaults_from(&base.editor.font),
            },
            ui: UiConfig {
                font: self.ui.font.with_defaults_from(&base.ui.font),
            },
        }
    }

    /// Every property settled against JumpPad's own defaults, and an unnamed
    /// palette against `fallback_palette` - the palette of the slot this
    /// theme is being shown in.
    fn resolved(&self, fallback_palette: &str) -> ResolvedTheme {
        ResolvedTheme {
            palette: self
                .palette
                .clone()
                .unwrap_or_else(|| fallback_palette.to_string()),
            background_alpha: self.background.alpha.unwrap_or(DEFAULT_ALPHA),
            background_blur: self.background.blur.unwrap_or(DEFAULT_BLUR),
            foreground_alpha: self.foreground.alpha.unwrap_or(DEFAULT_ALPHA),
            editor_font: self.editor.font.resolved(),
            ui_font: self.ui.font.resolved(),
        }
    }
}

/// A [`ThemeConfig`] with every fallback already applied - what the app
/// actually draws with. Flat, because nothing here is optional any more: the
/// sections a theme is written in shape the config file, not the values that
/// come out of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTheme {
    pub palette: String,
    pub background_alpha: f32,
    pub background_blur: u32,
    pub foreground_alpha: f32,
    pub editor_font: ResolvedFont,
    pub ui_font: ResolvedFont,
}

/// One surface's typeface and text size, settled. `family` stays optional
/// because an unnamed family is an answer the app acts on - it draws with the
/// system's own face for that surface.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFont {
    pub family: Option<String>,
    pub size: f32,
}

impl Default for ResolvedFont {
    fn default() -> Self {
        Self {
            family: None,
            size: DEFAULT_FONT_SIZE,
        }
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

/// What a theme does to the surface behind the text. `alpha` is its
/// opacity: `1.0` fully solid, `0.0` fully invisible. Unnamed takes the base
/// theme's, and failing that [`DEFAULT_ALPHA`]. Its own section so anything
/// else about the background joins it here rather than beside the text's.
/// Clamped where applied, not here, so this crate doesn't need an `iced`
/// dependency.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f32>,
    /// How much the desktop showing through is frosted. `0` leaves it
    /// sharp; anything above that asks for a blur.
    ///
    /// A number rather than a flag because macOS takes it as a radius. Only
    /// one platform has that knob, though - Windows reads any positive
    /// value as the one Acrylic material it offers - so the amount is a
    /// request, the same as the blur itself is. The OS's compositor does
    /// all of it; see `windows.rs`/`macos.rs` for what each can deliver.
    ///
    /// Nothing to see at `alpha = 1.0`, where there is no desktop showing
    /// through to frost. Unnamed takes the base theme's, and failing that
    /// [`DEFAULT_BLUR`]. Capped where applied, not here, so this crate
    /// keeps holding no opinion the platform owns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<u32>,
}

/// The same, for the text drawn on that surface. Independent of it, so a
/// window can be see-through without its text fading with it.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ForegroundConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f32>,
}

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

/// What the Tab key inserts, and how wide a tab character is drawn. `width`
/// means columns in both modes: the stops a tab advances to, and the stops
/// the spaces reach. Clamped where applied, not here, so this crate doesn't
/// need to know what the editor considers a usable range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndentationConfig {
    pub style: IndentationStyle,
    pub width: u16,
}

impl Default for IndentationConfig {
    fn default() -> Self {
        Self {
            style: IndentationStyle::Tabs,
            width: 4,
        }
    }
}

/// Whether an indent is one tab character or a run of spaces. A file's
/// existing tabs are drawn at `width` under either, since the setting
/// describes the document as much as the keystroke.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum IndentationStyle {
    /// One tab character, however many columns that covers.
    #[default]
    Tabs,
    /// However many spaces reach the next stop from where the caret is.
    Spaces,
}

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
    fn with_defaults_from(&self, base: &FontConfig) -> FontConfig {
        FontConfig {
            family: self.family.clone().or_else(|| base.family.clone()),
            size: self.size.or(base.size),
        }
    }

    fn resolved(&self) -> ResolvedFont {
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

/// How much the desktop behind a translucent window is frosted when neither
/// the theme nor the base theme says: not at all, which is what JumpPad
/// showed before the setting existed. Zero is also the cheaper window - a
/// frosted backdrop is drawn by the compositor on every frame the desktop
/// moves under it.
pub const DEFAULT_BLUR: u32 = 0;

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

    /// A config whose dark slot names `theme`, which the caller defines.
    fn with_dark_theme(theme: &str) -> Config {
        toml::from_str(&format!(
            "[mode]\ndetection = \"dark\"\ntheme.dark = \"mine\"\n\n[themes.mine]\n{theme}"
        ))
        .unwrap()
    }

    fn config(toml: &str) -> Config {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn a_theme_with_no_fonts_keeps_the_defaults_for_both() {
        let theme = with_dark_theme("").theme_for(Appearance::Dark);
        assert_eq!(theme.editor_font, ResolvedFont::default());
        assert_eq!(theme.ui_font, ResolvedFont::default());
        assert_eq!(theme.editor_font.family, None);
        assert_eq!(theme.editor_font.size, DEFAULT_FONT_SIZE);
        assert_eq!(theme.ui_font.family, None);
    }

    #[test]
    fn the_editor_and_ui_fonts_are_named_independently() {
        let config = with_dark_theme(
            r#"
            editor.font.family = "JetBrains Mono"
            editor.font.size = 18.0
            ui.font.family = "Inter"
            ui.font.size = 13.0
            "#,
        );
        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.editor_font.family.as_deref(), Some("JetBrains Mono"));
        assert_eq!(theme.editor_font.size, 18.0);
        assert_eq!(theme.ui_font.family.as_deref(), Some("Inter"));
        assert_eq!(theme.ui_font.size, 13.0);
    }

    /// The rule that makes every property optional at every depth: naming
    /// one leaves its siblings, and the other section, untouched.
    #[test]
    fn an_editor_size_alone_leaves_the_family_and_the_ui_font_default() {
        let config = with_dark_theme("editor.font.size = 13.5");
        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.editor_font.family, None);
        assert_eq!(theme.editor_font.size, 13.5);
        assert_eq!(theme.ui_font, ResolvedFont::default());
    }

    #[test]
    fn a_slot_naming_nothing_of_its_own_is_the_plain_palette_for_it() {
        let config = Config::default();
        assert_eq!(
            config.theme_for(Appearance::Light),
            ResolvedTheme {
                palette: "light".to_string(),
                background_alpha: DEFAULT_ALPHA,
                background_blur: DEFAULT_BLUR,
                foreground_alpha: DEFAULT_ALPHA,
                editor_font: ResolvedFont::default(),
                ui_font: ResolvedFont::default(),
            }
        );
        assert_eq!(config.theme_for(Appearance::Dark).palette, "dark");
    }

    #[test]
    fn a_theme_with_no_palette_takes_the_one_for_its_slot() {
        let config: Config = toml::from_str(
            r#"
            [mode]
            theme.light = "mine"
            theme.dark = "mine"

            [themes.mine]
            background.alpha = 0.5
            "#,
        )
        .unwrap();
        assert_eq!(config.theme_for(Appearance::Light).palette, "light");
        assert_eq!(config.theme_for(Appearance::Dark).palette, "dark");
        // The rest of the theme is the same either way.
        assert_eq!(config.theme_for(Appearance::Dark).background_alpha, 0.5);
    }

    #[test]
    fn a_theme_named_after_a_palette_wins_its_own_name() {
        let config: Config = toml::from_str(
            r#"
            [mode]
            theme.dark = "Dracula"

            [themes.Dracula]
            palette = "Nord"
            "#,
        )
        .unwrap();
        assert_eq!(config.theme_for(Appearance::Dark).palette, "Nord");
    }

    #[test]
    fn a_slot_naming_no_theme_is_read_as_a_palette() {
        let config: Config =
            toml::from_str("[mode]\ntheme.dark = \"Tokyo Night Storm\"")
                .unwrap();
        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.palette, "Tokyo Night Storm");
        assert_eq!(theme.background_alpha, DEFAULT_ALPHA);
        assert_eq!(theme.foreground_alpha, DEFAULT_ALPHA);
        assert_eq!(theme.editor_font, ResolvedFont::default());
        assert_eq!(theme.ui_font, ResolvedFont::default());
    }

    /// The example the base theme exists for: themes named after the slots
    /// they fill, sharing a palette, differing only where they say so.
    #[test]
    fn a_file_of_themes_alone_needs_no_mode_section() {
        let config = config(
            r#"
            [themes.base]
            palette = "Ferra"

            [themes.dark]
            background.alpha = 0.95

            [themes.light]
            background.alpha = 1.0
            "#,
        );

        let light = config.theme_for(Appearance::Light);
        assert_eq!(light.palette, "Ferra");
        assert_eq!(light.background_alpha, 1.0);

        let dark = config.theme_for(Appearance::Dark);
        assert_eq!(dark.palette, "Ferra");
        assert_eq!(dark.background_alpha, 0.95);
    }

    #[test]
    fn the_slots_default_to_the_theme_names_light_and_dark() {
        assert_eq!(ThemeSlots::default().light, "light");
        assert_eq!(ThemeSlots::default().dark, "dark");
    }

    #[test]
    fn a_theme_takes_every_property_the_base_theme_names() {
        let config = config(
            r#"
            [mode]
            theme.dark = "mine"

            [themes.base]
            palette = "Nord"
            background.alpha = 0.8
            foreground.alpha = 0.9
            editor.font.family = "JetBrains Mono"
            editor.font.size = 18.0
            ui.font.family = "Inter"
            ui.font.size = 13.0

            [themes.mine]
            "#,
        );

        assert_eq!(
            config.theme_for(Appearance::Dark),
            ResolvedTheme {
                palette: "Nord".to_string(),
                background_alpha: 0.8,
                background_blur: DEFAULT_BLUR,
                foreground_alpha: 0.9,
                editor_font: ResolvedFont {
                    family: Some("JetBrains Mono".to_string()),
                    size: 18.0,
                },
                ui_font: ResolvedFont {
                    family: Some("Inter".to_string()),
                    size: 13.0,
                },
            }
        );
    }

    #[test]
    fn a_themes_own_property_beats_the_base_themes() {
        let config = config(
            r#"
            [mode]
            theme.dark = "mine"

            [themes.base]
            palette = "Nord"
            editor.font.size = 18.0

            [themes.mine]
            palette = "Dracula"
            editor.font.size = 21.0
            "#,
        );

        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.palette, "Dracula");
        assert_eq!(theme.editor_font.size, 21.0);
    }

    /// The reason every leaf is an `Option`: a theme naming the value that
    /// happens to be JumpPad's own default still has to beat a base theme
    /// that named something else.
    #[test]
    fn a_theme_can_be_solid_over_a_translucent_base() {
        let config = config(
            r#"
            [themes.base]
            background.alpha = 0.95

            [themes.light]
            background.alpha = 1.0
            "#,
        );

        assert_eq!(config.theme_for(Appearance::Light).background_alpha, 1.0);
        assert_eq!(config.theme_for(Appearance::Dark).background_alpha, 0.95);
    }

    /// Property by property, not section by section: naming a size doesn't
    /// discard the family beside it.
    #[test]
    fn the_base_theme_and_a_theme_merge_property_by_property() {
        let config = config(
            r#"
            [themes.base]
            editor.font.family = "JetBrains Mono"
            ui.font.family = "Inter"

            [themes.dark]
            editor.font.size = 21.0
            "#,
        );

        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.editor_font.family.as_deref(), Some("JetBrains Mono"));
        assert_eq!(theme.editor_font.size, 21.0);
        assert_eq!(theme.ui_font.family.as_deref(), Some("Inter"));
        assert_eq!(theme.ui_font.size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn what_neither_the_theme_nor_the_base_names_takes_the_stock_default() {
        let config = config(
            r#"
            [themes.base]
            background.alpha = 0.8

            [themes.dark]
            "#,
        );

        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.palette, "dark");
        assert_eq!(theme.foreground_alpha, DEFAULT_ALPHA);
        assert_eq!(theme.editor_font, ResolvedFont::default());
    }

    #[test]
    fn a_slot_naming_no_theme_still_takes_the_base_themes_properties() {
        let config = config(
            r#"
            [mode]
            theme.dark = "Tokyo Night Storm"

            [themes.base]
            background.alpha = 0.8
            editor.font.size = 18.0
            "#,
        );

        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.palette, "Tokyo Night Storm");
        assert_eq!(theme.background_alpha, 0.8);
        assert_eq!(theme.editor_font.size, 18.0);
    }

    /// A slot naming a palette is read as a theme naming that palette, so it
    /// outranks the base theme's the way any other named property does.
    #[test]
    fn a_slot_naming_a_palette_outranks_the_base_themes_palette() {
        let config = config(
            r#"
            [mode]
            theme.dark = "Nord"

            [themes.base]
            palette = "Ferra"
            "#,
        );

        assert_eq!(config.theme_for(Appearance::Dark).palette, "Nord");
    }

    #[test]
    fn the_base_theme_can_be_shown_in_a_slot_of_its_own() {
        let config = config(
            r#"
            [mode]
            theme.dark = "base"

            [themes.base]
            palette = "Ferra"
            background.alpha = 0.8
            "#,
        );

        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.palette, "Ferra");
        assert_eq!(theme.background_alpha, 0.8);
    }

    #[test]
    fn an_empty_theme_table_changes_nothing() {
        let empty_base = config(
            r#"
            [themes.base]

            [themes.dark]
            background.alpha = 0.5
            "#,
        );
        let no_base = config("[themes.dark]\nbackground.alpha = 0.5");

        assert_eq!(
            empty_base.theme_for(Appearance::Dark),
            no_base.theme_for(Appearance::Dark)
        );
    }

    #[test]
    fn a_theme_with_no_blur_named_leaves_the_desktop_sharp() {
        let theme = config("[themes.dark]\nbackground.alpha = 0.8")
            .theme_for(Appearance::Dark);
        assert_eq!(theme.background_blur, 0);
        assert_eq!(theme.background_blur, DEFAULT_BLUR);
    }

    #[test]
    fn a_theme_can_ask_for_a_frosted_desktop() {
        let theme = config(
            r#"
            [themes.dark]
            background.alpha = 0.8
            background.blur = 24
            "#,
        )
        .theme_for(Appearance::Dark);

        assert_eq!(theme.background_blur, 24);
        assert_eq!(theme.background_alpha, 0.8);
    }

    /// The same per-leaf inheritance the alpha beside it gets: a shared
    /// blur, and one slot that opts back out of it.
    #[test]
    fn blur_inherits_from_the_base_theme_and_a_theme_can_turn_it_off() {
        let config = config(
            r#"
            [themes.base]
            background.alpha = 0.8
            background.blur = 24

            [themes.dark]

            [themes.light]
            background.blur = 0
            "#,
        );

        assert_eq!(config.theme_for(Appearance::Dark).background_blur, 24);
        assert_eq!(config.theme_for(Appearance::Light).background_blur, 0);
    }

    /// The amount is carried whole rather than flattened to on/off here -
    /// only the platform knows whether it can do anything with it.
    #[test]
    fn an_unreasonable_blur_is_carried_as_written() {
        let theme = config("[themes.dark]\nbackground.blur = 4000")
            .theme_for(Appearance::Dark);
        assert_eq!(theme.background_blur, 4000);
    }

    /// A negative radius is not a smaller one, and the file says so rather
    /// than rounding it into a setting nobody asked for.
    #[test]
    fn a_negative_blur_fails_the_file() {
        assert!(
            toml::from_str::<Config>("[themes.dark]\nbackground.blur = -1")
                .is_err()
        );
    }

    /// Blur says nothing about how the window is created - the compositor
    /// is asked for it after the fact - so it can't be what makes a window
    /// transparent. A theme that asks for one without an alpha to show it
    /// through gets a solid window and no frost, which is the honest
    /// answer rather than a translucency it never asked for.
    #[test]
    fn blur_alone_does_not_ask_for_a_transparent_window() {
        assert!(
            !config("[themes.dark]\nbackground.blur = 24").wants_transparency()
        );
    }

    #[test]
    fn pinned_detection_ignores_what_the_os_reports() {
        let pinned: ModeConfig =
            toml::from_str(r#"detection = "dark""#).unwrap();
        assert_eq!(pinned.showing(Some(Appearance::Light)), Appearance::Dark);
        assert_eq!(pinned.showing(None), Appearance::Dark);
    }

    #[test]
    fn automatic_detection_follows_the_os_and_defaults_to_light() {
        let auto = ModeConfig::default();
        assert_eq!(auto.detection, Detection::Auto);
        assert_eq!(auto.showing(Some(Appearance::Dark)), Appearance::Dark);
        assert_eq!(auto.showing(Some(Appearance::Light)), Appearance::Light);
        // No preference reported, or none readable.
        assert_eq!(auto.showing(None), Appearance::Light);
    }

    /// Over every theme, not just the two the slots name: `[mode]` is
    /// live-editable, so any theme in the file can reach the screen without
    /// a restart, and the window's transparency cannot follow it there.
    #[test]
    fn transparency_is_wanted_if_any_theme_at_all_asks_for_it() {
        assert!(!Config::default().wants_transparency());

        let solid = with_dark_theme("background.alpha = 1.0");
        assert!(!solid.wants_transparency());

        let unnamed: Config = toml::from_str(
            r#"
            [themes.never_shown]
            background.alpha = 0.9
            "#,
        )
        .unwrap();
        assert!(unnamed.wants_transparency());
    }

    #[test]
    fn a_translucent_base_theme_wants_transparency() {
        assert!(
            config("[themes.base]\nbackground.alpha = 0.9")
                .wants_transparency()
        );
    }

    /// The base theme is one of these entries, and `[mode]` is live-editable,
    /// so a slot can name it after the window has already been created.
    #[test]
    fn a_base_theme_every_other_theme_overrides_still_wants_transparency() {
        let overridden = config(
            r#"
            [themes.base]
            background.alpha = 0.95

            [themes.light]
            background.alpha = 1.0

            [themes.dark]
            background.alpha = 1.0
            "#,
        );
        assert!(overridden.wants_transparency());
    }

    /// Everything in the default config is a default, so the file written on
    /// first run is the one section that isn't: the languages.
    #[test]
    fn the_written_default_file_carries_only_the_languages() {
        let written = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(written.contains("[[languages]]"));
        for defaulted in [
            "[mode]",
            "[themes]",
            "[visor]",
            "[window]",
            "[scroll]",
            "[history]",
            "[files]",
            "detection",
            "palette",
            "family",
        ] {
            assert!(
                !written.contains(defaulted),
                "{defaulted} sits on its default and shouldn't be written"
            );
        }
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
        let file = TempFile::with_contents(
            "valid.toml",
            "[mode]\ntheme.dark = \"Dracula\"",
        );
        let missing = PathBuf::from("does_not_exist/config.toml");
        let config: Config = try_parse(&[missing, file.0.clone()]).unwrap();
        assert_eq!(config.mode.theme.dark, "Dracula");
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
        let file =
            TempFile::with_contents("broken.toml", "[mode]\ndetection = ");
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
    fn an_unnamed_alpha_resolves_to_fully_solid() {
        let theme = with_dark_theme("").theme_for(Appearance::Dark);
        assert_eq!(theme.background_alpha, 1.0);
        assert_eq!(theme.foreground_alpha, 1.0);
        assert_eq!(DEFAULT_ALPHA, 1.0);
    }

    #[test]
    fn config_toml_with_no_window_section_keeps_decorations() {
        // Old config files predate the section and must stay valid.
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.window, WindowConfig::default());
        assert!(config.window.decorations);
    }

    #[test]
    fn config_toml_can_turn_decorations_off() {
        let config: Config = toml::from_str(
            r#"

            [window]
            decorations = false
            "#,
        )
        .unwrap();
        assert!(!config.window.decorations);
    }

    #[test]
    fn config_toml_with_no_languages_keeps_the_builtin_defaults() {
        let config: Config = toml::from_str("").unwrap();
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

            [syntaxes]
            toml = ["toml"]

            [[comment_styles]]
            syntaxes = ["toml"]
            prefix = "; "
            "#,
        )
        .unwrap();
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

    /// Every theme leaf is skipped when unset, so an untouched section writes
    /// as a bare header - which still has to read back as the same theme.
    #[test]
    fn a_theme_with_nothing_set_round_trips_through_toml() {
        let mut config = Config::default();
        config
            .themes
            .insert(BASE_THEME.to_string(), ThemeConfig::default());

        let written = toml::to_string_pretty(&config).unwrap();
        let reparsed: Config = toml::from_str(&written).unwrap();
        assert_eq!(reparsed, config);
    }

    #[test]
    fn the_sample_files_parse() {
        let config: Config =
            toml::from_str(include_str!("../../../config/config.sample.toml"))
                .unwrap();
        assert!(!config.languages.is_empty());
        // Parsing is not enough on its own: an unknown key is skipped in
        // silence and leaves the default standing, so a sample naming a key
        // that no longer exists would still parse and still be wrong. The
        // sample sets both indentation keys away from their defaults, which
        // is what makes them worth reading back.
        assert_eq!(config.indentation.style, IndentationStyle::Spaces);
        assert_eq!(config.indentation.width, 2);
        let _: KeybindsConfig = toml::from_str(include_str!(
            "../../../config/keybinds.sample.toml"
        ))
        .unwrap();
    }

    #[test]
    fn a_mode_pins_a_slot_only_when_it_names_one() {
        let pinning = |detection| ModeConfig {
            detection,
            ..ModeConfig::default()
        };

        assert_eq!(pinning(Detection::Auto).pinned(), None);
        assert_eq!(pinning(Detection::Light).pinned(), Some(Appearance::Light));
        assert_eq!(pinning(Detection::Dark).pinned(), Some(Appearance::Dark));
    }

    /// A pinned mode shows what it pins, whatever the OS says - so the two
    /// answers agree wherever both are asked.
    #[test]
    fn a_pinned_mode_shows_what_it_pins() {
        for detection in [Detection::Light, Detection::Dark] {
            let mode = ModeConfig {
                detection,
                ..ModeConfig::default()
            };

            for os in [None, Some(Appearance::Light), Some(Appearance::Dark)] {
                assert_eq!(Some(mode.showing(os)), mode.pinned());
            }
        }
    }

    #[test]
    fn config_toml_with_no_scroll_section_falls_back_to_the_shipped_speed() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.scroll, ScrollConfig::default());
        assert_eq!(config.scroll.sensitivity, 1.0);
        assert_eq!(config.scroll.drag_speed, 1.0);
    }

    #[test]
    fn config_toml_with_a_scroll_section_parses() {
        let config: Config = toml::from_str(
            r#"

            [scroll]
            sensitivity = 2.5
            drag_speed = 0.5
            "#,
        )
        .unwrap();
        assert_eq!(config.scroll.sensitivity, 2.5);
        assert_eq!(config.scroll.drag_speed, 0.5);
    }

    #[test]
    fn one_scroll_speed_can_be_set_without_the_other() {
        let config: Config = toml::from_str(
            r#"

            [scroll]
            drag_speed = 0.25
            "#,
        )
        .unwrap();
        assert_eq!(config.scroll.drag_speed, 0.25);
        assert_eq!(config.scroll.sensitivity, 1.0);
    }

    #[test]
    fn config_toml_with_no_files_section_asks_before_overwriting() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.files, FilesConfig::default());
        assert!(config.files.save_conflict_resolution.asks());
    }

    #[test]
    fn config_toml_can_ask_for_saves_that_always_win() {
        let config: Config = toml::from_str(
            r#"

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
    fn config_toml_with_no_indentation_section_indents_with_tabs() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.indentation, IndentationConfig::default());
        assert_eq!(config.indentation.style, IndentationStyle::Tabs);
        assert_eq!(config.indentation.width, 4);
    }

    #[test]
    fn config_toml_can_ask_for_spaces() {
        let config: Config = toml::from_str(
            r#"

            [indentation]
            style = "spaces"
            width = 2
            "#,
        )
        .unwrap();
        assert_eq!(config.indentation.style, IndentationStyle::Spaces);
        assert_eq!(config.indentation.width, 2);
    }

    #[test]
    fn an_indentation_width_can_be_set_without_the_style() {
        let config: Config = toml::from_str(
            r#"

            [indentation]
            width = 8
            "#,
        )
        .unwrap();
        assert_eq!(config.indentation.style, IndentationStyle::Tabs);
        assert_eq!(config.indentation.width, 8);
    }

    #[test]
    fn an_unknown_indentation_style_fails_the_files_parse() {
        // Loudly, like every other string-valued enum: a typo here would
        // otherwise indent with something the user never asked for.
        let parsed: Result<Config, _> = toml::from_str(
            r#"

            [indentation]
            style = "tab"
            "#,
        );
        assert!(parsed.is_err());
    }

    #[test]
    fn a_theme_with_no_alpha_section_falls_back_to_solid() {
        let theme = with_dark_theme("").theme_for(Appearance::Dark);
        assert_eq!(theme.background_alpha, DEFAULT_ALPHA);
        assert_eq!(theme.foreground_alpha, DEFAULT_ALPHA);
    }

    #[test]
    fn a_theme_can_set_either_surface_alpha() {
        let config = with_dark_theme(
            r#"
            background.alpha = 0.7
            foreground.alpha = 0.9
            "#,
        );
        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.background_alpha, 0.7);
        assert_eq!(theme.foreground_alpha, 0.9);
    }

    /// The surfaces are separate sections, so naming one says nothing about
    /// the other.
    #[test]
    fn a_theme_naming_one_surface_leaves_the_other_solid() {
        let config = with_dark_theme("background.alpha = 0.5");
        let theme = config.theme_for(Appearance::Dark);
        assert_eq!(theme.background_alpha, 0.5);
        assert_eq!(theme.foreground_alpha, DEFAULT_ALPHA);
    }
}
