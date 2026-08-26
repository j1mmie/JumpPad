//! Global (system-wide) hotkey registration and listening - lets the visor
//! keybind toggle the window even while another application has focus.
//!
//! Also home to the app-command hotkey resolution glue: turning a raw key
//! press into an [`Action`], and an `Action` into the [`Message`] the app
//! shell reacts to.

use std::collections::HashMap;
use std::sync::Arc;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use iced::Subscription;
use iced::keyboard::{self, key};

use jumppad_actions::{Action, Context};

use crate::app::Message;

/// Owns the registration for the visor's global toggle hotkey - dropping
/// `manager` unregisters it with the OS, so this must stay alive for the app's lifetime.
pub struct Hotkey {
    #[allow(dead_code)] // never read again, but must stay alive
    manager: GlobalHotKeyManager,
    hotkey: HotKey,
}

impl Hotkey {
    /// Registers `hotkey` as a global shortcut. Returns `None` on failure
    /// (e.g. the combo is already claimed) rather than treating it as fatal.
    pub fn register(hotkey: HotKey) -> Option<Self> {
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(err) => {
                log::warn!("couldn't create global hotkey manager: {err}");
                return None;
            }
        };
        if let Err(err) = manager.register(hotkey) {
            log::warn!(
                "couldn't register global hotkey {hotkey} \
                 (already in use by another application?): {err}"
            );
            return None;
        }
        Some(Self { manager, hotkey })
    }

    pub fn id(&self) -> u32 {
        self.hotkey.id()
    }
}

/// Subscribes to every global hotkey event - `update()` filters down to the
/// press of the specific hotkey it cares about.
pub fn subscription() -> Subscription<Message> {
    Subscription::run(hotkey_events).map(Message::HotkeyEvent)
}

/// A plain fn, as required by `Subscription::run` - iced uses the function
/// pointer as the subscription's identity, so this only starts once.
fn hotkey_events() -> impl iced::futures::Stream<Item = GlobalHotKeyEvent> {
    iced::stream::channel(
        16,
        |output: iced::futures::channel::mpsc::Sender<GlobalHotKeyEvent>| async move {
            GlobalHotKeyEvent::set_event_handler(Some(move |event| {
                // `try_send` needs `&mut`; cloning the sender is cheap.
                let mut output = output.clone();
                let _ = output.try_send(event);
            }));
            // Keeps the stream alive for the life of the app - the event
            // handler above is what actually delivers events.
            std::future::pending::<()>().await
        },
    )
}

/// App-level command names a `keybinds.toml` override may target.
/// A user's `keybinds.toml` remaps, resolved to physical key + modifiers.
///
/// Physical rather than logical so a remap lands on the same *place* on every
/// layout, which is what lets a German-layout user reach a chord their layout
/// cannot type directly.
pub(crate) type KeyOverrides = HashMap<(keyboard::Modifiers, key::Code), Action>;

/// Resolves `keybinds.toml`'s overrides into a lookup keyed by physical key +
/// modifiers. One table for both layers now that both speak `Action` - it
/// used to be two, `build_app_overrides` and `build_editor_overrides`, each
/// carrying its own copy of the command-name list.
pub(crate) fn build_key_overrides(
    keybinds: &jumppad_config::KeybindsConfig,
) -> KeyOverrides {
    let mut map = HashMap::new();
    for (name, resolved) in keybinds.resolved_overrides() {
        if let Some(action) = Action::from_name(&name) {
            map.insert((resolved.modifiers, resolved.code), action);
        }
    }
    map
}

/// How the shell performs an [`Action`], or `None` for one it doesn't own -
/// every `Action::Editor`, which the text widget handles instead.
///
/// The other half of `jumppad_textarea::binding_for`; between them they must
/// cover every action, which `every_action_is_wired_to_something` checks.
pub(crate) fn message_for(action: Action) -> Option<Message> {
    match action {
        Action::NewTab => Some(Message::NewTab),
        Action::OpenFile => Some(Message::OpenFile),
        Action::SaveFile => Some(Message::SaveFile),
        Action::SaveFileAs => Some(Message::SaveFileAs),
        Action::CloseActiveTab => Some(Message::CloseActiveTab),
        Action::SelectPreviousTab => Some(Message::SelectPreviousTab),
        Action::SelectNextTab => Some(Message::SelectNextTab),
        Action::SelectPreviousActiveTab => {
            Some(Message::SelectPreviousActiveTab)
        }
        Action::Find => Some(Message::OpenFind),
        Action::FindNext => Some(Message::FindNext),
        Action::FindPrevious => Some(Message::FindPrevious),
        _ => None,
    }
}

/// The action a press asks for: a user override first, then the shipped
/// default chords. Shared by the shell and the editor widget, so the two can
/// no longer disagree about which wins.
fn resolve_action(
    key: &keyboard::Key,
    physical_key: key::Physical,
    modifiers: keyboard::Modifiers,
    context: Context,
    overrides: &KeyOverrides,
) -> Option<Action> {
    if let key::Physical::Code(code) = physical_key
        && let Some(&action) = overrides.get(&(modifiers, code))
        && (action.context() == Context::Always || action.context() == context)
    {
        return Some(action);
    }
    jumppad_keybinds::action_for(key, physical_key, modifiers, context)
}

/// The resolver handed to every `TextArea`, closing over the overrides of the
/// moment. Rebuilt and re-injected on a `keybinds.toml` reload.
pub(crate) fn build_key_resolver(
    overrides: Arc<KeyOverrides>,
) -> Arc<jumppad_textarea::KeyResolver> {
    Arc::new(move |press: &jumppad_textarea::KeyPress| {
        resolve_action(
            &press.key,
            press.physical_key,
            press.modifiers,
            Context::EditorFocused,
            &overrides,
        )
    })
}

/// Logs (doesn't fail) any `keybinds.toml` override whose command name
/// isn't recognized by either layer - a cheap typo-catcher, not a
/// validation framework.
pub(crate) fn warn_unrecognized_overrides(
    overrides: &HashMap<String, global_hotkey::hotkey::HotKey>,
) {
    for name in overrides.keys() {
        if Action::from_name(name).is_none() {
            log::warn!(
                "keybinds.toml overrides an unrecognized command {name:?}, ignoring"
            );
        }
    }
}

pub(crate) fn handle_hotkey(
    key: keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical_key: key::Physical,
    overrides: &KeyOverrides,
) -> Option<Message> {
    // `Context::Always`: the shell's own shortcuts, which do not require the
    // editor to hold focus. An editor action resolved here returns `None`
    // from `message_for` and falls through to the widget, as it always has.
    let action = resolve_action(
        &key,
        physical_key,
        modifiers,
        Context::Always,
        overrides,
    )?;

    message_for(action)
}
