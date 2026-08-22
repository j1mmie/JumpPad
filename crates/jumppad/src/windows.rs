//! Windows window tweaks that neither iced nor winit exposes.

// `window_handle()` resolves through `iced::window::Window`'s own supertrait
// bound, so `HasWindowHandle` doesn't need to be in scope here.
use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use iced::window::raw_window_handle::RawWindowHandle;
use jumppad_config::ResolvedBlur;
use windows_sys::Win32::Foundation::{BOOL, HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND, DWM_SYSTEMBACKDROP_TYPE,
    DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DwmEnableBlurBehindWindow, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BLACK_BRUSH, CreateRectRgn, DeleteObject, FillRect, GetDC, GetStockObject,
    ReleaseDC,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleA, GetProcAddress,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

/// Pulls the `HWND` out of an iced window, or `None` if this isn't a Win32
/// window (or the handle has already gone away).
fn hwnd_of(window: &dyn iced::window::Window, what: &str) -> Option<HWND> {
    let Ok(handle) = window.window_handle() else {
        log::warn!("jumppad: no window handle; leaving {what} alone");
        return None;
    };
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

/// Frosts the desktop behind a translucent window the way `[themes]
/// background.blur` asks, by whichever of Windows' two acrylics that names.
///
/// **Windows has no blur radius, so the amount is not what varies here** -
/// see `ResolvedBlur`. What varies is which acrylic, and the one thing they
/// disagree about is whether the frost survives the window losing focus.
///
/// - **`DWMSBT_TRANSIENTWINDOW`**, the documented Windows 11 system
///   backdrop, which is what a radius or `"acrylic"` asks for. DWM draws it
///   behind the client area and stops drawing it while the window is not
///   focused, which is DWM's own policy for a *transient* material and has
///   no override: the knob that exists, `SystemBackdropConfiguration.
///   IsInputActive`, belongs to the Windows App SDK's backdrop controllers,
///   not to this attribute.
/// - **`ACCENT_ENABLE_ACRYLICBLURBEHIND`**, what `"acrylic_always"` asks
///   for, through the undocumented `SetWindowCompositionAttribute`. Its
///   `ACCENT_POLICY` has no notion of focus at all, so DWM applies it
///   whether or not the window is active - which is the whole reason the
///   name exists. Opt-in, and never touched otherwise, because it costs:
///   the API is undocumented and could go; it is reported to lag while the
///   window is dragged or resized; and the blur drops out mid-maximize
///   until the window has finished maximizing.
///
/// `DWMSBT_NONE` is set for every other case, including alongside the
/// accent-policy acrylic - it is what stops DWM drawing a Mica backdrop
/// nobody asked for (see AGENTS.md), and that has to hold whether or not
/// something else is doing the frosting.
pub fn set_system_backdrop(
    window: &dyn iced::window::Window,
    blur: ResolvedBlur,
) {
    let Some(hwnd) = hwnd_of(window, "the system backdrop") else {
        return;
    };
    let dwm_draws_it = blur.is_on() && !blur.holds_unfocused;

    set_backdrop_type(
        hwnd,
        if dwm_draws_it {
            DWMSBT_TRANSIENTWINDOW
        } else {
            DWMSBT_NONE
        },
    );
    set_accent_acrylic(hwnd, blur.is_on() && blur.holds_unfocused);
}

/// Writes `DWMWA_SYSTEMBACKDROP_TYPE`, the documented half.
fn set_backdrop_type(hwnd: HWND, backdrop: DWM_SYSTEMBACKDROP_TYPE) {
    // SAFETY: the handle guarantees a live `HWND`, and the attribute is
    // written from a correctly sized `DWM_SYSTEMBACKDROP_TYPE`. Unsupported
    // on Windows 10 and early 11 builds, where DWM returns a failure
    // `HRESULT` and changes nothing - which for `DWMSBT_NONE` is already the
    // behaviour we want, and for the acrylic is the honest answer that those
    // builds have no such material to offer.
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            std::ptr::from_ref(&backdrop).cast(),
            size_of_val(&backdrop) as u32,
        )
    };

    if result < 0 {
        log::debug!(
            "jumppad: no system-backdrop control on this Windows build (0x{result:X})"
        );
    } else if backdrop == DWMSBT_TRANSIENTWINDOW {
        log::debug!("jumppad: asked DWM to frost the window's backdrop");
    } else {
        log::debug!("jumppad: disabled the window's system backdrop");
    }
}

/// Turns the accent-policy acrylic on or off, the undocumented half.
///
/// Nothing is asked of `SetWindowCompositionAttribute` until a theme has
/// actually asked for `"acrylic_always"`: a session that never names it
/// never reaches for the undocumented API at all, and only one that has
/// needs the call that clears it again.
fn set_accent_acrylic(hwnd: HWND, frosted: bool) {
    static EVER_APPLIED: AtomicBool = AtomicBool::new(false);

    if !frosted && !EVER_APPLIED.swap(false, Ordering::Relaxed) {
        return;
    }
    let Some(set_composition_attribute) = accent_policy_setter() else {
        if frosted {
            log::warn!(
                "jumppad: no SetWindowCompositionAttribute on this Windows; \
                 leaving [themes] background.blur = \"acrylic_always\" unhonored"
            );
        }
        return;
    };
    EVER_APPLIED.store(frosted, Ordering::Relaxed);

    let mut policy = AccentPolicy {
        state: if frosted {
            ACCENT_ENABLE_ACRYLICBLURBEHIND
        } else {
            ACCENT_DISABLED
        },
        flags: 0,
        // The tint the acrylic carries of its own, as `AABBGGRR`. Held to
        // the least alpha that still counts as one, because the theme's own
        // background color is already painted over this at
        // `background.alpha` and a second tint would only fight it - but not
        // to zero, which this acrylic reads as "no tint at all" and refuses.
        gradient_color: 0x0100_0000,
        animation_id: 0,
    };
    let mut attribute = WindowCompositionAttributeData {
        attribute: WCA_ACCENT_POLICY,
        data: std::ptr::from_mut(&mut policy).cast(),
        size: size_of::<AccentPolicy>(),
    };

    // SAFETY: the handle guarantees a live `HWND`, and the attribute
    // describes a correctly sized `ACCENT_POLICY` it points at. The layout
    // is the reverse-engineered one every caller of this undocumented
    // function uses, and a mismatch would be refused rather than acted on -
    // the size is passed alongside.
    let applied = unsafe {
        set_composition_attribute(hwnd, std::ptr::from_mut(&mut attribute))
    };

    if applied == 0 {
        log::warn!("jumppad: the accent-policy acrylic was refused");
    } else {
        log::debug!(
            "jumppad: accent-policy acrylic turned {}",
            if frosted { "on" } else { "off" }
        );
    }
}

/// Zeroes the window's redirection surface and re-arms DWM per-pixel alpha,
/// so a translucent window is translucent from the first frame rather than
/// from the first resize.
///
/// **The opaque-until-you-resize fix.** Every Win32 window DWM composites has
/// a *redirection surface* behind it, and per-pixel alpha compositing reads
/// **that surface's** alpha channel - not the swapchain's. Two things
/// conspire to leave it opaque:
///
/// - winit registers its window class with `hbrBackground: 0`, a null brush,
///   so nothing ever paints the surface. Win32 references on this topic
///   specifically prescribe `BLACK_BRUSH` here, precisely so the surface
///   starts at an all-zero (fully transparent) state.
/// - winit does call `DwmEnableBlurBehindWindow`, which is what asks DWM to
///   honour per-pixel alpha - but from `on_create`, long before any swapchain
///   exists. The same "right lever, wrong moment" trap as the macOS shadow
///   cache in `macos.rs`, and it wants the same treatment: re-apply it once
///   real frames have presented.
///
/// So the surface keeps whatever the system left in it, which reads as an
/// opaque (typically white) window. Resizing reallocates it, which is why
/// dragging the window bigger reveals correctly translucent bands exactly
/// where the new area landed - and why the old "resize kick" hack worked.
/// This does the same job without touching the window's size: fill the client
/// area with the black brush that should have been the class background, then
/// re-issue the blur-behind that marks the alpha as meaningful.
///
/// Only `jumppad-gpu` needs it. `tiny-skia` presents by blitting through this
/// very surface every frame, so it initialises it as a side effect of drawing.
pub fn reset_redirection_surface(window: &dyn iced::window::Window) {
    let Some(hwnd) = hwnd_of(window, "the redirection surface") else {
        return;
    };

    // SAFETY: the handle guarantees a live `HWND`. Each GDI object below is
    // released on every path, and `GetClientRect` fully initialises `rect`
    // before it is read (checked via its `BOOL` return).
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        if GetClientRect(hwnd, &mut rect) != 0 {
            // `HDC` is a bare handle in `windows-sys` 0.52, so a failed
            // `GetDC` comes back as 0 rather than a null pointer.
            let hdc = GetDC(hwnd);
            if hdc != 0 {
                // Black is what makes this work, not an arbitrary colour:
                // GDI writes 0x00000000, so the surface's alpha lands at 0
                // and the desktop shows through at full strength.
                FillRect(hdc, &rect, GetStockObject(BLACK_BRUSH) as _);
                ReleaseDC(hwnd, hdc);
            }
        }

        // An empty region means "no blur, just honour the alpha" - the same
        // parameters winit passes at creation, re-applied now that the
        // swapchain is live.
        let region = CreateRectRgn(0, 0, -1, -1);
        let blur_behind = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
            fEnable: 1,
            hRgnBlur: region,
            fTransitionOnMaximized: 0,
        };
        let result = DwmEnableBlurBehindWindow(hwnd, &blur_behind);
        DeleteObject(region as _);

        if result < 0 {
            log::warn!(
                "jumppad: could not re-arm per-pixel alpha (0x{result:X})"
            );
        } else {
            log::debug!("jumppad: reset the window's redirection surface");
        }
    }
}

/// `SetWindowCompositionAttribute(hwnd, data)`, returning zero on failure.
type SetWindowCompositionAttribute = unsafe extern "system" fn(
    HWND,
    *mut WindowCompositionAttributeData,
) -> BOOL;

/// The one attribute this app sets through that function: `WCA_ACCENT_POLICY`.
const WCA_ACCENT_POLICY: u32 = 0x13;

/// `ACCENT_STATE`'s two values this app has any use for.
const ACCENT_DISABLED: u32 = 0;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

/// The argument `SetWindowCompositionAttribute` takes: which attribute, and
/// a sized pointer to its value.
#[repr(C)]
struct WindowCompositionAttributeData {
    attribute: u32,
    data: *mut c_void,
    size: usize,
}

/// `WCA_ACCENT_POLICY`'s value. Undeclared by Windows in any public header,
/// so this layout is the reverse-engineered one - the same four fields, in
/// the same order, that every other caller of this function passes.
#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

/// `SetWindowCompositionAttribute` out of `user32.dll`, or `None` where it
/// has gone. Looked up rather than linked for the same reason the macOS blur
/// is (see `macos.rs`): an undocumented export that disappears should cost
/// the one theme setting that names it, not the app's ability to start.
///
/// `GetModuleHandleA` rather than `LoadLibrary`: a GUI process has `user32`
/// loaded long before it has a window, so there is no reference to take or
/// give back.
fn accent_policy_setter() -> Option<SetWindowCompositionAttribute> {
    static FOUND: OnceLock<Option<SetWindowCompositionAttribute>> =
        OnceLock::new();

    *FOUND.get_or_init(|| {
        // SAFETY: both names are nul-terminated `CStr`s that outlive the
        // calls, and the result is transmuted to the signature every caller
        // of this function uses. A missing module or export comes back null
        // and is handled as the `None` below.
        unsafe {
            let user32 = GetModuleHandleA(c"user32.dll".as_ptr().cast());
            if user32 == 0 {
                return None;
            }
            let found = GetProcAddress(
                user32,
                c"SetWindowCompositionAttribute".as_ptr().cast(),
            )?;

            Some(std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                SetWindowCompositionAttribute,
            >(found))
        }
    })
}
