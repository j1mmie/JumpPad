use std::collections::HashMap;

use global_hotkey::hotkey::{Code as GhCode, HotKey, Modifiers as GhModifiers};

/// A chord resolved into iced-native types, ready to compare directly
/// against a `keyboard::Event::KeyPressed`'s `(modifiers, physical_key)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolvedKeybind {
    pub modifiers: iced_core::keyboard::Modifiers,
    pub code: iced_core::keyboard::key::Code,
}

/// Converts `global_hotkey`'s `Code` to iced's own `keyboard::key::Code` -
/// both track W3C UI Events `code` values, so almost every variant matches
/// by name. `MetaLeft`/`MetaRight` are iced's `SuperLeft`/`SuperRight`;
/// `Super` and other iced-less variants resolve to `None`.
fn to_iced_code(code: GhCode) -> Option<iced_core::keyboard::key::Code> {
    use iced_core::keyboard::key::Code as Ic;

    macro_rules! same_name {
        ($($name:ident),* $(,)?) => {
            match code {
                $(GhCode::$name => Some(Ic::$name),)*
                GhCode::MetaLeft => Some(Ic::SuperLeft),
                GhCode::MetaRight => Some(Ic::SuperRight),
                _ => None,
            }
        };
    }

    same_name![
        Abort, Again, AltLeft, AltRight, ArrowDown, ArrowLeft, ArrowRight, ArrowUp,
        AudioVolumeDown, AudioVolumeMute, AudioVolumeUp, Backquote, Backslash, Backspace,
        BracketLeft, BracketRight, BrowserBack, BrowserFavorites, BrowserForward, BrowserHome,
        BrowserRefresh, BrowserSearch, BrowserStop, CapsLock, Comma, ContextMenu, ControlLeft,
        ControlRight, Convert, Copy, Cut, Delete, Digit0, Digit1, Digit2, Digit3, Digit4,
        Digit5, Digit6, Digit7, Digit8, Digit9, Eject, End, Enter, Equal, Escape, F1, F10,
        F11, F12, F13, F14, F15, F16, F17, F18, F19, F2, F20, F21, F22, F23, F24, F25, F26,
        F27, F28, F29, F3, F30, F31, F32, F33, F34, F35, F4, F5, F6, F7, F8, F9, Find, Fn,
        FnLock, Help, Hiragana, Home, Hyper, Insert, IntlBackslash, IntlRo, IntlYen, KanaMode,
        Katakana, KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL,
        KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
        Lang1, Lang2, Lang3, Lang4, Lang5, LaunchApp1, LaunchApp2, LaunchMail, MediaPlayPause,
        MediaSelect, MediaStop, MediaTrackNext, MediaTrackPrevious, Minus, NonConvert,
        NumLock, Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7,
        Numpad8, Numpad9, NumpadAdd, NumpadBackspace, NumpadClear, NumpadClearEntry,
        NumpadComma, NumpadDecimal, NumpadDivide, NumpadEnter, NumpadEqual, NumpadHash,
        NumpadMemoryAdd, NumpadMemoryClear, NumpadMemoryRecall, NumpadMemoryStore,
        NumpadMemorySubtract, NumpadMultiply, NumpadParenLeft, NumpadParenRight, NumpadStar,
        NumpadSubtract, Open, PageDown, PageUp, Paste, Pause, Period, Power, PrintScreen,
        Props, Quote, Resume, ScrollLock, Select, Semicolon, ShiftLeft, ShiftRight, Slash,
        Sleep, Space, Suspend, Tab, Turbo, Undo, WakeUp,
    ]
}

/// Keeps only the four bits with a direct iced equivalent; the rest
/// (`META`, `CAPS_LOCK`, ...) have no iced-side meaning and are dropped deliberately.
fn to_iced_modifiers(modifiers: GhModifiers) -> iced_core::keyboard::Modifiers {
    let mut resolved = iced_core::keyboard::Modifiers::empty();
    if modifiers.contains(GhModifiers::SHIFT) {
        resolved |= iced_core::keyboard::Modifiers::SHIFT;
    }
    if modifiers.contains(GhModifiers::CONTROL) {
        resolved |= iced_core::keyboard::Modifiers::CTRL;
    }
    if modifiers.contains(GhModifiers::ALT) {
        resolved |= iced_core::keyboard::Modifiers::ALT;
    }
    if modifiers.contains(GhModifiers::SUPER) {
        resolved |= iced_core::keyboard::Modifiers::LOGO;
    }
    resolved
}

fn resolve(hotkey: &HotKey) -> Option<ResolvedKeybind> {
    Some(ResolvedKeybind {
        modifiers: to_iced_modifiers(hotkey.mods),
        code: to_iced_code(hotkey.key)?,
    })
}

/// Resolves a raw `command name -> HotKey` overrides map into iced-native
/// types, dropping (and logging) any entry with no iced equivalent.
pub(crate) fn resolved_overrides(
    overrides: &HashMap<String, HotKey>,
) -> HashMap<String, ResolvedKeybind> {
    overrides
        .iter()
        .filter_map(|(name, hotkey)| match resolve(hotkey) {
            Some(resolved) => Some((name.clone(), resolved)),
            None => {
                eprintln!(
                    "xizor_config: override {name:?} uses a key with no iced equivalent, ignoring"
                );
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_codes_convert_directly() {
        assert_eq!(to_iced_code(GhCode::KeyA), Some(iced_core::keyboard::key::Code::KeyA));
        assert_eq!(to_iced_code(GhCode::ArrowUp), Some(iced_core::keyboard::key::Code::ArrowUp));
        assert_eq!(to_iced_code(GhCode::Tab), Some(iced_core::keyboard::key::Code::Tab));
        assert_eq!(
            to_iced_code(GhCode::BracketLeft),
            Some(iced_core::keyboard::key::Code::BracketLeft)
        );
    }

    #[test]
    fn meta_left_and_right_convert_to_super_left_and_right() {
        assert_eq!(
            to_iced_code(GhCode::MetaLeft),
            Some(iced_core::keyboard::key::Code::SuperLeft)
        );
        assert_eq!(
            to_iced_code(GhCode::MetaRight),
            Some(iced_core::keyboard::key::Code::SuperRight)
        );
    }

    #[test]
    fn super_and_chromium_only_extras_have_no_iced_equivalent() {
        assert_eq!(to_iced_code(GhCode::Super), None);
        assert_eq!(to_iced_code(GhCode::BrightnessUp), None);
        assert_eq!(to_iced_code(GhCode::Unidentified), None);
    }

    #[test]
    fn modifiers_map_the_four_shared_bits() {
        let mods = GhModifiers::SHIFT | GhModifiers::CONTROL;
        let resolved = to_iced_modifiers(mods);
        assert!(resolved.shift());
        assert!(resolved.control());
        assert!(!resolved.alt());
        assert!(!resolved.logo());
    }

    #[test]
    fn super_modifier_maps_to_logo() {
        let resolved = to_iced_modifiers(GhModifiers::SUPER);
        assert!(resolved.logo());
    }

    #[test]
    fn unsupported_modifier_bits_are_dropped_not_errored() {
        // META has no iced-side meaning - should resolve to no modifiers at
        // all, not panic or silently alias to something else.
        let resolved = to_iced_modifiers(GhModifiers::META);
        assert_eq!(resolved, iced_core::keyboard::Modifiers::empty());
    }

    #[test]
    fn resolved_overrides_round_trips_a_valid_entry() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "new_tab".to_string(),
            HotKey::new(Some(GhModifiers::CONTROL), GhCode::KeyN),
        );
        let resolved = resolved_overrides(&overrides);
        assert_eq!(
            resolved.get("new_tab"),
            Some(&ResolvedKeybind {
                modifiers: iced_core::keyboard::Modifiers::CTRL,
                code: iced_core::keyboard::key::Code::KeyN,
            })
        );
    }

    #[test]
    fn resolved_overrides_drops_entries_with_no_iced_equivalent() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "weird".to_string(),
            HotKey::new(Some(GhModifiers::CONTROL), GhCode::Super),
        );
        let resolved = resolved_overrides(&overrides);
        assert!(resolved.is_empty());
    }
}
