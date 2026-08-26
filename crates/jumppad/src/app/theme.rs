use super::*;

/// Mirror of `build_editor_overrides` for comment styles - built here so
/// `jumppad_textarea` doesn't need to depend on `jumppad_config`.
pub(super) fn build_comment_styles(
    config: &jumppad_config::Config,
) -> HashMap<String, jumppad_textarea::CommentStyle> {
    config
        .comment_styles_by_extension()
        .into_iter()
        .map(|(extension, style)| {
            let style = match style {
                jumppad_config::CommentSyntax::Single(prefix) => {
                    jumppad_textarea::CommentStyle::Single(prefix)
                }
                jumppad_config::CommentSyntax::Multi { left, right } => {
                    jumppad_textarea::CommentStyle::Multi { left, right }
                }
            };
            (extension, style)
        })
        .collect()
}

/// Mirror of `build_comment_styles` for `[indentation]`, and here for the
/// same reason: the widget crate names its own indentation types so it
/// doesn't have to depend on `jumppad_config`. The width is range-checked
/// on the way through, by the only constructor there is.
pub(super) fn build_indentation(
    config: &jumppad_config::Config,
) -> jumppad_textarea::Indentation {
    let style = match config.indentation.style {
        jumppad_config::IndentationStyle::Tabs => {
            jumppad_textarea::IndentationStyle::Tabs
        }
        jumppad_config::IndentationStyle::Spaces => {
            jumppad_textarea::IndentationStyle::Spaces
        }
    };
    jumppad_textarea::Indentation::new(style, config.indentation.width)
}

/// A reloaded setting that only applies at startup. Logged, not shown in
/// the banner: the change is valid, it just waits for the next start.
pub(super) fn restart_required(what: &str) {
    log::info!("{what} changed - takes effect on restart");
}

/// What the OS reports, in JumpPad's terms. `None` means it stated no
/// preference, which leaves `detection = "auto"` on the light slot.
pub(super) fn system_appearance(
    reported: iced::theme::Mode,
) -> Option<Appearance> {
    match reported {
        iced::theme::Mode::Light => Some(Appearance::Light),
        iced::theme::Mode::Dark => Some(Appearance::Dark),
        iced::theme::Mode::None => None,
    }
}

/// Matches a config-file palette name against `Theme::ALL` by display name
/// (`"Dracula"`, `"Solarized Light"`, ...), case-insensitively so hand-edited
/// TOML doesn't have to get the exact casing right. Falls back to `fallback`
/// - and logs why - rather than failing startup over a typo.
///
/// An `iced::Theme` comes back because that is how iced carries a palette:
/// each of its themes is one, named. JumpPad's own themes are the wider
/// thing, in `jumppad_config`.
pub(super) fn resolve_palette(name: &str, fallback: Theme) -> Theme {
    let theme = Theme::ALL
        .iter()
        .find(|theme| theme.to_string().eq_ignore_ascii_case(name.trim()));

    match theme {
        Some(theme) => theme.clone(),
        None => {
            let valid = Theme::ALL
                .iter()
                .map(Theme::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            log::warn!(
                "unknown palette {name:?}, using default. Valid options: {valid}"
            );
            fallback
        }
    }
}
