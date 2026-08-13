//! JumpPad's default key chords, in one table.
//!
//! Keys are *a* way to ask for an [`Action`], not what an action is. This
//! crate maps one to the other and holds nothing else; a mouse or gesture
//! binding would sit beside it, mapping its own input onto the same actions.
//!
//! These used to be `if` chains in two crates - `key_binding` in
//! `jumppad_textarea` and `handle_hotkey` in `jumppad` - where the default
//! chord was the one part of an action's identity that wasn't already written
//! down as data.

use iced_core::keyboard::{Key, Modifiers, key};
use jumppad_actions::{Action, Context};

/// What has to be held. `command` and `jump` are *roles*, not physical keys:
/// `command` is Cmd on macOS and Ctrl elsewhere, `jump` is Option on macOS and
/// Ctrl elsewhere, matching `iced_core`'s `Modifiers::command`/`jump`.
/// `control` is literal Ctrl on every platform - Ctrl+Tab needs that, and
/// wanting it is why that binding used to have to dodge the `command()` gate
/// by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mods {
    pub command: bool,
    pub jump: bool,
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Mods {
    const fn new() -> Self {
        Self {
            command: false,
            jump: false,
            control: false,
            shift: false,
            alt: false,
        }
    }

    pub const NONE: Self = Self::new();
    pub const COMMAND: Self = Self {
        command: true,
        ..Self::new()
    };
    pub const COMMAND_SHIFT: Self = Self {
        command: true,
        shift: true,
        ..Self::new()
    };
    pub const CONTROL: Self = Self {
        control: true,
        ..Self::new()
    };
    pub const JUMP: Self = Self {
        jump: true,
        ..Self::new()
    };
    pub const ALT: Self = Self {
        alt: true,
        ..Self::new()
    };
    /// Cmd+Opt on macOS, Ctrl+Alt elsewhere - `command` is the platform
    /// accelerator, `alt` is the literal Option/Alt key on both.
    pub const COMMAND_ALT: Self = Self {
        command: true,
        alt: true,
        ..Self::new()
    };

    /// The concrete modifier mask this spec means on the platform in hand.
    pub fn mask(self) -> Modifiers {
        let macos = cfg!(target_os = "macos");
        let mut mask = Modifiers::empty();
        if self.command {
            mask |= if macos {
                Modifiers::LOGO
            } else {
                Modifiers::CTRL
            };
        }
        if self.jump {
            mask |= if macos {
                Modifiers::ALT
            } else {
                Modifiers::CTRL
            };
        }
        if self.control {
            mask |= Modifiers::CTRL;
        }
        if self.shift {
            mask |= Modifiers::SHIFT;
        }
        if self.alt {
            mask |= Modifiers::ALT;
        }
        mask
    }

    /// Exact: every modifier is accounted for, held or not.
    ///
    /// The old chains tested only the modifiers they cared about, so
    /// Cmd+Alt+N opened a tab as readily as Cmd+N. Being exact also makes the
    /// table order-independent, where the chains needed
    /// `Character("s") if shift` written above plain `Character("s")`.
    pub fn matches(self, modifiers: Modifiers) -> bool {
        self.mask() == modifiers
    }
}

/// The key itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    /// A named key - arrows, Tab, Backspace. Unaffected by Shift, so the
    /// modified and unmodified forms agree and either may be matched.
    Named(key::Named),
    /// A latin character, resolved through `Key::to_latin`, which falls back
    /// to the physical key for letters and digits. That is what lets Cmd+Z
    /// work on a Cyrillic or Greek layout. Punctuation has no such fallback -
    /// `/` on a German layout needs Shift+7 and simply will not match, which
    /// is what `keybinds.sample.toml` tells those users to override.
    Latin(char),
}

impl Trigger {
    fn matches(self, key: &Key, physical_key: key::Physical) -> bool {
        match self {
            Trigger::Named(named) => {
                matches!(key.as_ref(), Key::Named(pressed) if pressed == named)
            }
            Trigger::Latin(c) => key.to_latin(physical_key) == Some(c),
        }
    }
}

/// One way to ask for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub mods: Mods,
    pub trigger: Trigger,
}

impl Chord {
    pub const fn new(mods: Mods, trigger: Trigger) -> Self {
        Self { mods, trigger }
    }

    fn matches(
        self,
        key: &Key,
        physical_key: key::Physical,
        modifiers: Modifiers,
    ) -> bool {
        self.mods.matches(modifiers) && self.trigger.matches(key, physical_key)
    }
}

const fn named(mods: Mods, named: key::Named) -> Chord {
    Chord::new(mods, Trigger::Named(named))
}

const fn latin(mods: Mods, c: char) -> Chord {
    Chord::new(mods, Trigger::Latin(c))
}

/// Every action's default chords. An action may have more than one (redo
/// answers to both Cmd+Shift+Z and Cmd+Y); an action absent from this table
/// simply has no default and can still be bound in `keybinds.toml`.
pub const DEFAULT_KEYS: &[(Action, &[Chord])] = &[
    // Application shell.
    (Action::NewTab, &[latin(Mods::COMMAND, 'n')]),
    (Action::OpenFile, &[latin(Mods::COMMAND, 'o')]),
    (Action::SaveFile, &[latin(Mods::COMMAND, 's')]),
    (Action::SaveFileAs, &[latin(Mods::COMMAND_SHIFT, 's')]),
    (Action::CloseActiveTab, &[latin(Mods::COMMAND, 'w')]),
    (
        Action::SelectPreviousTab,
        &[latin(Mods::COMMAND_SHIFT, '[')],
    ),
    (Action::SelectNextTab, &[latin(Mods::COMMAND_SHIFT, ']')]),
    // Ctrl on every platform, Cmd nowhere - the one binding that means the
    // physical Ctrl key rather than "the platform's accelerator".
    (
        Action::SelectPreviousActiveTab,
        &[named(Mods::CONTROL, key::Named::Tab)],
    ),
    (Action::Find, &[latin(Mods::COMMAND, 'f')]),
    (Action::FindNext, &[latin(Mods::COMMAND, 'g')]),
    (Action::FindPrevious, &[latin(Mods::COMMAND_SHIFT, 'g')]),
    // Text editing.
    (
        Action::WordDeleteBackward,
        &[named(Mods::JUMP, key::Named::Backspace)],
    ),
    (
        Action::WordDeleteForward,
        &[named(Mods::JUMP, key::Named::Delete)],
    ),
    (
        Action::DocumentStart,
        &[named(Mods::COMMAND, key::Named::ArrowUp)],
    ),
    (
        Action::SelectDocumentStart,
        &[named(Mods::COMMAND_SHIFT, key::Named::ArrowUp)],
    ),
    (
        Action::DocumentEnd,
        &[named(Mods::COMMAND, key::Named::ArrowDown)],
    ),
    (
        Action::SelectDocumentEnd,
        &[named(Mods::COMMAND_SHIFT, key::Named::ArrowDown)],
    ),
    (Action::Undo, &[latin(Mods::COMMAND, 'z')]),
    (
        Action::Redo,
        &[latin(Mods::COMMAND_SHIFT, 'z'), latin(Mods::COMMAND, 'y')],
    ),
    (Action::ToggleComment, &[latin(Mods::COMMAND, '/')]),
    (Action::DeleteLine, &[latin(Mods::COMMAND, 'd')]),
    (Action::MoveLineUp, &[named(Mods::ALT, key::Named::ArrowUp)]),
    (
        Action::MoveLineDown,
        &[named(Mods::ALT, key::Named::ArrowDown)],
    ),
    (
        Action::CopyLineUp,
        &[named(Mods::COMMAND_ALT, key::Named::ArrowUp)],
    ),
    (
        Action::CopyLineDown,
        &[named(Mods::COMMAND_ALT, key::Named::ArrowDown)],
    ),
];

/// Whether an action wanting `required` may fire in the context in hand.
fn permits(current: Context, required: Context) -> bool {
    required == Context::Always || current == required
}

/// The action a press asks for by default, or `None` if no chord claims it.
///
/// `key` is the *unmodified* logical key - iced's `KeyPressed::key`, not
/// `modified_key` - so Shift+`[` still reads as `[` rather than `{`.
pub fn action_for(
    key: &Key,
    physical_key: key::Physical,
    modifiers: Modifiers,
    context: Context,
) -> Option<Action> {
    DEFAULT_KEYS
        .iter()
        .filter(|(action, _)| permits(context, action.context()))
        .find(|(_, chords)| {
            chords
                .iter()
                .any(|chord| chord.matches(key, physical_key, modifiers))
        })
        .map(|(action, _)| *action)
}

/// Every default chord for an action, for a shortcut sheet or a palette.
pub fn chords_for(action: Action) -> &'static [Chord] {
    DEFAULT_KEYS
        .iter()
        .find(|(candidate, _)| *candidate == action)
        .map(|(_, chords)| *chords)
        .unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn press(
        key: Key,
        code: key::Code,
        modifiers: Modifiers,
    ) -> Option<Action> {
        action_for(
            &key,
            key::Physical::Code(code),
            modifiers,
            Context::EditorFocused,
        )
    }

    fn character(c: &str) -> Key {
        Key::Character(c.to_string().into())
    }

    /// The platform's accelerator, as `Mods::COMMAND` resolves it.
    fn command() -> Modifiers {
        Mods::COMMAND.mask()
    }

    #[test]
    fn no_two_actions_share_a_chord_in_one_context() {
        // `keybinds.sample.toml` admits collisions are
        // "undefined-which-wins - not a supported configuration, just not
        // guarded against". For defaults, this guards against it.
        let mut seen: HashMap<(Modifiers, Trigger, Context), Action> =
            HashMap::new();
        for (action, chords) in DEFAULT_KEYS {
            for chord in *chords {
                let at = (chord.mods.mask(), chord.trigger, action.context());
                if let Some(other) = seen.insert(at, *action) {
                    panic!("{action} and {other} share a chord: {chord:?}");
                }
            }
        }
    }

    #[test]
    fn every_action_with_a_default_appears_once() {
        let mut seen = std::collections::HashSet::new();
        for (action, chords) in DEFAULT_KEYS {
            assert!(seen.insert(*action), "{action} listed twice");
            assert!(!chords.is_empty(), "{action} has an empty chord list");
        }
    }

    #[test]
    fn the_line_commands_keep_their_chords() {
        let alt = Modifiers::ALT;
        // Cmd+Opt on macOS, Ctrl+Alt elsewhere.
        let copy = Mods::COMMAND_ALT.mask();
        let up = Key::Named(key::Named::ArrowUp);
        let down = Key::Named(key::Named::ArrowDown);

        assert_eq!(
            press(up.clone(), key::Code::ArrowUp, alt),
            Some(Action::MoveLineUp)
        );
        assert_eq!(
            press(down.clone(), key::Code::ArrowDown, alt),
            Some(Action::MoveLineDown)
        );
        assert_eq!(
            press(up, key::Code::ArrowUp, copy),
            Some(Action::CopyLineUp)
        );
        assert_eq!(
            press(down, key::Code::ArrowDown, copy),
            Some(Action::CopyLineDown)
        );
    }

    #[test]
    fn redo_answers_to_both_of_its_chords() {
        assert_eq!(
            press(character("z"), key::Code::KeyZ, command()),
            Some(Action::Undo)
        );
        assert_eq!(
            press(
                character("z"),
                key::Code::KeyZ,
                command() | Modifiers::SHIFT
            ),
            Some(Action::Redo)
        );
        assert_eq!(
            press(character("y"), key::Code::KeyY, command()),
            Some(Action::Redo)
        );
    }

    #[test]
    fn shifted_and_unshifted_chords_do_not_collide() {
        // The old chain needed `Character("s") if shift` written above plain
        // `Character("s")`; exact matching makes the order irrelevant.
        assert_eq!(
            press(character("s"), key::Code::KeyS, command()),
            Some(Action::SaveFile)
        );
        assert_eq!(
            press(
                character("s"),
                key::Code::KeyS,
                command() | Modifiers::SHIFT
            ),
            Some(Action::SaveFileAs)
        );
    }

    #[test]
    fn an_extra_modifier_no_longer_counts_as_the_chord() {
        // Deliberate tightening: `modifiers.command()` used to be true with
        // Alt also held, so Cmd+Alt+N opened a tab.
        assert_eq!(
            press(character("n"), key::Code::KeyN, command()),
            Some(Action::NewTab)
        );
        assert_eq!(
            press(character("n"), key::Code::KeyN, command() | Modifiers::ALT),
            None
        );
    }

    #[test]
    fn a_latin_chord_survives_a_cyrillic_layout() {
        // `to_latin` falls back to the physical key for letters, so Cmd+Z is
        // still undo when the layout produces "я".
        assert_eq!(
            press(character("я"), key::Code::KeyZ, command()),
            Some(Action::Undo)
        );
    }

    #[test]
    fn an_editor_action_does_not_fire_with_the_editor_unfocused() {
        let alt = Modifiers::ALT;
        let up = Key::Named(key::Named::ArrowUp);
        assert_eq!(
            action_for(
                &up,
                key::Physical::Code(key::Code::ArrowUp),
                alt,
                Context::Always
            ),
            None
        );
        assert_eq!(
            action_for(
                &up,
                key::Physical::Code(key::Code::ArrowUp),
                alt,
                Context::EditorFocused
            ),
            Some(Action::MoveLineUp)
        );
    }

    #[test]
    fn every_editor_chord_that_used_to_be_hardcoded_still_resolves() {
        // These assertions used to live in `jumppad_textarea`'s
        // `key_binding` tests, against the `if` chains this table replaced.
        // They belong here now that the chords do.
        let cmd = command();
        let jump = Mods::JUMP.mask();
        for (key, code, modifiers, expected) in [
            (character("d"), key::Code::KeyD, cmd, Action::DeleteLine),
            (character("/"), key::Code::Slash, cmd, Action::ToggleComment),
            (
                Key::Named(key::Named::ArrowUp),
                key::Code::ArrowUp,
                cmd,
                Action::DocumentStart,
            ),
            (
                Key::Named(key::Named::ArrowDown),
                key::Code::ArrowDown,
                cmd,
                Action::DocumentEnd,
            ),
            (
                Key::Named(key::Named::ArrowUp),
                key::Code::ArrowUp,
                cmd | Modifiers::SHIFT,
                Action::SelectDocumentStart,
            ),
            (
                Key::Named(key::Named::ArrowDown),
                key::Code::ArrowDown,
                cmd | Modifiers::SHIFT,
                Action::SelectDocumentEnd,
            ),
            (
                Key::Named(key::Named::Backspace),
                key::Code::Backspace,
                jump,
                Action::WordDeleteBackward,
            ),
            (
                Key::Named(key::Named::Delete),
                key::Code::Delete,
                jump,
                Action::WordDeleteForward,
            ),
        ] {
            assert_eq!(
                press(key, code, modifiers),
                Some(expected),
                "{expected} lost its chord"
            );
        }
    }

    #[test]
    fn every_app_chord_that_used_to_be_hardcoded_still_resolves() {
        let cmd = command();
        let shift = cmd | Modifiers::SHIFT;
        for (c, code, modifiers, expected) in [
            ('n', key::Code::KeyN, cmd, Action::NewTab),
            ('o', key::Code::KeyO, cmd, Action::OpenFile),
            ('s', key::Code::KeyS, cmd, Action::SaveFile),
            ('s', key::Code::KeyS, shift, Action::SaveFileAs),
            ('w', key::Code::KeyW, cmd, Action::CloseActiveTab),
            ('f', key::Code::KeyF, cmd, Action::Find),
            ('g', key::Code::KeyG, cmd, Action::FindNext),
            ('g', key::Code::KeyG, shift, Action::FindPrevious),
            (
                '[',
                key::Code::BracketLeft,
                shift,
                Action::SelectPreviousTab,
            ),
            (']', key::Code::BracketRight, shift, Action::SelectNextTab),
        ] {
            assert_eq!(
                action_for(
                    &character(&c.to_string()),
                    key::Physical::Code(code),
                    modifiers,
                    Context::Always
                ),
                Some(expected),
                "{expected} lost its chord"
            );
        }
    }

    #[test]
    fn a_bare_alt_arrow_and_a_copy_arrow_stay_apart() {
        // Move and duplicate differ only by the accelerator, so exact
        // modifier matching is the whole of what keeps them apart.
        assert_eq!(
            press(
                Key::Named(key::Named::ArrowUp),
                key::Code::ArrowUp,
                Modifiers::ALT
            ),
            Some(Action::MoveLineUp)
        );
        assert_eq!(
            press(
                Key::Named(key::Named::ArrowUp),
                key::Code::ArrowUp,
                Mods::COMMAND_ALT.mask()
            ),
            Some(Action::CopyLineUp)
        );
        // And the old chord is now unbound rather than quietly still working.
        assert_eq!(
            press(
                Key::Named(key::Named::ArrowUp),
                key::Code::ArrowUp,
                Modifiers::ALT | Modifiers::SHIFT
            ),
            None
        );
    }

    #[test]
    fn ctrl_tab_is_literal_ctrl_on_every_platform() {
        assert_eq!(
            action_for(
                &Key::Named(key::Named::Tab),
                key::Physical::Code(key::Code::Tab),
                Modifiers::CTRL,
                Context::Always
            ),
            Some(Action::SelectPreviousActiveTab)
        );
    }
}
