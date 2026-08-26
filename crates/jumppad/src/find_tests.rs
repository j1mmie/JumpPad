use super::*;

/// A state pre-searched over `text` for `query`, as if just opened at
/// the document start.
fn searched(text: &str, query: &str) -> FindState {
    let mut state = FindState {
        query: query.to_string(),
        open: true,
        ..FindState::default()
    };
    state.search(text);
    state
}

#[test]
fn search_selects_the_first_match_from_the_origin() {
    let mut state = searched("one two\nthree two", "two");
    assert_eq!(state.matches.len(), 2);
    assert_eq!(state.current, Some(0));

    // Opening lower down starts at the match below it instead.
    state.origin = (1, 0);
    state.search("one two\nthree two");
    assert_eq!(state.current, Some(1));
}

#[test]
fn search_wraps_to_the_top_when_nothing_follows_the_origin() {
    let mut state = searched("two here", "two");
    state.origin = (9, 9); // past every match
    state.search("two here");
    assert_eq!(state.current, Some(0));
}

#[test]
fn step_wraps_around_in_both_directions() {
    let mut state = searched("a a a", "a");
    assert_eq!(state.matches.len(), 3);
    assert_eq!(state.current, Some(0));

    state.step(1);
    assert_eq!(state.current, Some(1));
    state.step(1);
    assert_eq!(state.current, Some(2));
    state.step(1);
    assert_eq!(
        state.current,
        Some(0),
        "next past the last wraps to the first"
    );

    state.step(-1);
    assert_eq!(
        state.current,
        Some(2),
        "previous before the first wraps to the last"
    );
}

#[test]
fn step_on_no_matches_selects_nothing() {
    let mut state = searched("nothing here", "zebra");
    state.step(1);
    assert_eq!(state.current, None);
    assert_eq!(state.current_match(), None);
}

#[test]
fn index_at_finds_the_match_the_cursor_touches() {
    let state = searched("one two\nthree two", "two");
    assert_eq!(state.index_at((0, 4)), Some(0), "at the start of a match");
    // Selecting a match leaves the cursor at its end, which still counts.
    assert_eq!(state.index_at((0, 7)), Some(0), "at the end of a match");
    assert_eq!(state.index_at((1, 7)), Some(1));
    assert_eq!(state.index_at((0, 0)), None, "outside every match");
}

#[test]
fn counter_reports_position_results_or_nothing() {
    let mut state = searched("a a a", "a");
    assert_eq!(state.counter().as_deref(), Some("1 of 3"));
    state.step(1);
    assert_eq!(state.counter().as_deref(), Some("2 of 3"));

    assert_eq!(
        searched("text", "zebra").counter().as_deref(),
        Some("No results")
    );
    assert_eq!(
        searched("text", "").counter(),
        None,
        "an empty query shows no counter"
    );
}
