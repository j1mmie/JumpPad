//! Windows window tweaks that neither iced nor winit exposes.

// `window_handle()` resolves through `iced::window::Window`'s own supertrait
// bound, so `HasWindowHandle` doesn't need to be in scope here.
use iced::window::raw_window_handle::RawWindowHandle;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND, DWM_SYSTEMBACKDROP_TYPE,
    DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DwmEnableBlurBehindWindow, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BLACK_BRUSH, CreateRectRgn, DeleteObject, FillRect, GetDC, GetStockObject,
    ReleaseDC,
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

/// Puts the Windows 11 system backdrop where `[themes] background.blur`
/// asks: `DWMSBT_TRANSIENTWINDOW` (Acrylic - a live blur of whatever sits
/// behind the window) when it is on, and `DWMSBT_NONE` when it is off, so a
/// translucent window shows the *desktop* through it rather than a DWM-drawn
/// material nobody asked for.
///
/// **The off case is the bright-window-with-decorations fix.** winit
/// unconditionally calls `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE,
/// ...)` with whatever `WindowAttributes::platform_specific.backdrop_type`
/// holds, and iced never surfaces that field, so it stays at winit's
/// `BackdropType::Auto` - `DWMSBT_AUTO`, "let DWM pick". On Windows 11 DWM's
/// pick for a *decorated* window is a Mica-style backdrop painted behind the
/// client area. Our alpha then reveals that material instead of the desktop,
/// and since it is a light, wallpaper-derived wash the window reads far
/// brighter and more solid than the configured alpha - dramatically so on a
/// light theme, which is already close to the material's own brightness.
///
/// The correlation that pinned it down: `[window] decorations = false` fixes
/// it outright. An undecorated window has no frame for DWM to hang a backdrop
/// on, so nothing is drawn behind the client area and the desktop shows
/// through correctly. `DWMSBT_NONE` asks for that same "no material" state
/// while keeping the titlebar.
///
/// **The on case is that same mechanism, wanted.** A backdrop DWM paints
/// behind the client area is exactly what a blur has to be here - nothing in
/// this process can reach the desktop's pixels - so asking for the Acrylic
/// material is the whole implementation. That also fixes the material's
/// reach: it is drawn behind the client area only, so the frost stops at the
/// titlebar, which keeps its own system look.
///
/// Only `jumppad-gpu` is known to show either state: the `tiny-skia` binary
/// presents through softbuffer's GDI blit into the window's redirection
/// bitmap rather than a flip-model swapchain composited by DWM, and never
/// showed the unwanted backdrop that `DWMSBT_NONE` exists to remove - so it
/// most likely won't show a wanted one either.
pub fn set_system_backdrop(window: &dyn iced::window::Window, blur: bool) {
    let Some(hwnd) = hwnd_of(window, "the system backdrop") else {
        return;
    };

    let backdrop: DWM_SYSTEMBACKDROP_TYPE = if blur {
        DWMSBT_TRANSIENTWINDOW
    } else {
        DWMSBT_NONE
    };
    // SAFETY: the handle guarantees a live `HWND`, and the attribute is
    // written from a correctly sized `DWM_SYSTEMBACKDROP_TYPE`. Unsupported
    // on Windows 10 and early 11 builds, where DWM returns a failure `HRESULT`
    // and changes nothing - which for the `DWMSBT_NONE` case is already the
    // behaviour we want, and for the blur case is the honest answer that
    // those builds have no material to offer.
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
    } else if blur {
        log::debug!("jumppad: asked DWM to frost the window's backdrop");
    } else {
        log::debug!("jumppad: disabled the window's system backdrop");
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
