//! Windows window tweaks that neither iced nor winit exposes.

// `window_handle()` resolves through `iced::window::Window`'s own supertrait
// bound, so `HasWindowHandle` doesn't need to be in scope here.
use iced::window::raw_window_handle::RawWindowHandle;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMSBT_NONE, DWMWA_SYSTEMBACKDROP_TYPE,
};

/// Turns off the Windows 11 system backdrop so a translucent window shows the
/// *desktop* through it rather than a DWM-drawn material.
///
/// **The bright-window-with-decorations fix.** winit unconditionally calls
/// `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE, ...)` with whatever
/// `WindowAttributes::platform_specific.backdrop_type` holds, and iced never
/// surfaces that field, so it stays at winit's `BackdropType::Auto` -
/// `DWMSBT_AUTO`, "let DWM pick". On Windows 11 DWM's pick for a *decorated*
/// window is a Mica-style backdrop painted behind the client area. Our alpha
/// then reveals that material instead of the desktop, and since it is a light,
/// wallpaper-derived wash the window reads far brighter and more solid than
/// the configured alpha - dramatically so on a light theme, which is already
/// close to the material's own brightness.
///
/// The correlation that pinned it down: `[window] decorations = false` fixes
/// it outright. An undecorated window has no frame for DWM to hang a backdrop
/// on, so nothing is drawn behind the client area and the desktop shows
/// through correctly. `DWMSBT_NONE` asks for that same "no material" state
/// while keeping the titlebar.
///
/// Only `jumppad-gpu` is affected in practice: the `tiny-skia` binary presents
/// through softbuffer's GDI blit into the window's redirection bitmap rather
/// than a flip-model swapchain composited by DWM, and comes out correct with
/// decorations either way.
pub fn disable_system_backdrop(window: &dyn iced::window::Window) {
    let Ok(handle) = window.window_handle() else {
        log::warn!("jumppad: no window handle; leaving the system backdrop alone");
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };

    let backdrop = DWMSBT_NONE;
    // SAFETY: the handle guarantees a live `HWND`, and the attribute is
    // written from a correctly sized `DWM_SYSTEMBACKDROP_TYPE`. Unsupported
    // on Windows 10 and early 11 builds, where DWM returns a failure `HRESULT`
    // and changes nothing - which is already the behaviour we want there.
    let result = unsafe {
        DwmSetWindowAttribute(
            handle.hwnd.get() as HWND,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            std::ptr::from_ref(&backdrop).cast(),
            size_of_val(&backdrop) as u32,
        )
    };

    if result < 0 {
        log::debug!("jumppad: no system-backdrop control on this Windows build (0x{result:X})");
    } else {
        log::debug!("jumppad: disabled the window's system backdrop");
    }
}
