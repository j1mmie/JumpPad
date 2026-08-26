use super::*;

fn beyond_debounce() -> Duration {
    CHANGE_DEBOUNCE + Duration::from_millis(1)
}

#[test]
fn a_settled_burst_reports_once() {
    let mut watch = DocumentWatch::new();
    let start = Instant::now();
    watch.note_event(start);
    watch.note_event(start + CHANGE_DEBOUNCE / 2);
    assert!(watch.pending());

    assert!(watch.settled(start + CHANGE_DEBOUNCE / 2 + beyond_debounce()));
    assert!(!watch.pending());
    assert!(
        !watch.settled(start + beyond_debounce() * 3),
        "a settled burst is consumed, not re-reported"
    );
}

#[test]
fn an_unsettled_burst_reports_nothing() {
    let mut watch = DocumentWatch::new();
    let start = Instant::now();
    watch.note_event(start);
    assert!(!watch.settled(start + CHANGE_DEBOUNCE / 2));
    assert!(watch.pending(), "the burst is still waiting");
}

#[test]
fn directories_are_derived_from_the_paths_and_deduped() {
    let dirs = watched_dirs(&[
        PathBuf::from("/home/user/notes/a.txt"),
        PathBuf::from("/home/user/notes/b.txt"),
        PathBuf::from("/etc/hosts"),
        PathBuf::from("relative.md"),
    ]);
    assert!(dirs.contains(&PathBuf::from("/home/user/notes")));
    assert!(dirs.contains(&PathBuf::from("/etc")));
    assert!(
        dirs.contains(&PathBuf::from(".")),
        "a bare name watches the cwd"
    );
    assert_eq!(dirs.len(), 3);
}
