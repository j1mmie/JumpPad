use serde::{Deserialize, Serialize};

/// How the hardware-rendered binary talks to the graphics stack: which
/// adapter it asks for, and whether it waits for the display before showing
/// a frame. Read only by `jumppad-gpu` - the software binary has no adapter
/// to choose and no swapchain to wait on.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(default)]
pub struct GpuConfig {
    pub power: GpuPower,

    /// `vsync = true` makes a frame wait for the display before it is shown.
    /// **Defaults to `false`**, which is the unusual answer and is the one
    /// that makes `jumppad-gpu` feel like `jumppad`.
    ///
    /// The two binaries reach the screen by completely different routes, and
    /// only one of them ever queued a frame. `jumppad` presents through
    /// `softbuffer`, which on Windows is a GDI blit into the window's
    /// redirection bitmap and on macOS is a layer-contents swap: neither
    /// blocks, so a frame drawn from the pointer's current position is on
    /// its way to the compositor before the function returns. `jumppad-gpu`
    /// presents through a swapchain, and with vsync on that swapchain holds
    /// the frame until the next refresh, then the desktop compositor spends
    /// another one showing it. Two refreshes at 60Hz is 33ms between moving
    /// the mouse and seeing the selection follow it, and that is the lag
    /// reported from Windows: a highlight two or three frames behind the
    /// pointer, and scrolling that arrives late.
    ///
    /// Turning it off costs nothing here that it would cost a game. Tearing
    /// is what vsync buys, and a torn frame needs the scanout to change
    /// mid-scan - which a window composited by DWM or by the macOS window
    /// server cannot do, because the compositor is what reaches the display,
    /// not this app's swapchain. There is no busy loop either: JumpPad draws
    /// when something asks it to and idles at zero frames otherwise, so
    /// "unsynchronized" here means "shown as soon as it is drawn", not
    /// "drawn as fast as the GPU can".
    ///
    /// It stays configurable because Linux can put this app's frames on the
    /// scanout directly, which is the one arrangement that can tear: an X11
    /// session running without a compositor, or a Wayland compositor giving
    /// a fullscreen window its own plane. Turn it back on if a frame ever
    /// shows up torn in half.
    pub vsync: bool,
}

/// How much GPU to ask for.
///
/// Defaults to [`GpuPower::High`], matching what iced asks for when nothing
/// says otherwise. Not because a plaintext editor needs a discrete card - it
/// does not, and asking for one costs battery - but because on Windows the
/// adapter decides whether the window can be translucent at all, and the
/// discrete one is likelier to say yes. Defaulting the other way traded a
/// feature away for power this app never uses.
///
/// The asymmetry belongs to the Vulkan backend: it asks the driver which
/// composite alpha modes a surface supports, and drivers disagree. An NVIDIA
/// adapter offered `PreMultiplied`; the AMD integrated one beside it offered
/// only `Opaque`, which silently costs `background.alpha` and every acrylic
/// resting on it. Neither macOS nor the software binary is affected - wgpu's
/// Metal backend reports its alpha modes as a constant, so any adapter there
/// can be translucent, and the software renderer presents through GDI on
/// Windows without consulting an adapter at all.
///
/// So `"low"` is the lever to reach for on a laptop, or to keep JumpPad off
/// whatever a discrete card is already busy with - an NVIDIA driver was seen
/// recursing to a stack overflow inside `vkCreateDevice` while a game held
/// the GPU, where the integrated adapter started every time. That is a
/// driver bug rather than something this setting fixes; it is only the lever
/// that avoids it, at the cost above.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum GpuPower {
    /// `power = "high"`. The discrete adapter where the machine has one.
    #[default]
    High,
    /// `power = "low"`. The integrated adapter. Cheaper, and out of a busy
    /// discrete card's way - but on Windows it is the choice that can leave
    /// you with an opaque window, so check that transparency survived it.
    Low,
    /// `power = "auto"`. State no preference and take whatever wgpu ranks
    /// first for the surface.
    Auto,
}

impl GpuPower {
    /// The spelling wgpu reads out of `WGPU_POWER_PREF`.
    ///
    /// That variable is how this setting reaches iced at all - see
    /// `prefer_gpu` in `jumppad`'s `lib.rs`. `Auto` is wgpu's `"none"`,
    /// meaning no preference rather than no GPU.
    pub fn as_wgpu_power_pref(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
            Self::Auto => "none",
        }
    }
}
