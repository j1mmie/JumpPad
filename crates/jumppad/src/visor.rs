//! Pure geometry and animation math for the visor's show/hide slide - just
//! what position the window should be at, no `Message`/window-command types.

use std::time::{Duration, Instant};

use display_info::DisplayInfo;
use iced::{Point, Rectangle, Size};

/// How long a full show/hide slide takes, start to finish.
const ANIMATION_DURATION: Duration = Duration::from_millis(200);

/// How much of the primary monitor's height the visor occupies once fully
/// shown.
const HEIGHT_FRACTION: f32 = 1.0 / 3.0;

/// An in-progress slide between two `y` positions. `x`/width/height are
/// snapped once before the slide starts and never tweened.
pub struct Animation {
    start: Instant,
    pub x: f32,
    from_y: f32,
    to_y: f32,
}

impl Animation {
    /// Starts a new slide toward `to_y`. `from_y` can be another animation's
    /// in-flight position, to reverse smoothly out of it.
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

/// The primary monitor's bounds, converted from physical to logical
/// coordinates. `None` if display enumeration fails (e.g. headless).
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
