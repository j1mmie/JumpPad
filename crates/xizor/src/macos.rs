//! macOS window tweaks that neither iced nor winit exposes.
//!
//! Both exist for one bug: the translucent-window "burn-in" (see AGENTS.md).
//! The ghost lives in the window server's shadow-content cache, *outside the
//! process* - which is why clearing surfaces, layer contents, or layer trees
//! never touched it, and why only things that made the window server retake
//! its snapshot (a drag-resize, a 1px programmatic resize) appeared to help.

// `window_handle()` resolves through `iced::window::Window`'s own supertrait
// bound, so `HasWindowHandle` doesn't need to be in scope here.
use iced::window::raw_window_handle::RawWindowHandle;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Bool};

/// One-time setup for a translucent window.
pub fn apply_translucent_window_fixes(window: &dyn iced::window::Window) {
    hide_stale_render_layers(window);
    log::info!("xizor: applied translucent-window fixes");
}

/// Recomputes the window's shadow from the window's *current* content.
///
/// **This is the burn-in fix.** The window server derives a translucent
/// window's shadow from a cached copy of its content, and composites that
/// cache behind the window - so once the real content changes, the stale copy
/// shows through at `1 - background_alpha`. Confirmed by elimination: with the
/// shadow disabled outright the ghost is gone, while every layer in the
/// process was dumped and found clean.
///
/// **Timing is the trap.** Calling this the moment the content changes
/// re-caches the *outgoing* frame - it must run after the new frame has
/// actually presented, which is why the caller counts a few presented frames
/// first.
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
/// `wgpu` renders into a `CAMetalLayer` it appends as a sublayer of the view's
/// root layer, and more than one ends up there - the observed tree has two,
/// the older one still `opaque=true` because it was configured before the
/// surface knew it was translucent. Sublayers draw back-to-front, so on a
/// translucent window the leftover's last frame would show through the live
/// layer. (It was not the burn-in - that was the shadow cache above - but an
/// opaque layer behind a translucent one is a straightforward compositing
/// hazard.) The live layer is the last sublayer, since `wgpu` appends and the
/// leftover is older; each hidden layer is logged, so if this ever hides the
/// wrong one the window goes blank and the log names it.
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
