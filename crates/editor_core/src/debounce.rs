use std::time::{Duration, Instant};

/// Tracks bursts of activity separated by quiet gaps, serving both debounce
/// edges: `poke` reports when a burst *starts* (leading edge - undo
/// coalescing keys off this), `fire_if_settled` reports when one *ends*
/// (trailing edge - config reload waits for this). Pure `Instant`
/// arithmetic, with `now` injected so it tests without a clock.
pub struct Debounce {
    window: Duration,
    last_poke: Option<Instant>,
}

impl Debounce {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            last_poke: None,
        }
    }

    /// Records activity. Returns true when this poke begins a new burst -
    /// the previous one either settled or never existed.
    pub fn poke(&mut self, now: Instant) -> bool {
        let new_burst = match self.last_poke {
            Some(last) => now.duration_since(last) > self.window,
            None => true,
        };
        self.last_poke = Some(now);
        new_burst
    }

    /// True once `window` has elapsed since the last poke. Consumes the
    /// burst, so it reports true exactly once per burst.
    pub fn fire_if_settled(&mut self, now: Instant) -> bool {
        match self.last_poke {
            Some(last) if now.duration_since(last) > self.window => {
                self.last_poke = None;
                true
            }
            _ => false,
        }
    }

    /// Whether a burst is waiting to settle - drives a gated timer.
    pub fn pending(&self) -> bool {
        self.last_poke.is_some()
    }

    /// Forgets the burst; the next poke is a leading edge again.
    pub fn reset(&mut self) {
        self.last_poke = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_millis(100);

    fn beyond_window() -> Duration {
        WINDOW + Duration::from_millis(1)
    }

    #[test]
    fn first_poke_is_a_leading_edge() {
        let mut debounce = Debounce::new(WINDOW);
        assert!(debounce.poke(Instant::now()));
    }

    #[test]
    fn a_poke_inside_the_window_extends_the_burst() {
        let mut debounce = Debounce::new(WINDOW);
        let start = Instant::now();
        debounce.poke(start);
        assert!(!debounce.poke(start + WINDOW / 2));
        // The burst end moved with the second poke: still unsettled at a
        // point past the first poke's window.
        assert!(!debounce.fire_if_settled(start + beyond_window()));
    }

    #[test]
    fn a_poke_past_the_window_starts_a_new_burst() {
        let mut debounce = Debounce::new(WINDOW);
        let start = Instant::now();
        debounce.poke(start);
        assert!(debounce.poke(start + beyond_window()));
    }

    #[test]
    fn fire_if_settled_reports_true_exactly_once() {
        let mut debounce = Debounce::new(WINDOW);
        let start = Instant::now();
        debounce.poke(start);
        assert!(!debounce.fire_if_settled(start + WINDOW / 2));
        assert!(debounce.fire_if_settled(start + beyond_window()));
        assert!(!debounce.fire_if_settled(start + beyond_window() * 2));
    }

    #[test]
    fn fire_if_settled_without_a_burst_is_false() {
        let mut debounce = Debounce::new(WINDOW);
        assert!(!debounce.fire_if_settled(Instant::now()));
    }

    #[test]
    fn reset_makes_the_next_poke_a_leading_edge() {
        let mut debounce = Debounce::new(WINDOW);
        let start = Instant::now();
        debounce.poke(start);
        debounce.reset();
        assert!(!debounce.pending());
        assert!(debounce.poke(start + WINDOW / 2));
    }

    #[test]
    fn pending_tracks_the_burst_lifecycle() {
        let mut debounce = Debounce::new(WINDOW);
        let start = Instant::now();
        assert!(!debounce.pending());
        debounce.poke(start);
        assert!(debounce.pending());
        debounce.fire_if_settled(start + beyond_window());
        assert!(!debounce.pending());
    }
}
