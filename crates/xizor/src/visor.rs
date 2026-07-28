//! Pure geometry and animation math for the visor's show/hide slide - no
//! `Message`/window-command types here, just what position the window
//! should be at. `app.rs` is what actually calls `iced::window::move_to`
//! with the values this module computes.

use std::time::{Duration, Instant};

use display_info::DisplayInfo;
use iced::{Point, Rectangle, Size};

/// How long a full show/hide slide takes, start to finish.
const ANIMATION_DURATION: Duration = Duration::from_millis(200);

/// How much of the primary monitor's height the visor occupies once fully
/// shown.
const HEIGHT_FRACTION: f32 = 1.0 / 3.0;

/// An in-progress slide between two `y` positions, in logical window
/// coordinates. `x`/width/height don't change mid-slide (see `app.rs`'s
/// `ToggleVisor` handling - those are snapped once, before a new animation
/// starts, never tweened), so `x` is just carried through unchanged rather
/// than re-derived from the monitor on every animation frame.
pub struct Animation {
    start: Instant,
    pub x: f32,
    from_y: f32,
    to_y: f32,
}

impl Animation {
    /// Starts a new slide toward `to_y`. `from_y` is normally the window's
    /// settled position, but can just as well be another animation's
    /// current (in-flight) position - reversing out of a not-yet-finished
    /// slide this way is what keeps a rapid double-press of the toggle
    /// keybind from glitching.
    pub fn new(x: f32, from_y: f32, to_y: f32) -> Self {
        Self {
            start: Instant::now(),
            x,
            from_y,
            to_y,
        }
    }

    /// The current interpolated `y`, eased with an ease-out cubic curve so
    /// the slide starts fast and settles gently instead of stopping
    /// abruptly.
    pub fn current_y(&self) -> f32 {
        let t = (self.start.elapsed().as_secs_f32() / ANIMATION_DURATION.as_secs_f32())
            .clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from_y + (self.to_y - self.from_y) * eased
    }

    pub fn is_finished(&self) -> bool {
        self.start.elapsed() >= ANIMATION_DURATION
    }
}

/// The primary monitor's bounds, in logical coordinates. `display_info`
/// reports physical pixels; `iced::window`'s `move_to`/`resize` both operate
/// in logical coordinates, so this divides through by `scale_factor` up
/// front rather than leaving that conversion to every caller. Returns
/// `None` if display enumeration fails outright (e.g. a headless
/// environment) - callers skip resizing/positioning rather than panicking.
pub fn primary_monitor_bounds() -> Option<Rectangle> {
    let displays = DisplayInfo::all().ok()?;
    let display = displays
        .iter()
        .find(|display| display.is_primary)
        .or_else(|| displays.first())?;

    // Guard against a nonsensical (zero) scale factor rather than dividing
    // by it - seen in the wild on some misbehaving drivers.
    let scale = if display.scale_factor > 0.0 {
        display.scale_factor
    } else {
        1.0
    };

    Some(Rectangle {
        x: display.x as f32 / scale,
        y: display.y as f32 / scale,
        width: display.width as f32 / scale,
        height: display.height as f32 / scale,
    })
}

/// The window's size while shown - full monitor width, one third its
/// height. Fixed regardless of shown/hidden state; only position animates.
pub fn visor_size(monitor: Rectangle) -> Size {
    Size::new(monitor.width, monitor.height * HEIGHT_FRACTION)
}

/// Where the window sits once fully slid into view.
pub fn shown_position(monitor: Rectangle) -> Point {
    Point::new(monitor.x, monitor.y)
}

/// Where the window sits once fully hidden - parked entirely above the
/// monitor's visible area, not just above its own final resting position.
pub fn hidden_position(monitor: Rectangle) -> Point {
    Point::new(monitor.x, monitor.y - visor_size(monitor).height)
}
