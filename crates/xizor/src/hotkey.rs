//! Global (system-wide) hotkey registration and listening - this is what
//! lets the visor keybind (see `xizor_config::KeybindsConfig`) toggle the
//! window even while some other application has focus, unlike iced's own
//! `keyboard::on_key_press` (used for the in-app shortcuts in `app.rs`),
//! which only ever fires while xizor's own window is focused.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use iced::Subscription;

use crate::app::Message;

/// Owns the registration for the visor's global toggle hotkey. Kept alive
/// for the app's lifetime as a field on `XizorApp` - dropping `manager`
/// unregisters the hotkey with the OS, so this can't be a temporary.
pub struct Hotkey {
    // Never read again after construction, but must stay alive - see the
    // struct doc comment.
    #[allow(dead_code)]
    manager: GlobalHotKeyManager,
    hotkey: HotKey,
}

impl Hotkey {
    /// Registers `hotkey` as a global shortcut. Returns `None` (logging why)
    /// on failure - e.g. the combo is already claimed by another
    /// application - rather than treating that as fatal; the app just runs
    /// without a working toggle key in that case.
    pub fn register(hotkey: HotKey) -> Option<Self> {
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(err) => {
                eprintln!("xizor: couldn't create global hotkey manager: {err}");
                return None;
            }
        };
        if let Err(err) = manager.register(hotkey) {
            eprintln!(
                "xizor: couldn't register global hotkey {hotkey} \
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

/// Subscribes to every global hotkey event (of any registered hotkey, though
/// this app only ever registers the one visor toggle). `update()` is
/// responsible for filtering to `HotKeyState::Pressed` and the id of the
/// hotkey it actually cares about - see `Message::HotkeyEvent`.
pub fn subscription() -> Subscription<Message> {
    Subscription::run(hotkey_events).map(Message::HotkeyEvent)
}

/// A plain (non-capturing) fn, as required by `Subscription::run`'s
/// `fn() -> S` signature - iced uses the function pointer itself as the
/// subscription's identity, so this only ever starts once for the life of
/// the app, matching `GlobalHotKeyEvent::set_event_handler`'s "set once"
/// contract (it's backed by a `OnceCell` and silently no-ops on a second
/// call).
fn hotkey_events() -> impl iced::futures::Stream<Item = GlobalHotKeyEvent> {
    // The `mpsc::Sender`'s item type can no longer be inferred from context
    // alone since iced 0.14 changed `stream::channel`'s closure parameter to
    // `impl AsyncFnOnce` - spelling it out here is enough for the rest of
    // the function to infer normally.
    iced::stream::channel(16, |output: iced::futures::channel::mpsc::Sender<GlobalHotKeyEvent>| async move {
        GlobalHotKeyEvent::set_event_handler(Some(move |event| {
            // `try_send` needs `&mut`; cloning a sender is cheap (it shares
            // the same underlying queue) and sidesteps that without this
            // `Fn` callback needing interior mutability of its own.
            let mut output = output.clone();
            let _ = output.try_send(event);
        }));
        // The event handler above is what actually delivers events from
        // here on; this just keeps the stream (and the app's subscription
        // to it) alive for as long as the app runs.
        std::future::pending::<()>().await
    })
}
