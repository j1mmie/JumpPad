//! Resolving the typeface and text size the editor draws documents with.

use std::collections::HashSet;
use std::ops::RangeInclusive;
use std::sync::{Mutex, OnceLock};

use iced::Font;
use iced::advanced::graphics;

/// Fallback for an editor no config has reached yet. Mirrors
/// `DEFAULT_FONT_SIZE`, which in turn is iced's own default text size.
pub const DEFAULT_SIZE: f32 = 16.0;

/// Sizes a person could still read and still edit their way back out of. A
/// value outside it is a typo rather than an intent, and a small enough one
/// would leave `config.toml` as the only way to undo itself.
const READABLE_SIZES: RangeInclusive<f32> = 4.0..=200.0;

/// The editor's text size, with a nonsense value pulled back to a usable
/// one. `NaN` takes the default instead of the nearest bound, comparing
/// false against both.
pub fn clamp_size(size: f32) -> f32 {
    if size.is_nan() {
        return DEFAULT_SIZE;
    }
    size.clamp(*READABLE_SIZES.start(), *READABLE_SIZES.end())
}

/// The [`Font`] for a font family this machine has installed, or `None`
/// when no installed face carries that name - which is the caller's cue to
/// say so, since text set in a family nothing provides is drawn in whatever
/// face the platform falls back to, usually a proportional one.
///
/// Matched case-insensitively but resolved to the face's own spelling: the
/// text shaper compares family names exactly, so `"jetbrains mono"` would
/// otherwise pass this check and still draw as a fallback.
///
/// The first call builds the system's font list, which takes long enough to
/// notice - hence only on a `[font] family` that actually names something.
pub fn installed(family: &str) -> Option<Font> {
    let mut font_system =
        graphics::text::font_system().write().expect("font system");
    let name = font_system
        .raw()
        .db()
        .faces()
        .flat_map(|face| face.families.iter())
        .find(|(name, _)| name.eq_ignore_ascii_case(family))
        .map(|(name, _)| interned(name))?;

    Some(Font::with_name(name))
}

/// [`Font`] holds a family name as a `&'static str`, but a name arrives
/// owned by the config load that parsed it. Leaking bridges the two; the
/// set makes a given name cost that leak once, however many reloads name it.
fn interned(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();

    let mut names = NAMES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .expect("interned font names");
    if let Some(name) = names.get(name) {
        return name;
    }
    let name: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(name);

    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_sizes_are_pulled_back_into_range() {
        assert_eq!(clamp_size(18.0), 18.0);
        assert_eq!(clamp_size(0.0), *READABLE_SIZES.start());
        assert_eq!(clamp_size(-3.0), *READABLE_SIZES.start());
        assert_eq!(clamp_size(10_000.0), *READABLE_SIZES.end());
        assert_eq!(clamp_size(f32::NAN), DEFAULT_SIZE);
    }

    #[test]
    fn a_family_name_is_leaked_once_however_often_it_is_resolved() {
        let first = interned("JumpPad Test Family");
        let second = interned(&String::from("JumpPad Test Family"));
        assert_eq!(first.as_ptr(), second.as_ptr());
    }
}
