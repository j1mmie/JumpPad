use editor_core::EditorMessage;
use jumppad_actions::Action;

use crate::text_editor::{Binding, KeyPress, Motion, Status};

/// Turns a key press into the [`Action`] it asks for, if any.
///
/// Supplied by the app rather than built here: resolving a press means
/// knowing both the default chords (`jumppad_keybinds`) and the user's
/// `keybinds.toml` overrides (`jumppad_config`), and this crate depends on
/// neither - the same reason `build_editor_overrides` already lived in the
/// app. Handing down a resolver keeps that boundary and puts
/// override-beats-default precedence in one place instead of two.
pub type KeyResolver = dyn Fn(&KeyPress) -> Option<Action> + Send + Sync;

fn word_delete_backward() -> Binding<EditorMessage> {
    Binding::Sequence(vec![
        Binding::Select(Motion::Left.widen()),
        Binding::Backspace,
    ])
}

fn word_delete_forward() -> Binding<EditorMessage> {
    Binding::Sequence(vec![
        Binding::Select(Motion::Right.widen()),
        Binding::Delete,
    ])
}

/// How this crate performs an [`Action`], or `None` for one it doesn't own -
/// every `Action::App`, which the shell handles instead.
///
/// `jumppad`'s wiring test asserts every action is claimed by exactly one of
/// this and its own `message_for`, so an action added to the registry and
/// forgotten here fails the build rather than going quiet.
pub fn binding_for(action: Action) -> Option<Binding<EditorMessage>> {
    let custom = |message| Some(Binding::Custom(message));
    match action {
        Action::WordDeleteBackward => Some(word_delete_backward()),
        Action::WordDeleteForward => Some(word_delete_forward()),
        Action::DocumentStart => Some(Binding::Move(Motion::DocumentStart)),
        Action::SelectDocumentStart => {
            Some(Binding::Select(Motion::DocumentStart))
        }
        Action::DocumentEnd => Some(Binding::Move(Motion::DocumentEnd)),
        Action::SelectDocumentEnd => Some(Binding::Select(Motion::DocumentEnd)),
        Action::Undo => custom(EditorMessage::Undo),
        Action::Redo => custom(EditorMessage::Redo),
        Action::Indent => custom(EditorMessage::Indent),
        Action::Outdent => custom(EditorMessage::Outdent),
        Action::ToggleComment => custom(EditorMessage::ToggleComment),
        Action::DeleteLine => custom(EditorMessage::DeleteLine),
        Action::MoveLineUp => custom(EditorMessage::MoveLineUp),
        Action::MoveLineDown => custom(EditorMessage::MoveLineDown),
        Action::CopyLineUp => custom(EditorMessage::CopyLineUp),
        Action::CopyLineDown => custom(EditorMessage::CopyLineDown),
        _ => None,
    }
}

/// Turns a key press into a binding.
///
/// Two tiers, first match wins:
/// 1. `resolve` - the app's key -> [`Action`] map, which already merges the
///    user's `keybinds.toml` overrides over the default chords. An action
///    this crate doesn't perform falls through rather than swallowing the
///    press, so the shell still sees its own shortcuts.
/// 2. iced's own stock default dispatch.
pub(crate) fn key_binding(
    press: KeyPress,
    resolve: &KeyResolver,
) -> Option<Binding<EditorMessage>> {
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }

    if let Some(binding) = resolve(&press).and_then(binding_for) {
        return Some(binding);
    }

    // Tier 2: iced's own stock dispatch - with one correction. On macOS,
    // holding Cmd doesn't suppress character production, so an
    // unrecognized Cmd+<letter> would otherwise get typed into the
    // document *and* mark the event captured, hiding it from app-level
    // shortcuts. Discard an `Insert` produced while `command()` is held so
    // it falls through unhandled instead (a no-op on other platforms,
    // where `command()` is Ctrl and already suppresses character production).
    let command_held = press.modifiers.command();
    match Binding::from_key_press(press) {
        Some(Binding::Insert(_)) if command_held => None,
        other => other,
    }
}
