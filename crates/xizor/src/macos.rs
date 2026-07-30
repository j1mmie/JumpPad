//! macOS window tweaks that neither iced nor winit exposes.

// `window_handle()` resolves through `iced::window::Window`'s own supertrait
// bound, so `HasWindowHandle` doesn't need to be in scope here.
use iced::window::raw_window_handle::RawWindowHandle;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Bool};

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
    hide_stale_render_layers(window);
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

/// Recomputes the window's shadow.
///
/// macOS derives a translucent window's shadow from a window-server-side
/// snapshot of its content, and that snapshot is also what gets composited
/// behind the window - stale, it shows old content through at
/// `1 - background_alpha`. It lives outside the process, which is why no
/// layer or surface work reaches it. `invalidateShadow` tells the window
/// server to retake it.
pub fn invalidate_window_shadow(window: &dyn iced::window::Window) {
    let Some(view) = ns_view(window) else {
        return;
    };
    // SAFETY: `view` is a live `NSView`; `-window` and `-invalidateShadow` are
    // plain AppKit selectors, called on the main thread.
    unsafe {
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        let _: () = msg_send![ns_window, invalidateShadow];
    }
    log::debug!("xizor: invalidated the window shadow");
}

/// Hides leftover `wgpu` render layers stacked under the live one.
///
/// **This is the burn-in.** `wgpu` renders into a `CAMetalLayer` it appends as a
/// sublayer of the view's root layer, and more than one ends up there - the
/// observed tree has two, the older one still `opaque=true` because it was
/// configured before the surface knew it was translucent. Sublayers draw
/// back-to-front, so that leftover sits *behind* the live layer holding
/// whatever frame it last presented, and shows through at
/// `1 - background_alpha`. Nothing we render reaches it: it isn't the surface
/// we draw to, it isn't the root layer's `contents`, and clearing or redrawing
/// either one leaves it untouched. Only a swapchain rebuild disturbed it, which
/// is why a drag-resize appeared to be the cure.
///
/// The live layer is the last sublayer, since `wgpu` appends and the leftover is
/// older, so everything below it is fair game. Each one is logged before being
/// touched - if this ever hides the wrong layer the window goes blank, and the
/// log says exactly what went.
pub fn hide_stale_render_layers(window: &dyn iced::window::Window) {
    let Some(view) = ns_view(window) else {
        return;
    };
    // SAFETY: `view` is a live `NSView` and these are plain Core Animation
    // selectors, called on the main thread.
    unsafe {
        let root: *mut AnyObject = msg_send![view, layer];
        if root.is_null() {
            return;
        }
        let sublayers: *mut AnyObject = msg_send![root, sublayers];
        if sublayers.is_null() {
            return;
        }
        let count: usize = msg_send![sublayers, count];
        if count < 2 {
            return;
        }
        for index in 0..count - 1 {
            let layer: *mut AnyObject = msg_send![sublayers, objectAtIndex: index];
            log::info!(
                "xizor: hiding stale render layer [{index}] {}",
                describe_layer(layer)
            );
            let _: () = msg_send![layer, setContents: std::ptr::null_mut::<AnyObject>()];
            let _: () = msg_send![layer, setHidden: Bool::YES];
        }
    }
}

/// Dumps the view's Core Animation layer tree.
///
/// The burn-in is behind our content, frozen, and survives having the root
/// layer's `contents` cleared - but a drag-resize does clear it, and the one
/// thing a drag does that nothing else does is make `wgpu` rebuild the
/// swapchain. This prints what is actually in the tree so the next step isn't
/// another guess: how many sublayers there are, whether more than one
/// `CAMetalLayer` has accumulated, and which layers are holding `contents`.
pub fn log_layer_tree(window: &dyn iced::window::Window) {
    let Some(view) = ns_view(window) else {
        return;
    };
    // SAFETY: `view` is a live `NSView` and these are all plain AppKit and Core
    // Animation getters, called on the main thread.
    unsafe {
        let root: *mut AnyObject = msg_send![view, layer];
        if root.is_null() {
            log::info!("xizor: layer tree: view is not layer-backed");
            return;
        }
        log::info!("xizor: layer tree: root {}", describe_layer(root));

        let sublayers: *mut AnyObject = msg_send![root, sublayers];
        if sublayers.is_null() {
            log::info!("xizor: layer tree: root has no sublayers");
            return;
        }
        let count: usize = msg_send![sublayers, count];
        log::info!("xizor: layer tree: {count} sublayer(s)");
        for index in 0..count {
            let layer: *mut AnyObject = msg_send![sublayers, objectAtIndex: index];
            log::info!("xizor: layer tree:   [{index}] {}", describe_layer(layer));
        }
    }
}

/// # Safety
///
/// `layer` must be a live `CALayer`.
unsafe fn describe_layer(layer: *mut AnyObject) -> String {
    let class: *const AnyClass = msg_send![layer, class];
    let name = if class.is_null() {
        "<null class>".to_string()
    } else {
        (*class).name().to_string_lossy().into_owned()
    };

    let contents: *mut AnyObject = msg_send![layer, contents];
    let opaque: Bool = msg_send![layer, isOpaque];
    let hidden: Bool = msg_send![layer, isHidden];

    format!(
        "{name} contents={} opaque={} hidden={}",
        if contents.is_null() { "none" } else { "SET" },
        opaque.as_bool(),
        hidden.as_bool(),
    )
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
