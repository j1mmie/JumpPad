//! macOS window tweaks that neither iced nor winit exposes.

use iced::window::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use objc2::msg_send;
use objc2::runtime::{AnyObject, Bool};

/// Stops AppKit preserving the window's contents across a live resize.
///
/// **The macOS burn-in.** `NSWindow.preservesContentDuringLiveResize` caches
/// "the content of views that have not changed" while the user drags a window
/// edge. On a translucent window that cached copy stays *behind* the live
/// content and shows through at `1 - background_alpha`: a frozen snapshot of
/// whatever was on screen at the last resize, which reappears as soon as the
/// real content changes underneath it (switch tabs and the old document is
/// still there). It survives any number of redraws, because nothing we draw
/// touches it, and only a fresh drag replaces it - a programmatic resize isn't
/// a live resize and doesn't.
pub fn disable_live_resize_content_preservation(window: &dyn iced::window::Window) {
    let Ok(handle) = window.window_handle() else {
        log::warn!("xizor: no window handle; leaving live-resize preservation alone");
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };

    // SAFETY: the handle guarantees a live `NSView`, `-window` and
    // `-setPreservesContentDuringLiveResize:` are plain AppKit selectors, and
    // iced runs window actions on the main thread, which is where AppKit
    // requires them.
    unsafe {
        let view: *mut AnyObject = handle.ns_view.as_ptr().cast();
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            log::warn!("xizor: NSView has no NSWindow yet; burn-in fix not applied");
            return;
        }
        let _: () = msg_send![ns_window, setPreservesContentDuringLiveResize: Bool::NO];
    }
    log::info!("xizor: disabled NSWindow live-resize content preservation");
}
