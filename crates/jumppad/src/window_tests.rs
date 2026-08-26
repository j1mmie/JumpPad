use super::*;

fn config(toml: &str) -> Config {
    toml::from_str(toml).unwrap()
}

#[test]
fn a_default_config_asks_for_a_plain_solid_window() {
    let settings = settings(&Config::default());
    assert!(!settings.transparent);
    assert!(settings.decorations);
    assert_eq!(settings.level, Level::Normal);
}

/// Never left to the default: it only reaches the first window through
/// the application builder, and every window has drafts to flush.
#[test]
fn every_window_lets_the_app_handle_its_own_close() {
    assert!(!settings(&Config::default()).exit_on_close_request);
    assert!(
        Settings::default().exit_on_close_request,
        "the default it must override"
    );
}

#[test]
fn a_theme_wanting_translucency_asks_for_a_transparent_window() {
    let translucent = config("[themes.night]\nbackground.alpha = 0.9");
    assert!(settings(&translucent).transparent);
}

#[test]
fn a_translucent_base_theme_asks_for_a_transparent_window() {
    let shared = config("[themes.base]\nbackground.alpha = 0.9");
    assert!(settings(&shared).transparent);
}

/// Even a theme no `[mode]` slot names: the slots are live-editable, so
/// any theme in the file can reach the screen without a restart.
#[test]
fn transparency_is_asked_of_themes_that_are_not_showing() {
    let unshown = config(
        "[mode]\ntheme.dark = \"Dark\"\n\n[themes.unused]\nbackground.alpha = 0.5",
    );
    assert!(settings(&unshown).transparent);
}

#[test]
fn visor_mode_takes_the_decorations_off_and_floats_the_window() {
    let visor =
        config("[visor]\nenabled = true\n\n[window]\ndecorations = true");
    let settings = settings(&visor);
    assert!(!settings.decorations, "a drop-down visor is undecorated");
    assert_eq!(settings.level, Level::AlwaysOnTop);
}

#[test]
fn each_window_setting_alone_calls_for_a_replacement() {
    let plain = Config::default();

    for changed in [
        config("[themes.night]\nbackground.alpha = 0.9"),
        config("[window]\ndecorations = false"),
        config("[visor]\nenabled = true"),
    ] {
        assert!(
            needs_replacing(&plain, &changed),
            "{:?} should need a new window",
            settings(&changed)
        );
    }
}

#[test]
fn a_reload_that_moves_nothing_a_window_carries_keeps_it() {
    let before = config("[themes.night]\neditor.font.size = 16.0");
    let after = config(
        "[themes.night]\neditor.font.size = 24.0\npalette = \"Nord\"",
    );
    assert!(!needs_replacing(&before, &after));
}
