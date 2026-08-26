use super::*;
use std::collections::HashSet;

#[test]
fn every_action_has_exactly_one_row() {
    assert_eq!(Action::ALL.len(), ACTIONS.len());
    for action in Action::ALL {
        assert_eq!(
            action.def().action,
            *action,
            "{action}'s row is out of order, so `def` indexes the wrong one"
        );
    }
}

#[test]
fn names_are_unique() {
    // Names are what `keybinds.toml` binds to, so a duplicate would make
    // one of the two actions unreachable from a config file.
    let mut seen = HashSet::new();
    for action in Action::ALL {
        assert!(seen.insert(action.name()), "duplicate name {action}");
    }
}

#[test]
fn names_are_snake_case_and_labels_are_not_empty() {
    for action in Action::ALL {
        let name = action.name();
        assert!(
            name.chars().all(|c| c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || c == '_'),
            "{name} should be snake_case"
        );
        assert!(!action.label().is_empty(), "{name} has no label");
    }
}

#[test]
fn a_name_round_trips_to_its_action() {
    for action in Action::ALL {
        assert_eq!(Action::from_name(action.name()), Some(*action));
    }
    assert_eq!(Action::from_name("no_such_action"), None);
}

#[test]
fn editor_actions_want_the_editor_focused() {
    // Not a law of nature, but it is true of every action today, and an
    // editor action that fires with the editor unfocused would be a
    // surprise worth writing down deliberately.
    for action in Action::in_category(Category::Editor) {
        assert_eq!(action.context(), Context::EditorFocused, "{action}");
    }
}
