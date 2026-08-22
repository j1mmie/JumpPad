//! macOS window tweaks that neither iced nor winit exposes.

// `window_handle()` resolves through `iced::window::Window`'s own supertrait
// bound, so `HasWindowHandle` doesn't need to be in scope here.
use std::ffi::{CStr, CString, c_void};
use std::sync::OnceLock;

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

/// Frosts what shows through a translucent window, to `radius`, or takes the
/// frosting off at `0` - per `[themes] background.blur`.
///
/// Nothing in this process can reach the desktop's pixels, so the blurring is
/// the window server's. `CGSSetWindowBackgroundBlurRadius` hands it a radius
/// for one window and it blurs whatever that window sits over, wherever the
/// window's own pixels are see-through.
///
/// **It is a private symbol, and the only one JumpPad uses.** Apple has never
/// declared it in a public header, and the public route - an
/// `NSVisualEffectView` behind the content - is fixed at the system's own
/// blur with no knob on it at all. So every app that offers a blur *amount*
/// on macOS reaches for this one; kitty's `background_blur`, the setting this
/// one matches, is the same call. Looked up at runtime rather than linked,
/// because a private symbol that disappears should cost this feature and not
/// the app's ability to start: a macOS without it says so once, and draws an
/// unfrosted window.
///
/// **The window's background color has to stop being fully clear.** The
/// window server does not blur behind a pixel whose alpha is zero, and a
/// decorated window's rounded corners are exactly that - so the corners keep
/// the sharp desktop while the rest of the window frosts, and the shadow
/// there reads as a hard edge. winit sets that color to `clearColor` for
/// every transparent window; a hair of black over it is enough to bring the
/// corners into the blur, and is invisible at any alpha. Put back when the
/// blur goes off, so the window is left the way winit had it.
pub fn set_window_blur(window: &dyn iced::window::Window, radius: u32) {
    let Some(ns_window) = ns_window_of(window, "the window blur") else {
        return;
    };
    let Some(blur) = window_server_blur() else {
        log::warn!(
            "jumppad: no CGSSetWindowBackgroundBlurRadius on this macOS; \
             leaving [themes] background.blur unhonored"
        );
        return;
    };
    let radius = radius.min(MAX_BLUR_RADIUS);

    // SAFETY: `ns_window_of` returned a live `NSWindow`, the selectors are
    // plain AppKit ones, and iced runs window actions on the main thread,
    // where AppKit and the window server's per-thread connection both want
    // to be called. `-windowNumber` is what identifies this window to the
    // window server, which is the id `blur.set_radius` is asking about.
    let result = unsafe {
        let color: *mut AnyObject = match radius {
            0 => msg_send![class!(NSColor), clearColor],
            _ => msg_send![
                class!(NSColor),
                colorWithWhite: 0.0f64,
                alpha: CORNER_INK,
            ],
        };
        let _: () = msg_send![ns_window, setBackgroundColor: color];

        let number: isize = msg_send![ns_window, windowNumber];

        (blur.set_radius)((blur.connection)(), number, radius as i32)
    };

    if result == 0 {
        log::debug!("jumppad: window blur radius set to {radius}");
    } else {
        log::warn!(
            "jumppad: the window server refused a blur radius of {radius} ({result})"
        );
    }
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

/// The blur radius past which the window server is asked for nothing more.
/// Not a hard limit of its own - it is where the cost starts outrunning any
/// visible difference, which is the same ceiling kitty documents for the
/// setting this one matches.
const MAX_BLUR_RADIUS: u32 = 64;

/// The alpha of the black laid under a frosted window, to keep its corners
/// from being pixels the window server skips - see `set_window_blur`. Small
/// enough that it changes no color the app draws over it.
const CORNER_INK: f64 = 0.001;

/// `CGSDefaultConnectionForThread` - the calling thread's handle to the
/// window server.
type WindowServerConnection = unsafe extern "C" fn() -> *mut c_void;

/// `CGSSetWindowBackgroundBlurRadius(connection, window_number, radius)`,
/// returning a `CGError` that is zero on success.
type SetBlurRadius = unsafe extern "C" fn(*mut c_void, isize, i32) -> i32;

/// The two window-server entry points a blur radius needs, looked up once.
struct WindowServerBlur {
    /// Called per use rather than stored, since it answers per thread and
    /// this crate should not be the one assuming which thread that is.
    connection: WindowServerConnection,
    set_radius: SetBlurRadius,
}

/// Both symbols, or `None` if either has gone - they are only useful
/// together, and looking them up costs a `dlsym` apiece the first time.
fn window_server_blur() -> Option<&'static WindowServerBlur> {
    static FOUND: OnceLock<Option<WindowServerBlur>> = OnceLock::new();

    FOUND
        .get_or_init(|| {
            let connection = symbol("CGSDefaultConnectionForThread")?;
            let set_radius = symbol("CGSSetWindowBackgroundBlurRadius")?;

            // SAFETY: each pointer came back under the name of the function
            // it is being read as, and each signature is the one Apple's own
            // callers use. A name this process has no definition for came
            // back `None` above rather than reaching here.
            unsafe {
                Some(WindowServerBlur {
                    connection: std::mem::transmute::<
                        *mut c_void,
                        WindowServerConnection,
                    >(connection),
                    set_radius: std::mem::transmute::<
                        *mut c_void,
                        SetBlurRadius,
                    >(set_radius),
                })
            }
        })
        .as_ref()
}

/// One symbol from whatever this process has already loaded, by name. A name
/// nothing loaded defines is `None` - including one this app got wrong, since
/// `dlsym` is not told what it is looking for beyond the string.
fn symbol(name: &str) -> Option<*mut c_void> {
    let name = CString::new(name).ok()?;
    // SAFETY: `RTLD_DEFAULT` is dlsym's own "search the loaded images"
    // handle, and `name` outlives the call.
    let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };

    (!found.is_null()).then_some(found)
}
