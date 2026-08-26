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
