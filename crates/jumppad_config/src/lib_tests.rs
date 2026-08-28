use super::*;

#[test]
fn keybinds_toml_with_no_overrides_table_parses_as_empty() {
    let keybinds: KeybindsConfig =
        toml::from_str(r#"toggle = "control+Backquote""#).unwrap();
    assert!(keybinds.overrides.is_empty());
}

#[test]
fn keybinds_toml_with_overrides_table_parses_and_resolves() {
    let keybinds: KeybindsConfig = toml::from_str(
        r#"
        toggle = "control+Backquote"

        [overrides]
        new_tab = "control+alt+n"
        undo = "control+z"
        "#,
    )
    .unwrap();
    assert_eq!(keybinds.overrides.len(), 2);

    let resolved = keybinds.resolved_overrides();
    assert_eq!(
        resolved.get("new_tab"),
        Some(&ResolvedKeybind {
            modifiers: iced_core::keyboard::Modifiers::CTRL
                | iced_core::keyboard::Modifiers::ALT,
            code: iced_core::keyboard::key::Code::KeyN,
        })
    );
    assert_eq!(
        resolved.get("undo"),
        Some(&ResolvedKeybind {
            modifiers: iced_core::keyboard::Modifiers::CTRL,
            code: iced_core::keyboard::key::Code::KeyZ,
        })
    );
}

#[test]
fn a_malformed_chord_string_fails_the_whole_file_not_just_that_entry() {
    let result: Result<KeybindsConfig, _> = toml::from_str(
        r#"
        toggle = "control+Backquote"

        [overrides]
        new_tab = "not a real chord"
        "#,
    );
    assert!(result.is_err());
}

/// A config whose dark slot names `theme`, which the caller defines.
fn with_dark_theme(theme: &str) -> Config {
    toml::from_str(&format!(
        "[mode]\ndetection = \"dark\"\ntheme.dark = \"mine\"\n\n[themes.mine]\n{theme}"
    ))
    .unwrap()
}

fn config(toml: &str) -> Config {
    toml::from_str(toml).unwrap()
}

#[test]
fn an_unconfigured_gpu_asks_for_the_discrete_one() {
    // On Windows the adapter decides whether the window can be
    // translucent, so the default is the one likelier to allow it -
    // the same thing iced would have asked for unprompted.
    assert_eq!(config("").gpu.power, GpuPower::High);
    assert_eq!(
        config("").gpu.power.as_wgpu_power_pref(),
        "high",
        "the value wgpu is handed, not just the enum"
    );
}

#[test]
fn each_power_spelling_reaches_wgpu_as_its_own_answer() {
    for (written, expected, pref) in [
        ("low", GpuPower::Low, "low"),
        ("high", GpuPower::High, "high"),
        // wgpu spells "state no preference" as `none`, which is not the
        // same as asking for no GPU - hence the rename rather than
        // passing the config word straight through.
        ("auto", GpuPower::Auto, "none"),
    ] {
        let power = config(&format!("[gpu]\npower = \"{written}\"")).gpu.power;
        assert_eq!(power, expected, "parsing {written:?}");
        assert_eq!(power.as_wgpu_power_pref(), pref, "for {written:?}");
    }
}

#[test]
fn an_unknown_power_is_an_error_rather_than_a_silent_default() {
    // `load()` falls back to built-in defaults on a parse error and says
    // so; what it must not do is take "turbo" for "low" without a word.
    assert!(toml::from_str::<Config>("[gpu]\npower = \"turbo\"").is_err());
}

#[test]
fn the_default_gpu_section_stays_out_of_the_written_file() {
    // Same rule as every other section sitting on its default.
    let written = toml::to_string_pretty(&Config::default()).unwrap();
    assert!(!written.contains("[gpu]"), "got:\n{written}");
}

#[test]
fn a_named_power_round_trips_through_the_written_file() {
    let mut config = Config::default();
    config.gpu.power = GpuPower::Low;
    let written = toml::to_string_pretty(&config).unwrap();
    assert!(written.contains("[gpu]"), "got:\n{written}");
    let read: Config = toml::from_str(&written).unwrap();
    assert_eq!(read.gpu.power, GpuPower::Low);
}

#[test]
fn an_unconfigured_gpu_does_not_wait_for_the_display() {
    // The one default here that differs from what iced would do
    // unprompted, and the reason `jumppad-gpu` no longer trails the
    // pointer by a couple of frames on Windows. See `GpuConfig::vsync`.
    assert!(!config("").gpu.vsync);
}

#[test]
fn vsync_can_be_asked_for_again() {
    assert!(config("[gpu]\nvsync = true").gpu.vsync);
    assert!(!config("[gpu]\nvsync = false").gpu.vsync);
}

#[test]
fn asking_for_vsync_round_trips_through_the_written_file() {
    let mut config = Config::default();
    config.gpu.vsync = true;
    let written = toml::to_string_pretty(&config).unwrap();
    assert!(written.contains("[gpu]"), "got:\n{written}");
    let read: Config = toml::from_str(&written).unwrap();
    assert!(read.gpu.vsync);
}

#[test]
fn the_two_gpu_settings_are_independent() {
    // Each section is `#[serde(default)]`, so naming one setting must
    // leave the other at its own default rather than at whatever the
    // struct's `Default` would give a half-written section.
    let named_power = config("[gpu]\npower = \"low\"");
    assert_eq!(named_power.gpu.power, GpuPower::Low);
    assert!(!named_power.gpu.vsync);

    let named_vsync = config("[gpu]\nvsync = true");
    assert_eq!(named_vsync.gpu.power, GpuPower::High);
    assert!(named_vsync.gpu.vsync);
}

#[test]
fn a_theme_with_no_fonts_keeps_the_defaults_for_both() {
    let theme = with_dark_theme("").theme_for(Appearance::Dark);
    assert_eq!(theme.editor_font, ResolvedFont::default());
    assert_eq!(theme.ui_font, ResolvedFont::default());
    assert_eq!(theme.editor_font.family, None);
    assert_eq!(theme.editor_font.size, DEFAULT_FONT_SIZE);
    assert_eq!(theme.ui_font.family, None);
}

#[test]
fn the_editor_and_ui_fonts_are_named_independently() {
    let config = with_dark_theme(
        r#"
        editor.font.family = "JetBrains Mono"
        editor.font.size = 18.0
        ui.font.family = "Inter"
        ui.font.size = 13.0
        "#,
    );
    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.editor_font.family.as_deref(), Some("JetBrains Mono"));
    assert_eq!(theme.editor_font.size, 18.0);
    assert_eq!(theme.ui_font.family.as_deref(), Some("Inter"));
    assert_eq!(theme.ui_font.size, 13.0);
}

/// The rule that makes every property optional at every depth: naming
/// one leaves its siblings, and the other section, untouched.
#[test]
fn an_editor_size_alone_leaves_the_family_and_the_ui_font_default() {
    let config = with_dark_theme("editor.font.size = 13.5");
    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.editor_font.family, None);
    assert_eq!(theme.editor_font.size, 13.5);
    assert_eq!(theme.ui_font, ResolvedFont::default());
}

#[test]
fn a_slot_naming_nothing_of_its_own_is_the_plain_palette_for_it() {
    let config = Config::default();
    assert_eq!(
        config.theme_for(Appearance::Light),
        ResolvedTheme {
            palette: "light".to_string(),
            background_alpha: DEFAULT_ALPHA,
            background_blur: DEFAULT_BLUR,
            foreground_alpha: DEFAULT_ALPHA,
            editor_font: ResolvedFont::default(),
            line_numbers: ResolvedLineNumbers::default(),
            ui_font: ResolvedFont::default(),
        }
    );
    assert_eq!(config.theme_for(Appearance::Dark).palette, "dark");
}

#[test]
fn a_theme_with_no_palette_takes_the_one_for_its_slot() {
    let config: Config = toml::from_str(
        r#"
        [mode]
        theme.light = "mine"
        theme.dark = "mine"

        [themes.mine]
        background.alpha = 0.5
        "#,
    )
    .unwrap();
    assert_eq!(config.theme_for(Appearance::Light).palette, "light");
    assert_eq!(config.theme_for(Appearance::Dark).palette, "dark");
    // The rest of the theme is the same either way.
    assert_eq!(config.theme_for(Appearance::Dark).background_alpha, 0.5);
}

#[test]
fn a_theme_named_after_a_palette_wins_its_own_name() {
    let config: Config = toml::from_str(
        r#"
        [mode]
        theme.dark = "Dracula"

        [themes.Dracula]
        palette = "Nord"
        "#,
    )
    .unwrap();
    assert_eq!(config.theme_for(Appearance::Dark).palette, "Nord");
}

#[test]
fn a_slot_naming_no_theme_is_read_as_a_palette() {
    let config: Config =
        toml::from_str("[mode]\ntheme.dark = \"Tokyo Night Storm\"").unwrap();
    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.palette, "Tokyo Night Storm");
    assert_eq!(theme.background_alpha, DEFAULT_ALPHA);
    assert_eq!(theme.foreground_alpha, DEFAULT_ALPHA);
    assert_eq!(theme.editor_font, ResolvedFont::default());
    assert_eq!(theme.ui_font, ResolvedFont::default());
}

/// The example the base theme exists for: themes named after the slots
/// they fill, sharing a palette, differing only where they say so.
#[test]
fn a_file_of_themes_alone_needs_no_mode_section() {
    let config = config(
        r#"
        [themes.base]
        palette = "Ferra"

        [themes.dark]
        background.alpha = 0.95

        [themes.light]
        background.alpha = 1.0
        "#,
    );

    let light = config.theme_for(Appearance::Light);
    assert_eq!(light.palette, "Ferra");
    assert_eq!(light.background_alpha, 1.0);

    let dark = config.theme_for(Appearance::Dark);
    assert_eq!(dark.palette, "Ferra");
    assert_eq!(dark.background_alpha, 0.95);
}

#[test]
fn the_slots_default_to_the_theme_names_light_and_dark() {
    assert_eq!(ThemeSlots::default().light, "light");
    assert_eq!(ThemeSlots::default().dark, "dark");
}

#[test]
fn a_theme_takes_every_property_the_base_theme_names() {
    let config = config(
        r#"
        [mode]
        theme.dark = "mine"

        [themes.base]
        palette = "Nord"
        background.alpha = 0.8
        foreground.alpha = 0.9
        editor.font.family = "JetBrains Mono"
        editor.font.size = 18.0
        ui.font.family = "Inter"
        ui.font.size = 13.0

        [themes.mine]
        "#,
    );

    assert_eq!(
        config.theme_for(Appearance::Dark),
        ResolvedTheme {
            palette: "Nord".to_string(),
            background_alpha: 0.8,
            background_blur: DEFAULT_BLUR,
            foreground_alpha: 0.9,
            editor_font: ResolvedFont {
                family: Some("JetBrains Mono".to_string()),
                size: 18.0,
            },
            line_numbers: ResolvedLineNumbers::default(),
            ui_font: ResolvedFont {
                family: Some("Inter".to_string()),
                size: 13.0,
            },
        }
    );
}

#[test]
fn a_themes_own_property_beats_the_base_themes() {
    let config = config(
        r#"
        [mode]
        theme.dark = "mine"

        [themes.base]
        palette = "Nord"
        editor.font.size = 18.0

        [themes.mine]
        palette = "Dracula"
        editor.font.size = 21.0
        "#,
    );

    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.palette, "Dracula");
    assert_eq!(theme.editor_font.size, 21.0);
}

#[test]
fn a_document_is_unnumbered_until_a_theme_asks_for_numbers() {
    let numbers = Config::default().theme_for(Appearance::Dark).line_numbers;

    assert!(!numbers.enabled);
    assert_eq!(numbers.alpha, DEFAULT_LINE_NUMBERS_ALPHA);
}

/// Each leaf on its own, the way `editor.font` merges: a theme that only
/// turns the numbers off has no business losing the base theme's alpha.
#[test]
fn a_theme_takes_the_line_numbers_it_does_not_name_from_the_base() {
    let config = config(
        r#"
        [mode]
        theme.dark = "mine"

        [themes.base]
        editor.line_numbers.enabled = true
        editor.line_numbers.alpha = 0.3

        [themes.mine]
        editor.line_numbers.enabled = false
        "#,
    );

    let numbers = config.theme_for(Appearance::Dark).line_numbers;
    assert!(!numbers.enabled);
    assert_eq!(numbers.alpha, 0.3);
}

/// The reason every leaf is an `Option`: a theme naming the value that
/// happens to be JumpPad's own default still has to beat a base theme
/// that named something else.
#[test]
fn a_theme_can_show_numbers_a_base_theme_turned_off() {
    let config = config(
        r#"
        [themes.base]
        editor.line_numbers.enabled = false

        [themes.light]
        editor.line_numbers.enabled = true
        "#,
    );

    assert!(config.theme_for(Appearance::Light).line_numbers.enabled);
}

/// The reason every leaf is an `Option`: a theme naming the value that
/// happens to be JumpPad's own default still has to beat a base theme
/// that named something else.
#[test]
fn a_theme_can_be_solid_over_a_translucent_base() {
    let config = config(
        r#"
        [themes.base]
        background.alpha = 0.95

        [themes.light]
        background.alpha = 1.0
        "#,
    );

    assert_eq!(config.theme_for(Appearance::Light).background_alpha, 1.0);
    assert_eq!(config.theme_for(Appearance::Dark).background_alpha, 0.95);
}

/// Property by property, not section by section: naming a size doesn't
/// discard the family beside it.
#[test]
fn the_base_theme_and_a_theme_merge_property_by_property() {
    let config = config(
        r#"
        [themes.base]
        editor.font.family = "JetBrains Mono"
        ui.font.family = "Inter"

        [themes.dark]
        editor.font.size = 21.0
        "#,
    );

    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.editor_font.family.as_deref(), Some("JetBrains Mono"));
    assert_eq!(theme.editor_font.size, 21.0);
    assert_eq!(theme.ui_font.family.as_deref(), Some("Inter"));
    assert_eq!(theme.ui_font.size, DEFAULT_FONT_SIZE);
}

#[test]
fn what_neither_the_theme_nor_the_base_names_takes_the_stock_default() {
    let config = config(
        r#"
        [themes.base]
        background.alpha = 0.8

        [themes.dark]
        "#,
    );

    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.palette, "dark");
    assert_eq!(theme.foreground_alpha, DEFAULT_ALPHA);
    assert_eq!(theme.editor_font, ResolvedFont::default());
}

#[test]
fn a_slot_naming_no_theme_still_takes_the_base_themes_properties() {
    let config = config(
        r#"
        [mode]
        theme.dark = "Tokyo Night Storm"

        [themes.base]
        background.alpha = 0.8
        editor.font.size = 18.0
        "#,
    );

    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.palette, "Tokyo Night Storm");
    assert_eq!(theme.background_alpha, 0.8);
    assert_eq!(theme.editor_font.size, 18.0);
}

/// A slot naming a palette is read as a theme naming that palette, so it
/// outranks the base theme's the way any other named property does.
#[test]
fn a_slot_naming_a_palette_outranks_the_base_themes_palette() {
    let config = config(
        r#"
        [mode]
        theme.dark = "Nord"

        [themes.base]
        palette = "Ferra"
        "#,
    );

    assert_eq!(config.theme_for(Appearance::Dark).palette, "Nord");
}

#[test]
fn the_base_theme_can_be_shown_in_a_slot_of_its_own() {
    let config = config(
        r#"
        [mode]
        theme.dark = "base"

        [themes.base]
        palette = "Ferra"
        background.alpha = 0.8
        "#,
    );

    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.palette, "Ferra");
    assert_eq!(theme.background_alpha, 0.8);
}

#[test]
fn an_empty_theme_table_changes_nothing() {
    let empty_base = config(
        r#"
        [themes.base]

        [themes.dark]
        background.alpha = 0.5
        "#,
    );
    let no_base = config("[themes.dark]\nbackground.alpha = 0.5");

    assert_eq!(
        empty_base.theme_for(Appearance::Dark),
        no_base.theme_for(Appearance::Dark)
    );
}

fn blur_of(toml: &str) -> Blur {
    config(&format!("[themes.dark]\n{toml}"))
        .theme_for(Appearance::Dark)
        .background_blur
}

#[test]
fn a_theme_with_no_blur_named_leaves_the_desktop_sharp() {
    assert_eq!(blur_of("background.alpha = 0.8"), Blur::None);
    assert_eq!(DEFAULT_BLUR, Blur::None);
}

/// `0` and `"none"` are the same setting written two ways, so they stop
/// being two the moment the file is read.
#[test]
fn zero_and_none_are_the_same_answer() {
    assert_eq!(blur_of("background.blur = 0"), Blur::None);
    assert_eq!(blur_of(r#"background.blur = "none""#), Blur::None);
}

#[test]
fn a_theme_can_ask_for_a_frosted_desktop() {
    let theme = config(
        r#"
        [themes.dark]
        background.alpha = 0.8
        background.blur = 24
        "#,
    )
    .theme_for(Appearance::Dark);

    assert_eq!(theme.background_blur, Blur::Radius(24));
    assert_eq!(theme.background_alpha, 0.8);
}

#[test]
fn each_acrylic_name_is_its_own_answer() {
    assert_eq!(blur_of(r#"background.blur = "acrylic10""#), Blur::Acrylic10);
    assert_eq!(blur_of(r#"background.blur = "acrylic11""#), Blur::Acrylic11);
}

/// Which forms mean anything is the platform's to decide, so this crate
/// carries every one of them whole - a radius the reader's platform
/// cannot use included.
#[test]
fn every_form_survives_being_resolved() {
    assert_eq!(blur_of("background.blur = 4000"), Blur::Radius(4000));
}

/// The same per-leaf inheritance the alpha beside it gets: a shared
/// blur, and one slot that opts back out of it.
#[test]
fn blur_inherits_from_the_base_theme_and_a_theme_can_turn_it_off() {
    let config = config(
        r#"
        [themes.base]
        background.alpha = 0.8
        background.blur = "acrylic11"

        [themes.dark]

        [themes.light]
        background.blur = "none"
        "#,
    );

    assert_eq!(
        config.theme_for(Appearance::Dark).background_blur,
        Blur::Acrylic11
    );
    assert_eq!(
        config.theme_for(Appearance::Light).background_blur,
        Blur::None
    );
}

/// A negative radius is not a smaller one, and the file says so rather
/// than rounding it into a setting nobody asked for.
#[test]
fn a_negative_blur_fails_the_file_and_says_why() {
    let err = toml::from_str::<Config>("[themes.dark]\nbackground.blur = -1")
        .unwrap_err()
        .to_string();
    assert!(err.contains("negative"), "unhelpful error: {err}");
}

/// The reason the parse is hand-written: a misspelled name names the
/// ones that would have worked.
#[test]
fn an_unknown_blur_name_lists_the_ones_there_are() {
    let err = toml::from_str::<Config>(
        r#"[themes.dark]
        background.blur = "arcylic10""#,
    )
    .unwrap_err()
    .to_string();

    assert!(err.contains("arcylic10"), "unhelpful error: {err}");
    assert!(err.contains("acrylic11"), "unhelpful error: {err}");
    assert!(err.contains("none"), "unhelpful error: {err}");
}

/// Written back out as the same setting, so a config file this crate
/// rewrites doesn't turn a name into a number or the other way round.
#[test]
fn each_blur_form_round_trips() {
    for written in ["24", r#""none""#, r#""acrylic10""#, r#""acrylic11""#] {
        let config =
            config(&format!("[themes.dark]\nbackground.blur = {written}"));
        let round_tripped: Config =
            toml::from_str(&toml::to_string(&config).unwrap()).unwrap();

        assert_eq!(round_tripped, config, "{written} did not survive");
    }
}

#[test]
fn pinned_detection_ignores_what_the_os_reports() {
    let pinned: ModeConfig = toml::from_str(r#"detection = "dark""#).unwrap();
    assert_eq!(pinned.showing(Some(Appearance::Light)), Appearance::Dark);
    assert_eq!(pinned.showing(None), Appearance::Dark);
}

#[test]
fn automatic_detection_follows_the_os_and_defaults_to_light() {
    let auto = ModeConfig::default();
    assert_eq!(auto.detection, Detection::Auto);
    assert_eq!(auto.showing(Some(Appearance::Dark)), Appearance::Dark);
    assert_eq!(auto.showing(Some(Appearance::Light)), Appearance::Light);
    // No preference reported, or none readable.
    assert_eq!(auto.showing(None), Appearance::Light);
}

/// Over every theme, not just the two the slots name: `[mode]` is
/// live-editable, so any theme in the file can reach the screen without
/// a restart, and the window's transparency cannot follow it there.
#[test]
fn transparency_is_wanted_if_any_theme_at_all_asks_for_it() {
    assert!(!Config::default().wants_transparency());

    let solid = with_dark_theme("background.alpha = 1.0");
    assert!(!solid.wants_transparency());

    let unnamed: Config = toml::from_str(
        r#"
        [themes.never_shown]
        background.alpha = 0.9
        "#,
    )
    .unwrap();
    assert!(unnamed.wants_transparency());
}

#[test]
fn a_translucent_base_theme_wants_transparency() {
    assert!(
        config("[themes.base]\nbackground.alpha = 0.9").wants_transparency()
    );
}

/// The base theme is one of these entries, and `[mode]` is live-editable,
/// so a slot can name it after the window has already been created.
#[test]
fn a_base_theme_every_other_theme_overrides_still_wants_transparency() {
    let overridden = config(
        r#"
        [themes.base]
        background.alpha = 0.95

        [themes.light]
        background.alpha = 1.0

        [themes.dark]
        background.alpha = 1.0
        "#,
    );
    assert!(overridden.wants_transparency());
}

/// Everything in the default config is a default, so the file written on
/// first run is the one section that isn't: the languages.
#[test]
fn the_written_default_file_carries_only_the_languages() {
    let written = toml::to_string_pretty(&Config::default()).unwrap();
    for defaulted in [
        "[[languages]]",
        "[mode]",
        "[themes]",
        "[visor]",
        "[window]",
        "[scroll]",
        "[history]",
        "[files]",
        "detection",
        "palette",
        "family",
    ] {
        assert!(
            !written.contains(defaulted),
            "{defaulted} sits on its default and shouldn't be written"
        );
    }
}

#[test]
fn default_keybinds_config_has_no_overrides() {
    assert!(KeybindsConfig::default().overrides.is_empty());
}

/// A throwaway file that cleans up after itself, so the `try_parse`
/// tests can exercise the real read path.
struct TempFile(PathBuf);

impl TempFile {
    fn with_contents(name: &str, contents: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("jumppad_config_test_{name}"));
        std::fs::write(&path, contents).unwrap();
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn try_parse_reads_the_first_existing_candidate() {
    let file = TempFile::with_contents(
        "valid.toml",
        "[mode]\ntheme.dark = \"Dracula\"",
    );
    let missing = PathBuf::from("does_not_exist/config.toml");
    let config: Config = try_parse(&[missing, file.0.clone()]).unwrap();
    assert_eq!(config.mode.theme.dark, "Dracula");
}

#[test]
fn try_parse_with_no_existing_candidate_is_missing() {
    let result: Result<Config, _> =
        try_parse(&[PathBuf::from("does_not_exist/config.toml")]);
    assert!(matches!(result, Err(ReloadError::Missing)));
}

/// The deliberate contrast with `load()`: a broken file is an error the
/// caller can react to, not a silent reset to defaults.
#[test]
fn try_parse_surfaces_a_parse_error_instead_of_defaulting() {
    let file = TempFile::with_contents("broken.toml", "[mode]\ndetection = ");
    let result: Result<Config, _> = try_parse(std::slice::from_ref(&file.0));
    assert!(matches!(result, Err(ReloadError::Parse(_))));
}

#[test]
fn try_parse_surfaces_a_malformed_chord_as_a_parse_error() {
    let file = TempFile::with_contents(
        "bad_chord.toml",
        r#"
        toggle = "control+Backquote"

        [overrides]
        new_tab = "not a real chord"
        "#,
    );
    let result: Result<KeybindsConfig, _> =
        try_parse(std::slice::from_ref(&file.0));
    assert!(matches!(result, Err(ReloadError::Parse(_))));
}

#[test]
fn an_unnamed_alpha_resolves_to_fully_solid() {
    let theme = with_dark_theme("").theme_for(Appearance::Dark);
    assert_eq!(theme.background_alpha, 1.0);
    assert_eq!(theme.foreground_alpha, 1.0);
    assert_eq!(DEFAULT_ALPHA, 1.0);
}

#[test]
fn config_toml_with_no_window_section_keeps_decorations() {
    // Old config files predate the section and must stay valid.
    let config: Config = toml::from_str("").unwrap();
    assert_eq!(config.window, WindowConfig::default());
    assert!(config.window.decorations);
}

#[test]
fn config_toml_can_turn_decorations_off() {
    let config: Config = toml::from_str(
        r#"

        [window]
        decorations = false
        "#,
    )
    .unwrap();
    assert!(!config.window.decorations);
}

#[test]
fn comment_single_and_multi_together_fail_the_parse() {
    let result: Result<Config, _> = toml::from_str(
        r#"
        [[languages]]
        name = "Broken"
        extensions = ["x"]
        comment.single = "// "
        comment.multi.left = "<!--"
        comment.multi.right = "-->"
        "#,
    );
    let error = result.unwrap_err().to_string();
    assert!(error.contains("mutually exclusive"), "got: {error}");
}

#[test]
fn an_empty_comment_table_fails_the_parse() {
    let result: Result<Config, _> = toml::from_str(
        r#"
        [[languages]]
        name = "Broken"
        extensions = ["x"]

        [languages.comment]
        "#,
    );
    assert!(result.is_err());
}

#[test]
fn a_language_without_a_comment_key_parses_as_none() {
    let config: Config = toml::from_str(
        r#"
        [[languages]]
        name = "Plain"
        extensions = ["txt"]
        "#,
    )
    .unwrap();
    assert_eq!(config.languages[0].comment, None);
    assert_eq!(config.languages[0].syntax, None);
    assert_eq!(
        config.languages[0].extensions.as_deref(),
        Some(["txt".to_string()].as_slice())
    );
}

#[test]
fn comment_multi_parses_the_dotted_key_form() {
    let config: Config = toml::from_str(
        r#"
        [[languages]]
        name = "HTML"
        syntax = "html"
        extensions = ["htm", "html", "xhtml"]
        comment.multi.left = "<!--"
        comment.multi.right = "-->"
        "#,
    )
    .unwrap();
    assert_eq!(
        config.languages[0].comment,
        Some(CommentSyntax::Multi {
            left: "<!--".to_string(),
            right: "-->".to_string()
        })
    );
}

#[test]
fn an_old_config_with_syntaxes_and_comment_styles_still_parses() {
    // Pre-[[languages]] sections are ignored unknowns: the file loads,
    // and those customizations fall back to whatever the bundles ship.
    let config: Config = toml::from_str(
        r#"

        [syntaxes]
        toml = ["toml"]

        [[comment_styles]]
        syntaxes = ["toml"]
        prefix = "; "
        "#,
    )
    .unwrap();
    assert!(
        config.languages.is_empty(),
        "neither old section contributes a language"
    );
}

/// Guards the first-run `write_default` path: the default config -
/// array-of-tables included - must serialize and parse back unchanged.
#[test]
fn default_config_round_trips_through_toml() {
    let written = toml::to_string_pretty(&Config::default()).unwrap();
    let reparsed: Config = toml::from_str(&written).unwrap();
    assert_eq!(reparsed, Config::default());
}

/// Every theme leaf is skipped when unset, so an untouched section writes
/// as a bare header - which still has to read back as the same theme.
#[test]
fn a_theme_with_nothing_set_round_trips_through_toml() {
    let mut config = Config::default();
    config
        .themes
        .insert(BASE_THEME.to_string(), ThemeConfig::default());

    let written = toml::to_string_pretty(&config).unwrap();
    let reparsed: Config = toml::from_str(&written).unwrap();
    assert_eq!(reparsed, config);
}

#[test]
fn the_sample_files_parse() {
    let config: Config =
        toml::from_str(include_str!("../../../config/config.sample.toml"))
            .unwrap();
    assert!(!config.languages.is_empty());
    // Parsing is not enough on its own: an unknown key is skipped in
    // silence and leaves the default standing, so a sample naming a key
    // that no longer exists would still parse and still be wrong. The
    // sample sets both indentation keys away from their defaults, which
    // is what makes them worth reading back.
    assert_eq!(config.indentation.style, IndentationStyle::Spaces);
    assert_eq!(config.indentation.width, 2);
    // Same again: the sample's separator list is the default with `-`
    // taken out of it.
    assert!(!config.words.separators.contains('-'));
    assert!(config.words.separators.contains('.'));
    // And again: the sample turns the line numbers on, which is not the
    // default, so a renamed key would show up here.
    let numbers = config.theme_for(Appearance::Dark).line_numbers;
    assert!(numbers.enabled);
    assert_eq!(numbers.alpha, 0.45);
    let _: KeybindsConfig =
        toml::from_str(include_str!("../../../config/keybinds.sample.toml"))
            .unwrap();
}

#[test]
fn a_mode_pins_a_slot_only_when_it_names_one() {
    let pinning = |detection| ModeConfig {
        detection,
        ..ModeConfig::default()
    };

    assert_eq!(pinning(Detection::Auto).pinned(), None);
    assert_eq!(pinning(Detection::Light).pinned(), Some(Appearance::Light));
    assert_eq!(pinning(Detection::Dark).pinned(), Some(Appearance::Dark));
}

/// A pinned mode shows what it pins, whatever the OS says - so the two
/// answers agree wherever both are asked.
#[test]
fn a_pinned_mode_shows_what_it_pins() {
    for detection in [Detection::Light, Detection::Dark] {
        let mode = ModeConfig {
            detection,
            ..ModeConfig::default()
        };

        for os in [None, Some(Appearance::Light), Some(Appearance::Dark)] {
            assert_eq!(Some(mode.showing(os)), mode.pinned());
        }
    }
}

#[test]
fn config_toml_with_no_scroll_section_falls_back_to_the_shipped_speed() {
    let config: Config = toml::from_str("").unwrap();
    assert_eq!(config.scroll, ScrollConfig::default());
    assert_eq!(config.scroll.sensitivity, 1.0);
    assert_eq!(config.scroll.drag_speed, 1.0);
}

#[test]
fn config_toml_with_a_scroll_section_parses() {
    let config: Config = toml::from_str(
        r#"

        [scroll]
        sensitivity = 2.5
        drag_speed = 0.5
        "#,
    )
    .unwrap();
    assert_eq!(config.scroll.sensitivity, 2.5);
    assert_eq!(config.scroll.drag_speed, 0.5);
}

#[test]
fn one_scroll_speed_can_be_set_without_the_other() {
    let config: Config = toml::from_str(
        r#"

        [scroll]
        drag_speed = 0.25
        "#,
    )
    .unwrap();
    assert_eq!(config.scroll.drag_speed, 0.25);
    assert_eq!(config.scroll.sensitivity, 1.0);
}

#[test]
fn config_toml_with_no_files_section_asks_before_overwriting() {
    let config: Config = toml::from_str("").unwrap();
    assert_eq!(config.files, FilesConfig::default());
    assert!(config.files.save_conflict_resolution.asks());
}

#[test]
fn config_toml_can_ask_for_saves_that_always_win() {
    let config: Config = toml::from_str(
        r#"

        [files]
        save_conflict_resolution = "overwrite"
        "#,
    )
    .unwrap();
    assert_eq!(
        config.files.save_conflict_resolution,
        SaveConflictResolution::Overwrite
    );
    assert!(!config.files.save_conflict_resolution.asks());
}

#[test]
fn config_toml_with_no_indentation_section_indents_with_tabs() {
    let config: Config = toml::from_str("").unwrap();
    assert_eq!(config.indentation, IndentationConfig::default());
    assert_eq!(config.indentation.style, IndentationStyle::Tabs);
    assert_eq!(config.indentation.width, 4);
}

#[test]
fn config_toml_can_ask_for_spaces() {
    let config: Config = toml::from_str(
        r#"

        [indentation]
        style = "spaces"
        width = 2
        "#,
    )
    .unwrap();
    assert_eq!(config.indentation.style, IndentationStyle::Spaces);
    assert_eq!(config.indentation.width, 2);
}

#[test]
fn an_indentation_width_can_be_set_without_the_style() {
    let config: Config = toml::from_str(
        r#"

        [indentation]
        width = 8
        "#,
    )
    .unwrap();
    assert_eq!(config.indentation.style, IndentationStyle::Tabs);
    assert_eq!(config.indentation.width, 8);
}

#[test]
fn an_unknown_indentation_style_fails_the_files_parse() {
    // Loudly, like every other string-valued enum: a typo here would
    // otherwise indent with something the user never asked for.
    let parsed: Result<Config, _> = toml::from_str(
        r#"

        [indentation]
        style = "tab"
        "#,
    );
    assert!(parsed.is_err());
}

#[test]
fn config_toml_with_no_words_section_separates_words_like_vs_code() {
    let config: Config = toml::from_str("").unwrap();
    assert_eq!(config.words, WordsConfig::default());
    assert_eq!(config.words.separators, DEFAULT_WORD_SEPARATORS);
    // The characters that make a `foo.bar(baz)` four words rather than
    // one, spot-checked so a mangled escape in the literal shows up.
    for separator in ['.', '(', ')', '"', '\'', '\\', '-'] {
        assert!(
            config.words.separators.contains(separator),
            "{separator:?} should be a word separator by default"
        );
    }
}

#[test]
fn a_words_section_replaces_the_whole_list() {
    let config: Config = toml::from_str(
        r#"

        [words]
        separators = ".,"
        "#,
    )
    .unwrap();
    assert_eq!(config.words.separators, ".,");
}

/// A list can be emptied, which leaves whitespace as the only thing that
/// ends a word - the widget's rule, not something this file can turn off.
#[test]
fn an_empty_separator_list_is_a_setting_rather_than_a_missing_one() {
    let config: Config = toml::from_str(
        r#"

        [words]
        separators = ""
        "#,
    )
    .unwrap();
    assert_eq!(config.words.separators, "");
}

#[test]
fn a_theme_with_no_alpha_section_falls_back_to_solid() {
    let theme = with_dark_theme("").theme_for(Appearance::Dark);
    assert_eq!(theme.background_alpha, DEFAULT_ALPHA);
    assert_eq!(theme.foreground_alpha, DEFAULT_ALPHA);
}

#[test]
fn a_theme_can_set_either_surface_alpha() {
    let config = with_dark_theme(
        r#"
        background.alpha = 0.7
        foreground.alpha = 0.9
        "#,
    );
    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.background_alpha, 0.7);
    assert_eq!(theme.foreground_alpha, 0.9);
}

/// The surfaces are separate sections, so naming one says nothing about
/// the other.
#[test]
fn a_theme_naming_one_surface_leaves_the_other_solid() {
    let config = with_dark_theme("background.alpha = 0.5");
    let theme = config.theme_for(Appearance::Dark);
    assert_eq!(theme.background_alpha, 0.5);
    assert_eq!(theme.foreground_alpha, DEFAULT_ALPHA);
}
