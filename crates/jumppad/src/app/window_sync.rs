use super::*;

impl JumpPadApp {
    /// Arms the shadow-refresh countdown (see `macos.rs`): the window server's
    /// shadow cache goes stale whenever the content changes, and refreshing it
    /// too early re-caches the outgoing frame, so the invalidation waits
    /// `SHADOW_REFRESH_FRAMES` presented frames.
    pub(super) fn arm_shadow_refresh(&mut self) {
        if cfg!(target_os = "macos") && self.background_alpha < 1.0 {
            self.shadow_refresh_frames = SHADOW_REFRESH_FRAMES;
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn refresh_window_shadow(&self) -> Task<Message> {
        match self.window {
            Some(id) => iced::window::run(id, |window| {
                crate::macos::invalidate_window_shadow(window);
            })
            .discard(),
            None => Task::none(),
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn refresh_window_shadow(&self) -> Task<Message> {
        Task::none()
    }

    /// Pins the window's appearance to the slot the config names, or clears
    /// it while the config is following the OS - a pinned appearance is
    /// precisely what stops the OS from being heard (see
    /// `macos::pin_appearance`) - and takes the OS's own answer on the way
    /// past. A session that spent time pinned has heard nothing from the OS
    /// in the meantime, so that answer is stale exactly when `auto` comes
    /// back on.
    #[cfg(target_os = "macos")]
    pub(super) fn sync_window_appearance(&self) -> Task<Message> {
        let Some(id) = self.window else {
            return Task::none();
        };
        let pinned = self.config.mode.pinned();

        iced::window::run(id, move |window| {
            crate::macos::pin_appearance(window, pinned);
            crate::macos::system_appearance()
        })
        // Travelling as the runtime's own report, so a read and a switch
        // arrive by the same road.
        .map(|appearance| match appearance {
            Some(Appearance::Light) => iced::theme::Mode::Light,
            Some(Appearance::Dark) => iced::theme::Mode::Dark,
            None => iced::theme::Mode::None,
        })
        .map(Message::SystemAppearanceReported)
    }

    /// Nothing to do elsewhere: no other platform lets a pinned appearance
    /// silence the OS, and winit reports the switch on its own.
    #[cfg(not(target_os = "macos"))]
    pub(super) fn sync_window_appearance(&self) -> Task<Message> {
        Task::none()
    }

    /// Frosts the desktop showing through the window the way the theme's
    /// `background.blur` asks. Each platform is asked its own way, and each
    /// reads only the forms it can act on - Windows its two acrylics, macOS a
    /// radius - so what the other platform's forms mean here is nothing.
    ///
    /// Only a translucent window is told anything, on either. A solid one
    /// has no desktop showing through to frost, and on Windows the off case
    /// also overrides a backdrop winit already asked DWM for, which would be
    /// a gratuitous difference from every other app on a window nobody can
    /// see through (see `windows.rs`).
    #[cfg(target_os = "windows")]
    pub(super) fn apply_window_blur(&self) -> Task<Message> {
        let blur = self.background_blur;
        match self.window {
            Some(id) if self.background_alpha < 1.0 => {
                iced::window::run(id, move |window| {
                    crate::windows::set_system_backdrop(window, blur);
                })
                .discard()
            }
            _ => Task::none(),
        }
    }

    /// Same gate as above, the window server's own blur behind it. Only the
    /// radius travels, and the acrylics have none: a window-server blur has
    /// no focus to lose the frost to, so the distinction they draw is one
    /// macOS has no way to be asked about.
    #[cfg(target_os = "macos")]
    pub(super) fn apply_window_blur(&self) -> Task<Message> {
        let radius = self.background_blur.radius();
        match self.window {
            Some(id) if self.background_alpha < 1.0 => {
                iced::window::run(id, move |window| {
                    crate::macos::set_window_blur(window, radius);
                })
                .discard()
            }
            _ => Task::none(),
        }
    }

    /// No blur to ask for anywhere else: X11 and Wayland leave it to the
    /// compositor, whether through its own window rules or a protocol
    /// extension, and neither is reachable through what iced exposes.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub(super) fn apply_window_blur(&self) -> Task<Message> {
        Task::none()
    }

    /// Arms the one-shot redirection-surface reset (see `windows.rs`). Only
    /// on a translucent Windows window: elsewhere there is nothing to fix, and
    /// on a solid window the surface's alpha is never read.
    pub(super) fn arm_surface_reset(&mut self) {
        if cfg!(target_os = "windows") && self.background_alpha < 1.0 {
            self.surface_reset_frames = SURFACE_RESET_FRAMES;
        }
    }

    #[cfg(target_os = "windows")]
    pub(super) fn reset_redirection_surface(&self) -> Task<Message> {
        match self.window {
            Some(id) => iced::window::run(id, |window| {
                crate::windows::reset_redirection_surface(window);
            })
            .discard(),
            None => Task::none(),
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn reset_redirection_surface(&self) -> Task<Message> {
        Task::none()
    }

    /// Snaps the window to the primary monitor's current bounds - full
    /// width, one third the height - and parks it off-screen above the top,
    /// ready to slide into view. Called once at startup.
    pub(super) fn snap_to_monitor(&mut self) -> Task<Message> {
        if !self.visor_enabled {
            return Task::none();
        }
        let Some(id) = self.window else {
            return Task::none();
        };
        let Some(monitor) = visor::primary_monitor_bounds() else {
            log::warn!("couldn't determine the primary monitor's bounds");
            return Task::none();
        };
        Task::batch([
            iced::window::resize(id, visor::visor_size(monitor)),
            iced::window::move_to(id, visor::hidden_position(monitor)),
        ])
    }

    /// Starts (or reverses) the visor's show/hide slide. Re-snaps width and
    /// x-position to the primary monitor's current bounds first, then tweens `y`.
    pub(super) fn toggle_visor(&mut self) -> Task<Message> {
        if !self.visor_enabled {
            return Task::none();
        }
        let Some(id) = self.window else {
            return Task::none();
        };
        let Some(monitor) = visor::primary_monitor_bounds() else {
            log::warn!("couldn't determine the primary monitor's bounds");
            return Task::none();
        };

        // Reverse out of a not-yet-finished animation instead of jumping to
        // the settled position, so a rapid double-toggle doesn't glitch.
        let current_y = match &self.animation {
            Some(animation) => animation.current_y(),
            None if self.visor_visible => visor::shown_position(monitor).y,
            None => visor::hidden_position(monitor).y,
        };

        self.visor_visible = !self.visor_visible;
        let target = if self.visor_visible {
            visor::shown_position(monitor)
        } else {
            visor::hidden_position(monitor)
        };
        self.animation = Some(Animation::new(target.x, current_y, target.y));

        let mut tasks = vec![
            iced::window::resize(id, visor::visor_size(monitor)),
            iced::window::move_to(id, Point::new(target.x, current_y)),
        ];
        if self.visor_visible {
            // Lets the user start typing immediately after summoning the visor.
            tasks.push(iced::window::gain_focus(id));
        }
        Task::batch(tasks)
    }

    pub(super) fn advance_animation(&mut self) -> Task<Message> {
        let Some(id) = self.window else {
            self.animation = None;
            return Task::none();
        };
        let Some(animation) = &self.animation else {
            return Task::none();
        };
        let point = Point::new(animation.x, animation.current_y());
        let finished = animation.is_finished();
        if finished {
            self.animation = None;
        }
        iced::window::move_to(id, point)
    }
}
