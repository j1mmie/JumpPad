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
#[path = "debounce_tests.rs"]
mod tests;
