//! macOS window tweaks that neither iced nor winit exposes.

// `window_handle()` resolves through `iced::window::Window`'s own supertrait
// bound, so `HasWindowHandle` doesn't need to be in scope here.
use iced::window::raw_window_handle::RawWindowHandle;
use objc2::msg_send;
use objc2::runtime::AnyObject;

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
    let Ok(handle) = window.window_handle() else {
        log::warn!(
            "jumppad: no window handle; leaving the window shadow alone"
        );
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: the handle guarantees a live `NSView`; `-window` and
    // `-invalidateShadow` are plain AppKit selectors, and iced runs window
    // actions on the main thread, which is where AppKit requires them.
    unsafe {
        let view: *mut AnyObject = handle.ns_view.as_ptr().cast();
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        let _: () = msg_send![ns_window, invalidateShadow];
    }
    log::debug!("jumppad: invalidated the window shadow");
}
