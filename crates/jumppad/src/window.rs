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
#[path = "window_tests.rs"]
mod tests;
