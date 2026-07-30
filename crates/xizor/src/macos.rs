//! macOS window tweaks that neither iced nor winit exposes.

// `window_handle()` resolves through `iced::window::Window`'s own supertrait
// bound, so `HasWindowHandle` doesn't need to be in scope here.
use iced::window::raw_window_handle::RawWindowHandle;
use objc2::msg_send;
use objc2::runtime::AnyObject;

/// `NSViewLayerContentsRedrawOnSetNeedsDisplay`. Deliberately not
/// `...RedrawNever`, which would stop AppKit calling `drawRect:` - winit's view
/// drives its whole redraw dispatch from there, so the app would simply stop
/// painting.
const REDRAW_ON_SET_NEEDS_DISPLAY: isize = 1;

/// Stops AppKit caching the window's contents into the view's root layer.
///
/// **The macOS burn-in.** `wgpu` renders into a `CAMetalLayer` added as a
/// *sublayer* of the view's AppKit-managed root layer, and deliberately leaves
/// the root layer's properties alone ("we would like to give the user full
/// control over them" - `wgpu_hal::metal::surface`). The default it leaves in
/// place is `NSViewLayerContentsRedrawDuringViewResize`, which has AppKit
/// snapshot the view's rendered contents into that root layer across a resize.
/// On a translucent window the snapshot sits *behind* the Metal sublayer and
/// shows through at `1 - background_alpha`: a frozen copy of whatever was on
/// screen at the last drag-resize, still there when the real content changes
/// underneath it. Nothing we draw touches it and no number of redraws clears
/// it, because it isn't in the surface we draw to.
///
/// Tried and insufficient, so it isn't re-attempted: `NSWindow`'s
/// `preservesContentDuringLiveResize`, which sounds like the same thing but
/// caches at the window level, not the view's layer - setting it to `NO`
/// changed nothing.
pub fn disable_resize_content_caching(window: &dyn iced::window::Window) {
    let Some(view) = ns_view(window) else {
        return;
    };
    // SAFETY: see `with_root_layer`.
    unsafe {
        let _: () = msg_send![view, setLayerContentsRedrawPolicy: REDRAW_ON_SET_NEEDS_DISPLAY];
    }
    clear_root_layer_snapshot(window);
    log::info!("xizor: disabled AppKit resize-content caching on the view's root layer");
}

/// Drops whatever AppKit has cached in the view's root layer.
///
/// Setting the policy above is meant to stop the snapshot being taken at all,
/// but it demonstrably doesn't on its own, so this is also called after a
/// resize (when the snapshot is created) and after a tab switch (when a stale
/// one becomes visible). If the burn-in outlives *this*, the root layer's
/// `contents` is not where it lives and the search moves outside the view.
pub fn clear_root_layer_snapshot(window: &dyn iced::window::Window) {
    let Some(view) = ns_view(window) else {
        return;
    };
    // SAFETY: `view` is a live `NSView`, `-layer` and `-setContents:` are plain
    // AppKit and Core Animation selectors, and iced runs window actions on the
    // main thread, which is where AppKit requires them.
    unsafe {
        let root_layer: *mut AnyObject = msg_send![view, layer];
        if root_layer.is_null() {
            log::warn!("xizor: NSView is not layer-backed; nothing to clear");
            return;
        }
        let _: () = msg_send![root_layer, setContents: std::ptr::null_mut::<AnyObject>()];
    }
    log::debug!("xizor: cleared the view root layer's cached contents");
}

fn ns_view(window: &dyn iced::window::Window) -> Option<*mut AnyObject> {
    let Ok(handle) = window.window_handle() else {
        log::warn!("xizor: no window handle; leaving AppKit's layers alone");
        return None;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.ns_view.as_ptr().cast())
}
