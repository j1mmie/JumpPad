//! The window itself: what the config asks for, and replacing the one on
//! screen when that answer changes.
//!
//! A window is handed its transparency, its decorations and its level once,
//! when it is created, and there is no setter for any of them afterwards. So
//! a `config.toml` edit that changes one of those can only be honored by
//! putting a second window on screen and letting the first go.

use iced::window::{Id, Level, Position, Settings};
use iced::{Size, Task};
use jumppad_config::Config;

/// The size a window opens at when there is no previous one to match.
const STARTUP_SIZE: Size = Size::new(900.0, 600.0);

/// The window `config` describes.
///
/// The single source for every setting a window can only be given when it is
/// created: `run()` builds the first window from this, and [`replace`] builds
/// every later one, so the two can't come apart.
pub fn settings(config: &Config) -> Settings {
    Settings {
        size: STARTUP_SIZE,
        // Visor mode wins: a drop-down visor is undecorated by definition.
        decorations: config.window.decorations && !config.visor.enabled,
        // Asked of every theme, not just the one showing: a theme switch
        // shouldn't need a new window when the file already said some theme
        // wanted one see-through. Skipped entirely (not just
        // requested-then-ignored) when no theme asks, since transparent
        // windows use a costlier compositing path.
        transparent: config.wants_transparency(),
        // The visor floats above whatever else has focus; an ordinary window
        // doesn't.
        level: if config.visor.enabled {
            Level::AlwaysOnTop
        } else {
            Level::Normal
        },
        // Set here rather than through the application builder, which only
        // reaches the first window: a replacement left on the default would
        // close itself on the OS close button, skipping the draft flush the
        // app does first.
        exit_on_close_request: false,
        ..Settings::default()
    }
}

/// Whether these two configs disagree about anything [`settings`] carries -
/// which is what makes a reload need a new window rather than reaching the
/// one already on screen.
pub fn needs_replacing(old: &Config, new: &Config) -> bool {
    let old = settings(old);
    let new = settings(new);

    old.decorations != new.decorations
        || old.transparent != new.transparent
        || old.level != new.level
}

/// Opens a window built to `settings` where `previous` currently sits, and
/// closes `previous` once the replacement exists. Yields the new window's id.
///
/// The order is load-bearing. iced ends the program when the last window is
/// destroyed, so the replacement has to exist before the outgoing one goes;
/// closing first would exit the app instead of swapping its window.
///
/// Two things are visible while both windows are up, for the frame or two
/// that takes: they show the same view, because `iced::application` hands
/// every window the same one, and they are cleared with the same color,
/// because a program's style is not per-window either.
pub fn replace(previous: Id, settings: Settings) -> Task<Id> {
    geometry(previous)
        .then(move |(size, position)| {
            let settings = Settings {
                size,
                position: position
                    .map_or(settings.position, Position::Specific),
                ..settings.clone()
            };
            let (_id, opened) = iced::window::open(settings);

            opened
        })
        .then(move |id| {
            Task::batch([iced::window::close(previous), Task::done(id)])
        })
}

/// Where the outgoing window sits, so its replacement can open in the same
/// place rather than jumping to the middle of the screen mid-session.
fn geometry(id: Id) -> Task<(Size, Option<iced::Point>)> {
    iced::window::size(id).then(move |size| {
        iced::window::position(id).map(move |position| (size, position))
    })
}

#[cfg(test)]
mod tests {
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
}
