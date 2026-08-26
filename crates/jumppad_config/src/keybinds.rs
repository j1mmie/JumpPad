use std::collections::HashMap;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use serde::{Deserialize, Serialize};

use crate::keybind_overrides::{self, ResolvedKeybind};

/// JumpPad's global keybindings, loaded from `keybinds.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindsConfig {
    /// Shows/hides the visor from anywhere, even without focus.
    pub toggle: HotKey,
    /// Command-name -> key-chord overrides for JumpPad's in-app shortcuts,
    /// e.g. `new_tab = "control+alt+n"` - takes precedence over the
    /// hardcoded default when present. An unrecognized name is silently
    /// ignored (logged once at startup).
    #[serde(default)]
    pub overrides: HashMap<String, HotKey>,
}

impl Default for KeybindsConfig {
    fn default() -> Self {
        Self {
            toggle: HotKey::new(Some(Modifiers::CONTROL), Code::Backquote),
            overrides: HashMap::new(),
        }
    }
}

impl KeybindsConfig {
    /// Resolves `overrides` into iced-native types, ready to compare against incoming key events.
    pub fn resolved_overrides(&self) -> HashMap<String, ResolvedKeybind> {
        keybind_overrides::resolved_overrides(&self.overrides)
    }
}
