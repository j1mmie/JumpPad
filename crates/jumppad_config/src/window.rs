use serde::{Deserialize, Serialize};

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
/// theme's, and failing that [`crate::font::DEFAULT_ALPHA`]. Its own section so anything
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

/// What the desktop behind a translucent window gets when neither the theme
/// nor the base theme says: no frosting, which is what JumpPad showed before
/// the setting existed. It is also the cheaper window - a frosted backdrop is
/// drawn by the compositor on every frame the desktop moves under it.
pub const DEFAULT_BLUR: Blur = Blur::None;
