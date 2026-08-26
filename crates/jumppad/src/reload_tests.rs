use super::*;

fn fingerprint_at(
    path: &str,
    mtime_secs: u64,
    len: u64,
) -> Option<Fingerprint> {
    Some(Fingerprint {
        path: PathBuf::from(path),
        stamp: DiskStamp {
            mtime: std::time::SystemTime::UNIX_EPOCH
                + Duration::from_secs(mtime_secs),
            len,
        },
    })
}

fn beyond_debounce() -> Duration {
    RELOAD_DEBOUNCE + Duration::from_millis(1)
}

#[test]
fn classify_matches_the_two_config_filenames_only() {
    assert_eq!(
        classify(Path::new("/some/dir/config.toml")),
        Some(WatchedFile::Config)
    );
    assert_eq!(
        classify(Path::new("keybinds.toml")),
        Some(WatchedFile::Keybinds)
    );
    assert_eq!(classify(Path::new("/some/dir/notes.txt")), None);
    assert_eq!(classify(Path::new("/some/dir/config.toml.bak")), None);
}

#[test]
fn observe_reports_every_kind_of_movement_once() {
    let mut state = FileState {
        fingerprint: fingerprint_at("config.toml", 1, 10),
        dirty: false,
    };

    assert!(
        !state.observe(fingerprint_at("config.toml", 1, 10)),
        "unchanged"
    );
    assert!(
        state.observe(fingerprint_at("config.toml", 2, 10)),
        "mtime moved"
    );
    assert!(
        state.observe(fingerprint_at("config.toml", 2, 11)),
        "length moved"
    );
    assert!(
        state.observe(fingerprint_at("other/config.toml", 2, 11)),
        "candidate flipped"
    );
    assert!(state.observe(None), "disappeared");
    assert!(
        state.observe(fingerprint_at("config.toml", 3, 5)),
        "appeared"
    );
    // Observation recorded the new state each time - a repeat is quiet.
    assert!(!state.observe(fingerprint_at("config.toml", 3, 5)));
}

#[test]
fn a_settled_burst_reports_each_dirty_file_once() {
    let mut watch = ConfigWatch::seeded(None, None);
    let start = Instant::now();
    watch.note_event(WatchedFile::Config, start);
    watch.note_event(WatchedFile::Keybinds, start);
    assert!(watch.pending());

    assert_eq!(
        watch.settled_with(None, None, start + beyond_debounce()),
        vec![WatchedFile::Config, WatchedFile::Keybinds]
    );
    assert!(!watch.pending());
    assert!(
        watch
            .settled_with(None, None, start + beyond_debounce() * 2)
            .is_empty()
    );
}

#[test]
fn an_unsettled_burst_reports_nothing() {
    let mut watch = ConfigWatch::seeded(None, None);
    let start = Instant::now();
    watch.note_event(WatchedFile::Config, start);
    assert!(
        watch
            .settled_with(None, None, start + RELOAD_DEBOUNCE / 2)
            .is_empty()
    );
    assert!(watch.pending(), "the burst is still waiting");
}

#[test]
fn a_fingerprint_check_dirties_only_what_moved() {
    let seeded = fingerprint_at("config.toml", 1, 10);
    let mut watch = ConfigWatch::seeded(seeded.clone(), None);
    let start = Instant::now();

    // Nothing moved: no burst starts.
    watch.check_with(seeded.clone(), None, start);
    assert!(!watch.pending());

    // Only the config file moved: keybinds stays clean.
    watch.check_with(fingerprint_at("config.toml", 2, 10), None, start);
    assert_eq!(
        watch.settled_with(None, None, start + beyond_debounce()),
        vec![WatchedFile::Config]
    );
}

#[test]
fn note_saved_ignores_paths_that_are_not_a_config_file() {
    let mut watch = ConfigWatch::seeded(None, None);
    watch.note_saved(Path::new("/tmp/notes.txt"), Instant::now());
    assert!(!watch.pending());
}
