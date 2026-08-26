use super::*;

#[test]
fn same_name_codes_convert_directly() {
    assert_eq!(
        to_iced_code(GhCode::KeyA),
        Some(iced_core::keyboard::key::Code::KeyA)
    );
    assert_eq!(
        to_iced_code(GhCode::ArrowUp),
        Some(iced_core::keyboard::key::Code::ArrowUp)
    );
    assert_eq!(
        to_iced_code(GhCode::Tab),
        Some(iced_core::keyboard::key::Code::Tab)
    );
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
