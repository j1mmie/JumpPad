//! macOS window tweaks that neither iced nor winit exposes.

// `window_handle()` resolves through `iced::window::Window`'s own supertrait
// bound, so `HasWindowHandle` doesn't need to be in scope here.
use std::ffi::CStr;

use iced::window::raw_window_handle::RawWindowHandle;
use jumppad_config::Appearance;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

/// The two appearances everything here resolves to. AppKit has more of them -
/// vibrant and high-contrast variants - and asks each one which of these it is
/// closest to.
const LIGHT: &CStr = c"NSAppearanceNameAqua";
const DARK: &CStr = c"NSAppearanceNameDarkAqua";

/// Recomputes the window's shadow from the window's *current* content.
///
/// **The translucent-window burn-in fix.** The window server derives a
/// translucent window's shadow from a cached copy of its content, and
/// composites that cache behind the window - so once the real content changes,
/// the stale copy shows through at `1 - background_alpha` (see AGENTS.md for
/// the full elimination that pinned this down). `invalidateShadow` makes the
/// window server retake the cache.
///
/// **Timing is the trap.** Calling this the moment the content changes
/// re-caches the *outgoing* frame - it must run after the new frame has
/// actually presented, which is why the caller counts a few presented frames
/// first (`SHADOW_REFRESH_FRAMES`).
pub fn invalidate_window_shadow(window: &dyn iced::window::Window) {
    let Some(ns_window) = ns_window_of(window, "window shadow") else {
        return;
    };
    // SAFETY: `ns_window_of` returned a live `NSWindow`, `-invalidateShadow`
    // is a plain AppKit selector, and iced runs window actions on the main
    // thread, which is where AppKit requires them.
    unsafe {
        let _: () = msg_send![ns_window, invalidateShadow];
    }
    log::debug!("jumppad: invalidated the window shadow");
}

/// Pins the window's light/dark appearance to `pinned`, or leaves it to
/// follow the OS (`None`).
///
/// **Clearing it is what lets a mid-session light/dark switch reach the app
/// at all.** winit reports a switch by watching the window's
/// `effectiveAppearance`, and deliberately says nothing while an appearance
/// is pinned - a pinned one only ever changes because the app changed it.
/// iced pins one from the theme as every window opens, so with `[mode]
/// detection = "auto"` the switch never arrived and the theme never moved
/// (see AGENTS.md).
///
/// Pinning is still right when the config names a slot: the theme is not
/// going to follow the OS, and the title bar should not either.
pub fn pin_appearance(
    window: &dyn iced::window::Window,
    pinned: Option<Appearance>,
) {
    let Some(ns_window) = ns_window_of(window, "window appearance") else {
        return;
    };
    // SAFETY: as above. `-setAppearance:` takes an `NSAppearance` or `nil`,
    // and `appearance_named` returns exactly that.
    unsafe {
        let appearance = match pinned {
            Some(Appearance::Light) => appearance_named(LIGHT),
            Some(Appearance::Dark) => appearance_named(DARK),
            None => std::ptr::null_mut(),
        };
        let _: () = msg_send![ns_window, setAppearance: appearance];
    }
    log::debug!("jumppad: window appearance pinned to {pinned:?}");
}

/// What the OS's light/dark setting says right now.
///
/// Read from the *application*, not from a window: a window with a pinned
/// appearance would answer with its pin. `None` if AppKit gives an answer
/// that is neither light nor dark, which it should not.
pub fn system_appearance() -> Option<Appearance> {
    // SAFETY: `+sharedApplication` and `-effectiveAppearance` are plain
    // AppKit selectors, on the main thread as AppKit requires.
    unsafe {
        let app: *mut AnyObject =
            msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return None;
        }
        let appearance: *mut AnyObject = msg_send![app, effectiveAppearance];

        appearance_of(appearance)
    }
}

/// The `NSWindow` behind an iced window, or `None` with a line in the log
/// saying which tweak is being skipped.
fn ns_window_of(
    window: &dyn iced::window::Window,
    what: &str,
) -> Option<*mut AnyObject> {
    let Ok(handle) = window.window_handle() else {
        log::warn!("jumppad: no window handle; leaving the {what} alone");
        return None;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: the handle guarantees a live `NSView`, and `-window` is a plain
    // AppKit selector.
    let ns_window: *mut AnyObject = unsafe {
        let view: *mut AnyObject = handle.ns_view.as_ptr().cast();
        msg_send![view, window]
    };

    (!ns_window.is_null()).then_some(ns_window)
}

/// Which of the two plain appearances an `NSAppearance` is closest to.
///
/// The same question winit asks of the same object, by the same route, so a
/// vibrant or high-contrast variant lands on the side winit would have put
/// it on.
///
/// # Safety
///
/// `appearance` must be an `NSAppearance` or null.
unsafe fn appearance_of(appearance: *mut AnyObject) -> Option<Appearance> {
    if appearance.is_null() {
        return None;
    }
    unsafe {
        let names = [ns_string(LIGHT), ns_string(DARK)];
        let list: *mut AnyObject = msg_send![
            class!(NSArray),
            arrayWithObjects: names.as_ptr(),
            count: names.len()
        ];
        let best: *mut AnyObject =
            msg_send![appearance, bestMatchFromAppearancesWithNames: list];
        if best.is_null() {
            return None;
        }
        let is_dark: bool = msg_send![best, isEqualToString: names[1]];

        Some(if is_dark {
            Appearance::Dark
        } else {
            Appearance::Light
        })
    }
}

/// # Safety
///
/// `name` must be one of AppKit's appearance names.
unsafe fn appearance_named(name: &CStr) -> *mut AnyObject {
    unsafe { msg_send![class!(NSAppearance), appearanceNamed: ns_string(name)] }
}

/// # Safety
///
/// `text` must stay valid for the length of the call; the `NSString` copies
/// it.
unsafe fn ns_string(text: &CStr) -> *mut AnyObject {
    unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] }
}
