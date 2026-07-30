//! Global (system-wide) hotkey registration and listening - lets the visor
//! keybind toggle the window even while another application has focus.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use iced::Subscription;

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
                eprintln!("jumppad: couldn't create global hotkey manager: {err}");
                return None;
            }
        };
        if let Err(err) = manager.register(hotkey) {
            eprintln!(
                "jumppad: couldn't register global hotkey {hotkey} \
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
    iced::stream::channel(16, |output: iced::futures::channel::mpsc::Sender<GlobalHotKeyEvent>| async move {
        GlobalHotKeyEvent::set_event_handler(Some(move |event| {
            // `try_send` needs `&mut`; cloning the sender is cheap.
            let mut output = output.clone();
            let _ = output.try_send(event);
        }));
        // Keeps the stream alive for the life of the app - the event
        // handler above is what actually delivers events.
        std::future::pending::<()>().await
    })
}
