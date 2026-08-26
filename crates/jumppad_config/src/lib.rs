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
    pub background_blur: Blur,
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
    /// How much the desktop showing through is frosted, or which of
    /// Windows' acrylics does the frosting - see [`Blur`]. Nothing to see
    /// at `alpha = 1.0`, where there is no desktop showing through at all.
    /// Unnamed takes the base theme's, and failing that [`DEFAULT_BLUR`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<Blur>,
}

/// What a theme asks the compositor to do with the desktop behind a
/// translucent window.
///
/// The forms exist because the platforms offer different choices and neither
/// offers the other's. macOS takes a blur *radius* and has one blur; Windows
/// has no radius at all but two acrylics, differing in whether the frost
/// survives the window losing focus. **Each platform reads only the forms it
/// can act on, and everything else is [`Blur::None`]** - so a radius is no
/// blur on Windows, and an acrylic is no blur on macOS, rather than either
/// guessing at what the other meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blur {
    /// `background.blur = "none"`, or `0`, or nothing at all. No frosting
    /// anywhere.
    #[default]
    None,
    /// `background.blur = 24`, the blur radius. **macOS only.**
    Radius(u32),
    /// `background.blur = "acrylic10"`. **Windows only**, and its
    /// `DWMSBT_TRANSIENTWINDOW` - which DWM stops drawing while the window
    /// is not focused.
    Acrylic10,
    /// `background.blur = "acrylic11"`. **Windows only**, and the
    /// acrylic that keeps frosting an unfocused window - a different API
    /// with its own costs, see `windows.rs`.
    Acrylic11,
}

/// How the named forms are spelled in the file. The one place those strings
/// are written, so parsing and the error naming the alternatives cannot
/// disagree.
const NONE: &str = "none";
const ACRYLIC_10: &str = "acrylic10";
const ACRYLIC_11: &str = "acrylic11";

impl Blur {
    /// The radius to frost by, and `0` for everything that names no radius -
    /// the acrylics included, since having no amount to give is exactly what
    /// they say.
    pub fn radius(self) -> u32 {
        match self {
            Blur::Radius(radius) => radius,
            _ => 0,
        }
    }
}

impl Serialize for Blur {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match self {
            Blur::None => serializer.serialize_str(NONE),
            Blur::Radius(radius) => serializer.serialize_u32(*radius),
            Blur::Acrylic10 => serializer.serialize_str(ACRYLIC_10),
            Blur::Acrylic11 => serializer.serialize_str(ACRYLIC_11),
        }
    }
}

/// Hand-written rather than `#[serde(untagged)]`, which reports a value that
/// is neither form as "data did not match any variant" and names nothing the
/// reader could have written instead. This says which words there are, and
/// says of a negative radius that it is negative.
impl<'de> Deserialize<'de> for Blur {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct BlurVisitor;

        impl serde::de::Visitor<'_> for BlurVisitor {
            type Value = Blur;

            fn expecting(
                &self,
                formatter: &mut std::fmt::Formatter,
            ) -> std::fmt::Result {
                write!(
                    formatter,
                    "a blur radius, or {NONE:?}, {ACRYLIC_10:?} or \
                     {ACRYLIC_11:?}"
                )
            }

            fn visit_u64<E: serde::de::Error>(
                self,
                radius: u64,
            ) -> Result<Blur, E> {
                match u32::try_from(radius) {
                    // The two spellings of off are the same setting, so they
                    // stop being two the moment the file is read.
                    Ok(0) => Ok(Blur::None),
                    Ok(radius) => Ok(Blur::Radius(radius)),
                    Err(_) => Err(E::custom(format!(
                        "blur radius {radius} is far too large"
                    ))),
                }
            }

            fn visit_i64<E: serde::de::Error>(
                self,
                radius: i64,
            ) -> Result<Blur, E> {
                u64::try_from(radius)
                    .map_err(|_| {
                        E::custom(format!(
                            "blur radius {radius} is negative; 0 is no blur"
                        ))
                    })
                    .and_then(|radius| self.visit_u64(radius))
            }

            fn visit_str<E: serde::de::Error>(
                self,
                name: &str,
            ) -> Result<Blur, E> {
                match name {
                    NONE => Ok(Blur::None),
                    ACRYLIC_10 => Ok(Blur::Acrylic10),
                    ACRYLIC_11 => Ok(Blur::Acrylic11),
                    _ => Err(E::unknown_variant(
                        name,
                        &[NONE, ACRYLIC_10, ACRYLIC_11],
                    )),
                }
            }
        }

        deserializer.deserialize_any(BlurVisitor)
    }
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

/// How the hardware-rendered binary talks to the graphics stack: which
/// adapter it asks for, and whether it waits for the display before showing
/// a frame. Read only by `jumppad-gpu` - the software binary has no adapter
/// to choose and no swapchain to wait on.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(default)]
pub struct GpuConfig {
    pub power: GpuPower,

    /// `vsync = true` makes a frame wait for the display before it is shown.
    /// **Defaults to `false`**, which is the unusual answer and is the one
    /// that makes `jumppad-gpu` feel like `jumppad`.
    ///
    /// The two binaries reach the screen by completely different routes, and
    /// only one of them ever queued a frame. `jumppad` presents through
    /// `softbuffer`, which on Windows is a GDI blit into the window's
    /// redirection bitmap and on macOS is a layer-contents swap: neither
    /// blocks, so a frame drawn from the pointer's current position is on
    /// its way to the compositor before the function returns. `jumppad-gpu`
    /// presents through a swapchain, and with vsync on that swapchain holds
    /// the frame until the next refresh, then the desktop compositor spends
    /// another one showing it. Two refreshes at 60Hz is 33ms between moving
    /// the mouse and seeing the selection follow it, and that is the lag
    /// reported from Windows: a highlight two or three frames behind the
    /// pointer, and scrolling that arrives late.
    ///
    /// Turning it off costs nothing here that it would cost a game. Tearing
    /// is what vsync buys, and a torn frame needs the scanout to change
    /// mid-scan - which a window composited by DWM or by the macOS window
    /// server cannot do, because the compositor is what reaches the display,
    /// not this app's swapchain. There is no busy loop either: JumpPad draws
    /// when something asks it to and idles at zero frames otherwise, so
    /// "unsynchronized" here means "shown as soon as it is drawn", not
    /// "drawn as fast as the GPU can".
    ///
    /// It stays configurable because Linux can put this app's frames on the
    /// scanout directly, which is the one arrangement that can tear: an X11
    /// session running without a compositor, or a Wayland compositor giving
    /// a fullscreen window its own plane. Turn it back on if a frame ever
    /// shows up torn in half.
    pub vsync: bool,
}

/// How much GPU to ask for.
///
/// Defaults to [`GpuPower::High`], matching what iced asks for when nothing
/// says otherwise. Not because a plaintext editor needs a discrete card - it
/// does not, and asking for one costs battery - but because on Windows the
/// adapter decides whether the window can be translucent at all, and the
/// discrete one is likelier to say yes. Defaulting the other way traded a
/// feature away for power this app never uses.
///
/// The asymmetry belongs to the Vulkan backend: it asks the driver which
/// composite alpha modes a surface supports, and drivers disagree. An NVIDIA
/// adapter offered `PreMultiplied`; the AMD integrated one beside it offered
/// only `Opaque`, which silently costs `background.alpha` and every acrylic
/// resting on it. Neither macOS nor the software binary is affected - wgpu's
/// Metal backend reports its alpha modes as a constant, so any adapter there
/// can be translucent, and the software renderer presents through GDI on
/// Windows without consulting an adapter at all.
///
/// So `"low"` is the lever to reach for on a laptop, or to keep JumpPad off
/// whatever a discrete card is already busy with - an NVIDIA driver was seen
/// recursing to a stack overflow inside `vkCreateDevice` while a game held
/// the GPU, where the integrated adapter started every time. That is a
/// driver bug rather than something this setting fixes; it is only the lever
/// that avoids it, at the cost above.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum GpuPower {
    /// `power = "high"`. The discrete adapter where the machine has one.
    #[default]
    High,
    /// `power = "low"`. The integrated adapter. Cheaper, and out of a busy
    /// discrete card's way - but on Windows it is the choice that can leave
    /// you with an opaque window, so check that transparency survived it.
    Low,
    /// `power = "auto"`. State no preference and take whatever wgpu ranks
    /// first for the surface.
    Auto,
}

impl GpuPower {
    /// The spelling wgpu reads out of `WGPU_POWER_PREF`.
    ///
    /// That variable is how this setting reaches iced at all - see
    /// `prefer_gpu` in `jumppad`'s `lib.rs`. `Auto` is wgpu's `"none"`,
    /// meaning no preference rather than no GPU.
    pub fn as_wgpu_power_pref(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
            Self::Auto => "none",
        }
    }
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

/// VS Code's `editor.wordSeparators`, character for character - the list
/// JumpPad ships with, so a `settings.json` line can be pasted straight
/// across. Mirrored by `jumppad_textarea`'s `word::DEFAULT_SEPARATORS`,
/// which is what a widget nobody has told otherwise uses.
pub const DEFAULT_WORD_SEPARATORS: &str = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?";

/// What ends a word: the characters a caret moving by one word stops at,
/// and the ones a double click stops selecting at.
///
/// Whitespace always separates words, whatever this names, so a list never
/// has to spell out a space, a tab or a newline. Anything the list leaves
/// out is part of a word - drop `-` from it and `font-size` is one word
/// rather than three.
///
/// The whole list is replaced rather than added to, the way a
/// user-provided `[[languages]]` array replaces the built-in one, and the
/// way VS Code's own setting behaves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WordsConfig {
    pub separators: String,
}

impl Default for WordsConfig {
    fn default() -> Self {
        Self {
            separators: DEFAULT_WORD_SEPARATORS.to_string(),
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

/// What the desktop behind a translucent window gets when neither the theme
/// nor the base theme says: no frosting, which is what JumpPad showed before
/// the setting existed. It is also the cheaper window - a frosted backdrop is
/// drawn by the compositor on every frame the desktop moves under it.
pub const DEFAULT_BLUR: Blur = Blur::None;

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

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
