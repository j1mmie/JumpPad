use super::*;
use editor_core::WASH_ALPHA_CEILING;
use editor_core::{SavedSelection, SelectionKind, TextEditorWidget};
use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use jumppad_config::Blur;

/// A minimal `TextEditorWidget` for tests, with no real rendering - it
/// holds its text so file-backed flows can assert on what landed in the
/// buffer.
struct StubEditor(String);

impl TextEditorWidget for StubEditor {
    fn view(&self) -> Element<'_, EditorMessage> {
        iced::widget::text("").into()
    }
    fn update(&mut self, _message: EditorMessage) -> bool {
        true // reports every message as an edit
    }
    fn text(&self) -> String {
        self.0.clone()
    }
    fn set_text(&mut self, text: &str) {
        self.0 = text.to_string();
    }
    fn reload_text(&mut self, text: &str) {
        self.0 = text.to_string();
    }
    fn poll_highlighting(&mut self) {}
    fn cursor_position(&self) -> (usize, usize) {
        (0, 0)
    }
    fn move_cursor_to(&mut self, _line: usize, _column: usize) {}
    fn selection(&self) -> Option<SavedSelection> {
        None
    }
    fn restore_selection(
        &mut self,
        _selection: SavedSelection,
        _cursor: (usize, usize),
    ) {
    }
    fn set_find_matches(
        &mut self,
        _matches: Vec<editor_core::FindMatch>,
        _current: Option<usize>,
    ) {
    }
    fn has_pending_highlighting(&self) -> bool {
        false
    }
}

/// The close prompt currently showing, for tests that assert on which
/// tab it's for and where its focus sits.
fn close_prompt(app: &JumpPadApp) -> Option<&PendingClose> {
    match &app.modal {
        Some(Modal::Close(pending)) => Some(pending),
        _ => None,
    }
}

/// Mirror of `close_prompt` for the save-conflict dialog.
fn conflict_prompt(app: &JumpPadApp) -> Option<&PendingConflict> {
    match &app.modal {
        Some(Modal::SaveConflict(pending)) => Some(pending),
        _ => None,
    }
}

fn stub_factory() -> EditorFactory {
    Box::new(|text, _extension| {
        Box::new(StubEditor(text.to_string())) as Box<dyn TextEditorWidget>
    })
}

/// Records every message it's handed, for asserting on what the app
/// actually dispatches to an editor.
struct RecordingEditor(std::rc::Rc<std::cell::RefCell<Vec<EditorMessage>>>);

impl TextEditorWidget for RecordingEditor {
    fn view(&self) -> Element<'_, EditorMessage> {
        iced::widget::text("").into()
    }
    fn update(&mut self, message: EditorMessage) -> bool {
        self.0.borrow_mut().push(message);
        false
    }
    fn text(&self) -> String {
        String::new()
    }
    fn set_text(&mut self, _text: &str) {}
    fn reload_text(&mut self, _text: &str) {}
    fn poll_highlighting(&mut self) {}
    fn cursor_position(&self) -> (usize, usize) {
        (0, 0)
    }
    fn move_cursor_to(&mut self, _line: usize, _column: usize) {}
    fn selection(&self) -> Option<SavedSelection> {
        None
    }
    fn restore_selection(
        &mut self,
        _selection: SavedSelection,
        _cursor: (usize, usize),
    ) {
    }
    fn set_find_matches(
        &mut self,
        _matches: Vec<editor_core::FindMatch>,
        _current: Option<usize>,
    ) {
    }
    fn has_pending_highlighting(&self) -> bool {
        false
    }
}

/// What `switch_active` restored into an editor - see `SelectionSpyEditor`.
#[derive(Debug, PartialEq)]
enum Restore {
    Cursor(usize, usize),
    Selection {
        selection: SavedSelection,
        cursor: (usize, usize),
    },
}

/// Reports a canned cursor/selection and records what the app restores,
/// for asserting on `switch_active`'s save/restore orchestration.
struct SelectionSpyEditor {
    selection: Option<SavedSelection>,
    cursor: (usize, usize),
    restored: std::rc::Rc<std::cell::RefCell<Vec<Restore>>>,
}

impl TextEditorWidget for SelectionSpyEditor {
    fn view(&self) -> Element<'_, EditorMessage> {
        iced::widget::text("").into()
    }
    fn update(&mut self, _message: EditorMessage) -> bool {
        false
    }
    fn text(&self) -> String {
        String::new()
    }
    fn set_text(&mut self, _text: &str) {}
    fn reload_text(&mut self, _text: &str) {}
    fn poll_highlighting(&mut self) {}
    fn cursor_position(&self) -> (usize, usize) {
        self.cursor
    }
    fn move_cursor_to(&mut self, line: usize, column: usize) {
        self.restored
            .borrow_mut()
            .push(Restore::Cursor(line, column));
    }
    fn selection(&self) -> Option<SavedSelection> {
        self.selection
    }
    fn restore_selection(
        &mut self,
        selection: SavedSelection,
        cursor: (usize, usize),
    ) {
        self.restored
            .borrow_mut()
            .push(Restore::Selection { selection, cursor });
    }
    fn set_find_matches(
        &mut self,
        _matches: Vec<editor_core::FindMatch>,
        _current: Option<usize>,
    ) {
    }
    fn has_pending_highlighting(&self) -> bool {
        false
    }
}

/// Builds an `JumpPadApp` with `tab_count` untitled tabs, skipping the real I/O `new()` does.
fn test_app(tab_count: u64) -> JumpPadApp {
    let factory = stub_factory();
    let tabs = (0..tab_count)
        .map(|id| Tab::untitled(id, &factory))
        .collect();
    JumpPadApp {
        tabs,
        languages: jumppad_config::Languages::default(),
        grammar_search_dirs: Vec::new(),
        active: 0,
        next_id: tab_count,
        error: None,
        editor_factory: factory,
        showing: Appearance::Light,
        os_appearance: None,
        theme: Theme::ALL[0].clone(),
        ui_text: UiText::new(
            Font::DEFAULT,
            jumppad_config::DEFAULT_FONT_SIZE,
        ),
        background_alpha: 1.0,
        background_blur: jumppad_config::DEFAULT_BLUR,
        shadow_refresh_frames: 0,
        surface_reset_frames: 0,
        session_dir: PathBuf::from("/tmp"),
        pending_close_after_save: Vec::new(),
        window: None,
        hotkey: None,
        visor_visible: false,
        animation: None,
        visor_enabled: false,
        previous_active_id: None,
        keybind_overrides: Arc::new(HashMap::new()),
        modal: None,
        close_queue: Vec::new(),
        conflict_queue: Vec::new(),
        file_dialog_active: false,
        files_hovered: false,
        find: HashMap::new(),
        modifiers: Modifiers::default(),
        config: jumppad_config::Config::default(),
        keybinds: jumppad_config::KeybindsConfig::default(),
        editor_config: jumppad_textarea::SharedEditorConfig::new(
            1.0,
            Arc::new(|_: &jumppad_textarea::KeyPress| None),
        ),
        config_watch: reload::ConfigWatch::new(),
        document_watch: docwatch::DocumentWatch::new(),
    }
}

/// End-to-end scenario against real `TextArea`s (not stubs): two
/// tabs make selections in turn, and each keeps its own through switches.
#[test]
fn each_tab_keeps_its_own_selection_through_switches() {
    use iced::widget::text_editor::{Action, Motion};

    let factory: EditorFactory = Box::new(|text, _extension| {
        let registry = syntax_registry::SyntaxRegistry::new(
            Vec::new(),
            syntax_registry::GrammarLookup::default(),
            || {},
        );
        Box::new(jumppad_textarea::TextArea::new(
            text,
            &registry,
            None,
            jumppad_textarea::SharedEditorConfig::new(
                1.0,
                Arc::new(|_: &jumppad_textarea::KeyPress| None),
            ),
        ))
    });
    let mut app = test_app(0);
    app.tabs = vec![
        Tab::restored(0, None, "alpha beta", false, &factory),
        Tab::restored(1, None, "gamma delta", false, &factory),
    ];
    app.next_id = 2;

    // Tab 0: a double-click-style word selection (Click then SelectWord,
    // the sequence the widget publishes). Which word it lands on is the
    // hit test's business - it depends on the font the machine has - so
    // what is asserted here is that a word was taken, and below that the
    // tab still has it after two switches.
    let _ = app.update(Message::Editor(
        0,
        EditorMessage::Action(Action::Click(iced::Point::new(10.0, 4.0))),
    ));
    let _ = app.update(Message::Editor(
        0,
        EditorMessage::Action(Action::SelectWord),
    ));
    let word = app.tabs[0].editor.selection();
    let word_cursor = app.tabs[0].editor.cursor_position();
    assert!(
        matches!(
            word,
            Some(SavedSelection {
                kind: SelectionKind::Range,
                ..
            })
        ),
        "expected a word selection, got {word:?}"
    );

    // Tab 1: a keyboard range selection over "ga".
    let _ = app.update(Message::SelectTab(1));
    for _ in 0..2 {
        let _ = app.update(Message::Editor(
            1,
            EditorMessage::Action(Action::Select(Motion::Right)),
        ));
    }

    // Both tabs hold their own selection, verified across two round trips.
    let _ = app.update(Message::SelectTab(0));
    assert_eq!(app.tabs[0].editor.selection(), word);
    assert_eq!(app.tabs[0].editor.cursor_position(), word_cursor);

    let _ = app.update(Message::SelectTab(1));
    assert_eq!(
        app.tabs[1].editor.selection(),
        Some(SavedSelection {
            anchor: (0, 0),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(app.tabs[1].editor.cursor_position(), (0, 2));
}

/// Same scenario as above, but with the message sequence a real mouse
/// selection produces (Click then Drags), exercising pixel hit-testing.
#[test]
fn mouse_made_selections_stay_independent_per_tab() {
    use iced::Point;
    use iced::widget::text_editor::Action;

    let factory: EditorFactory = Box::new(|text, _extension| {
        let registry = syntax_registry::SyntaxRegistry::new(
            Vec::new(),
            syntax_registry::GrammarLookup::default(),
            || {},
        );
        Box::new(jumppad_textarea::TextArea::new(
            text,
            &registry,
            None,
            jumppad_textarea::SharedEditorConfig::new(
                1.0,
                Arc::new(|_: &jumppad_textarea::KeyPress| None),
            ),
        ))
    });
    let mut app = test_app(0);
    app.tabs = vec![
        Tab::restored(0, None, "alpha beta", false, &factory),
        Tab::restored(1, None, "gamma delta", false, &factory),
    ];
    app.next_id = 2;

    let _ = app.update(Message::Editor(
        0,
        EditorMessage::Action(Action::Click(Point::ORIGIN)),
    ));
    let _ = app.update(Message::Editor(
        0,
        EditorMessage::Action(Action::Drag(Point::new(500.0, 4.0))),
    ));
    let selection_0 = app.tabs[0].editor.selection();
    assert!(selection_0.is_some());
    let cursor_0 = app.tabs[0].editor.cursor_position();

    let _ = app.update(Message::SelectTab(1));
    let _ = app.update(Message::Editor(
        1,
        EditorMessage::Action(Action::Click(Point::ORIGIN)),
    ));
    let _ = app.update(Message::Editor(
        1,
        EditorMessage::Action(Action::Drag(Point::new(500.0, 4.0))),
    ));
    assert!(app.tabs[1].editor.selection().is_some());

    let _ = app.update(Message::SelectTab(0));
    assert_eq!(app.tabs[0].editor.selection(), selection_0);
    assert_eq!(app.tabs[0].editor.cursor_position(), cursor_0);
}

/// An app whose tabs hold real `TextArea`s seeded with `texts`,
/// for find flows that need actual text and cursor behavior.
fn app_with_text(texts: &[&str]) -> JumpPadApp {
    let factory: EditorFactory = Box::new(|text, _extension| {
        let registry = syntax_registry::SyntaxRegistry::new(
            Vec::new(),
            syntax_registry::GrammarLookup::default(),
            || {},
        );
        Box::new(jumppad_textarea::TextArea::new(
            text,
            &registry,
            None,
            jumppad_textarea::SharedEditorConfig::new(
                1.0,
                Arc::new(|_: &jumppad_textarea::KeyPress| None),
            ),
        ))
    });
    let mut app = test_app(0);
    app.tabs = texts
        .iter()
        .enumerate()
        .map(|(index, body)| {
            Tab::restored(index as u64, None, body, false, &factory)
        })
        .collect();
    app.next_id = texts.len() as u64;
    app.editor_factory = factory;
    app
}

#[test]
fn opening_find_searches_and_selects_the_first_match() {
    let mut app = app_with_text(&["one two\nthree two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));

    let state = app.active_find().expect("palette open");
    assert!(state.open);
    assert_eq!(state.matches.len(), 2);
    assert_eq!(state.counter().as_deref(), Some("1 of 2"));
    // The first match is selected in the document, not just counted.
    assert_eq!(app.tabs[0].editor.cursor_position(), (0, 7));
}

#[test]
fn find_next_and_previous_step_and_wrap() {
    let mut app = app_with_text(&["one two\nthree two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));

    let _ = app.update(Message::FindNext);
    assert_eq!(
        app.active_find().unwrap().counter().as_deref(),
        Some("2 of 2")
    );
    assert_eq!(app.tabs[0].editor.cursor_position(), (1, 9));

    let _ = app.update(Message::FindNext);
    assert_eq!(
        app.active_find().unwrap().counter().as_deref(),
        Some("1 of 2"),
        "next past the last wraps"
    );

    let _ = app.update(Message::FindPrevious);
    assert_eq!(
        app.active_find().unwrap().counter().as_deref(),
        Some("2 of 2"),
        "previous before the first wraps"
    );
}

#[test]
fn escape_closes_the_palette_but_keeps_the_query() {
    let mut app = app_with_text(&["one two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));
    assert!(app.find_is_open());

    // Escape arrives as `CloseFind` from the dedicated event listener -
    // `keyboard::listen()` never sees it, since a focused `text_input`
    // captures Escape.
    let _ = app.update(Message::CloseFind);
    assert!(!app.find_is_open(), "escape closes the palette");
    assert_eq!(
        app.active_find().map(|state| state.query.as_str()),
        Some("two"),
        "the query survives for next time"
    );

    // Reopening brings the same query straight back.
    let _ = app.update(Message::OpenFind);
    assert!(app.find_is_open());
    assert_eq!(app.active_find().unwrap().matches.len(), 1);
}

#[test]
fn escape_leaves_the_find_palette_alone_while_the_modal_is_up() {
    // Escape now reaches the app from a listener that fires regardless
    // of which widget has focus, so the modal has to keep first claim on
    // it - cancelling the prompt, not quietly closing find behind it.
    let mut app = app_with_text(&["one two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));
    app.tabs[0].dirty = true;
    let _ = app.request_close(0);
    assert!(app.modal.is_some());

    let _ = app.update(Message::CloseFind);
    assert!(app.find_is_open(), "the modal owns Escape while it is up");
    assert!(app.modal.is_some());
}

#[test]
fn each_tab_keeps_its_own_find_query() {
    let mut app = app_with_text(&["alpha beta", "gamma delta gamma"]);

    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("beta".into()));

    let _ = app.update(Message::SelectTab(1));
    // Tab 1 has no palette of its own yet.
    assert!(!app.find_is_open());
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("gamma".into()));
    assert_eq!(
        app.active_find().unwrap().counter().as_deref(),
        Some("1 of 2")
    );

    // Back to tab 0: its own query and its own match count.
    let _ = app.update(Message::SelectTab(0));
    let state = app.active_find().expect("tab 0 palette");
    assert_eq!(state.query, "beta");
    assert_eq!(state.counter().as_deref(), Some("1 of 1"));

    let _ = app.update(Message::SelectTab(1));
    assert_eq!(app.active_find().unwrap().query, "gamma");
}

#[test]
fn closing_a_tab_forgets_its_find_state() {
    let mut app = app_with_text(&["one two", "other"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));
    let closed_id = app.tabs[0].id;
    assert!(app.find.contains_key(&closed_id));

    let _ = app.close_tab(0);
    assert!(!app.find.contains_key(&closed_id));
}

#[test]
fn editing_the_document_refreshes_the_match_list() {
    use iced::widget::text_editor;

    let mut app = app_with_text(&["two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));
    assert_eq!(app.active_find().unwrap().matches.len(), 1);

    // Typing a second occurrence into the document updates the count.
    app.tabs[0].editor.move_cursor_to(0, 3);
    for character in " two".chars() {
        let _ = app.update(Message::Editor(
            0,
            EditorMessage::Action(text_editor::Action::Edit(
                text_editor::Edit::Insert(character),
            )),
        ));
    }
    assert_eq!(app.active_find().unwrap().matches.len(), 2);
}

#[test]
fn find_on_a_missing_match_reports_no_results() {
    let mut app = app_with_text(&["one two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("zebra".into()));
    let state = app.active_find().unwrap();
    assert!(state.matches.is_empty());
    assert_eq!(state.counter().as_deref(), Some("No results"));

    // Stepping with nothing to step through must not panic.
    let _ = app.update(Message::FindNext);
    assert_eq!(app.active_find().unwrap().current, None);
}

#[test]
fn find_matches_case_insensitively_through_the_app() {
    let mut app = app_with_text(&["Two two TWO"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));
    assert_eq!(
        app.active_find().unwrap().counter().as_deref(),
        Some("1 of 3")
    );
}

#[test]
fn command_g_finds_again_with_the_palette_closed() {
    let mut app = app_with_text(&["two one two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));
    let _ = app.update(Message::CloseFind);
    assert!(!app.find_is_open());
    assert_eq!(
        app.tabs[0].editor.cursor_position(),
        (0, 3),
        "on the first match"
    );

    // Cmd+G steps without reopening the palette.
    let _ = app.update(Message::FindNext);
    assert!(
        !app.find_is_open(),
        "find-again must not reopen the palette"
    );
    assert_eq!(
        app.tabs[0].editor.cursor_position(),
        (0, 11),
        "second match"
    );

    // And wraps back around.
    let _ = app.update(Message::FindNext);
    assert_eq!(app.tabs[0].editor.cursor_position(), (0, 3));
}

#[test]
fn find_again_picks_up_document_edits_made_while_closed() {
    use iced::widget::text_editor;

    let mut app = app_with_text(&["two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));
    let _ = app.update(Message::CloseFind);
    assert_eq!(app.active_find().unwrap().matches.len(), 1);

    // Append a second occurrence with the palette shut - the stale match
    // list must be re-searched before stepping.
    app.tabs[0].editor.move_cursor_to(0, 3);
    for character in " two".chars() {
        let _ = app.update(Message::Editor(
            0,
            EditorMessage::Action(text_editor::Action::Edit(
                text_editor::Edit::Insert(character),
            )),
        ));
    }
    // Step from the top so the target is unambiguous - starting at the
    // cursor's post-edit spot (the end of the last match) would wrap.
    app.tabs[0].editor.move_cursor_to(0, 0);
    let _ = app.update(Message::FindNext);
    assert_eq!(
        app.active_find().unwrap().matches.len(),
        2,
        "the stale match list was re-searched"
    );
    assert_eq!(
        app.tabs[0].editor.cursor_position(),
        (0, 7),
        "the new match"
    );
}

#[test]
fn find_again_without_a_query_does_nothing() {
    let mut app = app_with_text(&["one two"]);
    // No palette ever opened for this tab.
    let _ = app.update(Message::FindNext);
    assert_eq!(app.tabs[0].editor.cursor_position(), (0, 0));
    assert!(app.active_find().is_none());
}

#[test]
fn find_again_steps_while_the_palette_is_open() {
    let mut app = app_with_text(&["two one two three two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));
    assert_eq!(
        app.active_find().unwrap().counter().as_deref(),
        Some("1 of 3")
    );

    // Cmd+G with the palette still up steps without closing it.
    let _ = app.update(Message::FindNext);
    assert!(app.find_is_open());
    assert_eq!(
        app.active_find().unwrap().counter().as_deref(),
        Some("2 of 3")
    );

    // Cmd+Shift+G goes back.
    let _ = app.update(Message::FindPrevious);
    assert_eq!(
        app.active_find().unwrap().counter().as_deref(),
        Some("1 of 3")
    );
    let _ = app.update(Message::FindPrevious);
    assert_eq!(
        app.active_find().unwrap().counter().as_deref(),
        Some("3 of 3"),
        "previous past the first wraps"
    );
}

#[test]
fn a_shortcut_character_leaked_while_command_is_held_is_not_typed_into_the_query()
 {
    // macOS only in practice: Cmd doesn't suppress character production
    // and `text_input` inserts it, so Cmd+G would append "g" to the
    // query on its way to being a shortcut.
    let mut app = app_with_text(&["one two"]);
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));

    let _ = app.update(Message::ModifiersChanged(Modifiers::COMMAND));
    let _ = app.update(Message::FindQueryChanged("twog".into()));
    assert_eq!(
        app.active_find().unwrap().query,
        "two",
        "the leaked shortcut character must not reach the query"
    );

    // A multi-character change with command held is a paste, not a leak.
    let _ = app.update(Message::FindQueryChanged("twobeta".into()));
    assert_eq!(
        app.active_find().unwrap().query,
        "twobeta",
        "paste still lands"
    );

    // And ordinary typing is untouched once command is released.
    let _ = app.update(Message::ModifiersChanged(Modifiers::empty()));
    let _ = app.update(Message::FindQueryChanged("twobetax".into()));
    assert_eq!(app.active_find().unwrap().query, "twobetax");
}

#[test]
fn command_shift_g_resolves_to_find_previous() {
    let overrides = HashMap::new();
    let resolved = handle_hotkey(
        Key::Character("g".into()),
        Modifiers::CTRL | Modifiers::SHIFT,
        key::Physical::Code(key::Code::KeyG),
        &overrides,
    );
    assert!(matches!(resolved, Some(Message::FindPrevious)));
}

#[test]
fn command_g_resolves_to_find_next() {
    let overrides = HashMap::new();
    let resolved = handle_hotkey(
        Key::Character("g".into()),
        Modifiers::CTRL,
        key::Physical::Code(key::Code::KeyG),
        &overrides,
    );
    assert!(matches!(resolved, Some(Message::FindNext)));
}

#[test]
fn command_f_resolves_to_open_find() {
    // `Modifiers::CTRL` stands in for `command()`, which resolves per-OS
    // at compile time - the same convention the undo/redo tests use.
    let overrides = HashMap::new();
    let resolved = handle_hotkey(
        Key::Character("f".into()),
        Modifiers::CTRL,
        key::Physical::Code(key::Code::KeyF),
        &overrides,
    );
    assert!(matches!(resolved, Some(Message::OpenFind)));
}

#[test]
fn shift_click_reaches_the_editor_as_a_selection_extending_drag() {
    use iced::widget::text_editor;

    let mut app = test_app(1);
    let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    app.tabs[0].editor = Box::new(RecordingEditor(log.clone()));
    let click = EditorMessage::Action(text_editor::Action::Click(
        iced::Point::ORIGIN,
    ));

    let _ = app.update(Message::ModifiersChanged(Modifiers::SHIFT));
    let _ = app.update(Message::Editor(0, click.clone()));
    // Releasing shift restores the plain click.
    let _ = app.update(Message::ModifiersChanged(Modifiers::default()));
    let _ = app.update(Message::Editor(0, click));

    assert!(matches!(
        log.borrow().as_slice(),
        [
            EditorMessage::Action(text_editor::Action::Drag(_)),
            EditorMessage::Action(text_editor::Action::Click(_)),
        ]
    ));
}

#[test]
fn switching_tabs_saves_and_restores_the_selection() {
    let mut app = test_app(2);
    let restored = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let saved = SavedSelection {
        anchor: (0, 2),
        kind: SelectionKind::Range,
    };
    app.tabs[0].editor = Box::new(SelectionSpyEditor {
        selection: Some(saved),
        cursor: (1, 4),
        restored: restored.clone(),
    });

    let _ = app.switch_active(1);
    assert_eq!(app.tabs[0].last_selection, Some(saved));
    assert_eq!(app.tabs[0].last_cursor, (1, 4));

    let _ = app.switch_active(0);
    assert_eq!(
        restored.borrow().as_slice(),
        [Restore::Selection {
            selection: saved,
            cursor: (1, 4)
        }]
    );
}

#[test]
fn switching_to_a_tab_without_a_selection_restores_only_the_cursor() {
    let mut app = test_app(2);
    let restored = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    app.tabs[0].editor = Box::new(SelectionSpyEditor {
        selection: None,
        cursor: (3, 1),
        restored: restored.clone(),
    });

    let _ = app.switch_active(1);
    assert_eq!(app.tabs[0].last_selection, None);

    let _ = app.switch_active(0);
    assert_eq!(restored.borrow().as_slice(), [Restore::Cursor(3, 1)]);
}

#[test]
fn switch_active_records_previous_tab_id() {
    let mut app = test_app(3);
    let _ = app.switch_active(1);
    assert_eq!(app.previous_active_id, Some(0));
    let _ = app.switch_active(2);
    assert_eq!(app.previous_active_id, Some(1));
}

#[test]
fn select_previous_active_tab_toggles_back_and_forth() {
    let mut app = test_app(3);
    let _ = app.switch_active(2);
    assert_eq!(app.active, 2);
    let _ = app.update(Message::SelectPreviousActiveTab);
    assert_eq!(app.active, 0);
    let _ = app.update(Message::SelectPreviousActiveTab);
    assert_eq!(app.active, 2);
}

#[test]
fn select_previous_active_tab_is_noop_for_a_stale_id() {
    let mut app = test_app(2);
    app.previous_active_id = Some(999); // no tab has this id - e.g. since closed
    let _ = app.update(Message::SelectPreviousActiveTab);
    assert_eq!(app.active, 0);
}

#[test]
fn cycle_tab_wraps_around_in_both_directions() {
    let mut app = test_app(3);
    let _ = app.update(Message::SelectPreviousTab);
    assert_eq!(app.active, 2);
    let _ = app.update(Message::SelectNextTab);
    assert_eq!(app.active, 0);
    let _ = app.update(Message::SelectNextTab);
    assert_eq!(app.active, 1);
}

#[test]
fn cycle_tab_on_single_tab_is_noop() {
    let mut app = test_app(1);
    let _ = app.update(Message::SelectNextTab);
    assert_eq!(app.active, 0);
}

fn no_overrides() -> KeyOverrides {
    HashMap::new()
}

#[test]
fn ctrl_tab_is_recognized_as_select_previous_active_tab() {
    let tab_key = Key::Named(Named::Tab);
    assert!(matches!(
        handle_hotkey(
            tab_key,
            Modifiers::CTRL,
            key::Physical::Code(key::Code::Tab),
            &no_overrides()
        ),
        Some(Message::SelectPreviousActiveTab)
    ));
}

#[test]
fn ctrl_tab_with_extra_modifiers_does_not_match() {
    let tab_key = Key::Named(Named::Tab);
    let physical = key::Physical::Code(key::Code::Tab);
    assert!(
        handle_hotkey(
            tab_key.clone(),
            Modifiers::CTRL | Modifiers::SHIFT,
            physical,
            &no_overrides()
        )
        .is_none()
    );
    assert!(
        handle_hotkey(
            tab_key,
            Modifiers::CTRL | Modifiers::ALT,
            physical,
            &no_overrides()
        )
        .is_none()
    );
}

#[test]
fn hardcoded_default_still_fires_with_empty_overrides() {
    assert!(matches!(
        handle_hotkey(
            Key::Character("n".into()),
            Modifiers::CTRL,
            key::Physical::Code(key::Code::KeyN),
            &no_overrides()
        ),
        Some(Message::NewTab)
    ));
}

#[test]
fn every_action_is_wired_to_exactly_one_layer() {
    // The composition root is the only place that can see both mappers,
    // so this is the only place the question can be asked. An action
    // added to the registry and wired to nothing fails here rather than
    // going quiet at runtime.
    for action in jumppad_actions::Action::ALL {
        let shell = message_for(*action).is_some();
        let editor = jumppad_textarea::binding_for(*action).is_some();
        assert!(
            shell != editor,
            "{action} is handled by {}",
            match (shell, editor) {
                (true, true) => "both layers",
                _ => "neither layer",
            }
        );
    }
}

#[test]
fn a_category_matches_the_layer_that_performs_it() {
    use jumppad_actions::Category;

    for action in jumppad_actions::Action::ALL {
        let expected = match action.category() {
            Category::App => message_for(*action).is_some(),
            Category::Editor => {
                jumppad_textarea::binding_for(*action).is_some()
            }
        };
        assert!(expected, "{action} is filed under the wrong category");
    }
}

#[test]
fn override_wins_over_a_conflicting_hardcoded_default() {
    // Ctrl+N normally binds NewTab (tier 2) - override it to OpenFile.
    let mut overrides = HashMap::new();
    overrides.insert((Modifiers::CTRL, key::Code::KeyN), Action::OpenFile);
    assert!(matches!(
        handle_hotkey(
            Key::Character("n".into()),
            Modifiers::CTRL,
            key::Physical::Code(key::Code::KeyN),
            &overrides
        ),
        Some(Message::OpenFile)
    ));
}

#[test]
fn build_key_overrides_ignores_unrecognized_command_name() {
    let mut keybinds = jumppad_config::KeybindsConfig::default();
    keybinds.overrides.insert(
        "frobnicate".to_string(),
        global_hotkey::hotkey::HotKey::new(
            Some(global_hotkey::hotkey::Modifiers::CONTROL),
            global_hotkey::hotkey::Code::KeyN,
        ),
    );
    assert!(build_key_overrides(&keybinds).is_empty());
}

#[test]
fn build_key_overrides_resolves_the_line_command_names() {
    // The one check that the new snake_case names survive the
    // `global_hotkey` -> iced conversion, arrow codes included.
    let keybinds: jumppad_config::KeybindsConfig = toml::from_str(
        r#"
        toggle = "control+Backquote"

        [overrides]
        delete_line = "control+alt+k"
        move_line_up = "control+alt+ArrowUp"
        copy_line_down = "control+shift+alt+ArrowDown"
        "#,
    )
    .unwrap();
    let overrides = build_key_overrides(&keybinds);
    assert_eq!(
        overrides.get(&(Modifiers::CTRL | Modifiers::ALT, key::Code::KeyK)),
        Some(&Action::DeleteLine)
    );
    assert_eq!(
        overrides
            .get(&(Modifiers::CTRL | Modifiers::ALT, key::Code::ArrowUp)),
        Some(&Action::MoveLineUp)
    );
    assert_eq!(
        overrides.get(&(
            Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT,
            key::Code::ArrowDown
        )),
        Some(&Action::CopyLineDown)
    );
}

#[test]
fn apply_keybinds_swaps_both_override_tables_live() {
    let mut app = test_app(1);
    let keybinds: jumppad_config::KeybindsConfig = toml::from_str(
        r#"
        toggle = "control+Backquote"

        [overrides]
        new_tab = "control+alt+n"
        undo = "control+alt+z"
        "#,
    )
    .unwrap();
    app.apply_keybinds(keybinds);

    // App level: the very next key press resolves through the new table.
    assert!(matches!(
        handle_hotkey(
            Key::Character("n".into()),
            Modifiers::CTRL | Modifiers::ALT,
            key::Physical::Code(key::Code::KeyN),
            &app.keybind_overrides
        ),
        Some(Message::NewTab)
    ));
    // Editor level: the resolver every open tab reads was replaced.
    let resolved = |app: &JumpPadApp, modifiers, code, key| {
        (app.editor_config.resolver())(&jumppad_textarea::KeyPress {
            key,
            modified_key: Key::Character("z".into()),
            physical_key: key::Physical::Code(code),
            modifiers,
            text: None,
            status: jumppad_textarea::text_editor::Status::Focused {
                is_hovered: false,
            },
        })
    };
    assert_eq!(
        resolved(
            &app,
            Modifiers::CTRL | Modifiers::ALT,
            key::Code::KeyZ,
            Key::Character("z".into())
        ),
        Some(Action::Undo)
    );

    // A revert to defaults removes the binds just as live.
    app.apply_keybinds(jumppad_config::KeybindsConfig::default());
    assert!(app.keybind_overrides.is_empty());
    assert_eq!(
        resolved(
            &app,
            Modifiers::CTRL | Modifiers::ALT,
            key::Code::KeyZ,
            Key::Character("z".into())
        ),
        None
    );
}

/// A config whose slots both name `theme`, so the same definition can be
/// read into either one.
fn config_with_theme(theme: &str) -> jumppad_config::Config {
    toml::from_str(&format!(
        "[mode]\ntheme.light = \"mine\"\ntheme.dark = \"mine\"\n\n[themes.mine]\n{theme}"
    ))
    .unwrap()
}

#[test]
fn apply_config_swaps_the_palette() {
    let mut app = test_app(1);

    let _ = app.apply_config(config_with_theme(r#"palette = "Dracula""#));
    assert_eq!(app.theme.to_string(), "Dracula");
}

/// The palette can be named by the slot instead of by a theme, so
/// picking colors needs no `[themes]` entry at all.
#[test]
fn a_slot_naming_a_palette_paints_that_palette() {
    let mut app = test_app(1);
    let config: jumppad_config::Config = toml::from_str(
        "[mode]\ndetection = \"dark\"\ntheme.dark = \"Nord\"",
    )
    .unwrap();

    let _ = app.apply_config(config);
    assert_eq!(app.theme.to_string(), "Nord");
    assert_eq!(app.editor_config.font_size(), 16.0, "default fonts stand");
}

#[test]
fn apply_config_background_alpha_reaches_the_window_and_the_editors() {
    let mut app = test_app(1);

    let _ = app.apply_config(config_with_theme("background.alpha = 0.5"));
    assert_eq!(app.background_alpha, 0.5);
    assert_eq!(app.editor_config.background_alpha(), 0.5);
}

/// The case that used to print a restart line and then never recover:
/// a session booted opaque, reloaded to a translucent theme. The reload
/// calls for a new window - `wants_transparency` moved - and the alpha
/// belongs to the window that reload is putting on screen, so it
/// applies now rather than waiting for a restart that was the only
/// thing that ever fixed it.
#[test]
fn a_session_that_booted_opaque_still_honors_a_reloaded_alpha() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());
    let translucent = config_with_theme("background.alpha = 0.5");

    assert!(
        window::needs_replacing(&app.config, &translucent),
        "the reload is what puts a transparent window on screen"
    );

    let _ = app.apply_config(translucent);
    assert_eq!(app.background_alpha, 0.5);
    assert_eq!(app.editor_config.background_alpha(), 0.5);

    // And it stays applied once that window arrives, rather than being
    // undone by the arm every window comes through.
    let _ =
        app.update(Message::WindowReady(Some(iced::window::Id::unique())));
    assert_eq!(app.background_alpha, 0.5);
}

/// Unlike the alpha above, a blur needs no window born anything in
/// particular: it is asked of the compositor once the window exists, so
/// it is a live setting in every session.
#[test]
fn apply_config_records_a_themes_blur_whatever_the_window_is() {
    let mut app = test_app(1);

    let _ = app.apply_config(config_with_theme("background.blur = 24"));
    assert_eq!(app.background_blur, Blur::Radius(24));

    let _ = app.apply_config(config_with_theme("background.blur = 0"));
    assert_eq!(app.background_blur, Blur::None);
}

/// The named forms reach the app as the two things a platform can be
/// told, so nothing downstream has to know which spelling was used.
#[test]
fn the_acrylic_names_arrive_as_a_blur_the_platforms_can_read() {
    let mut app = test_app(1);

    let _ = app.apply_config(config_with_theme(
        r#"background.blur = "acrylic10""#,
    ));
    assert_eq!(app.background_blur, Blur::Acrylic10);

    let _ = app.apply_config(config_with_theme(
        r#"background.blur = "acrylic11""#,
    ));
    assert_eq!(app.background_blur, Blur::Acrylic11);
}

/// Same blur either side of it, so only the acrylic Windows is asked
/// for moved - and that still has to reach the window.
#[test]
fn swapping_which_acrylic_still_reaches_the_window() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());

    let _ = app.apply_config(config_with_theme(
        "background.alpha = 0.5\nbackground.blur = \"acrylic10\"",
    ));
    assert_eq!(app.background_blur, Blur::Acrylic10);

    let swapped = app.apply_config(config_with_theme(
        "background.alpha = 0.5\nbackground.blur = \"acrylic11\"",
    ));
    assert_eq!(app.background_blur, Blur::Acrylic11);
    if cfg!(target_os = "windows") {
        assert_ne!(swapped.units(), 0, "the window was never told");
    }
}

/// A radius the platform will not honor whole is still recorded whole -
/// the capping belongs to the platform that has a ceiling, not here.
#[test]
fn an_unreasonable_radius_is_recorded_as_written() {
    let mut app = test_app(1);
    let _ = app.apply_config(config_with_theme("background.blur = 4000"));
    assert_eq!(app.background_blur, Blur::Radius(4000));
}

/// The blur rides with the theme, so an OS light/dark switch carries it
/// the same way it carries the palette.
#[test]
fn an_appearance_switch_carries_the_blur_with_it() {
    let mut app = test_app(1);

    let _ = app.apply_config(
        toml::from_str(
            r#"
            [themes.base]
            background.alpha = 0.8

            [themes.dark]
            background.blur = 24
            "#,
        )
        .unwrap(),
    );

    let _ = app.apply_os_appearance(Some(Appearance::Dark));
    assert_eq!(app.background_blur, Blur::Radius(24));

    let _ = app.apply_os_appearance(Some(Appearance::Light));
    assert_eq!(app.background_blur, Blur::None);
}

/// A solid window has no desktop showing through to frost, and on
/// Windows the call would also override a backdrop nobody can see past
/// the paint anyway - so it is never made.
#[test]
fn a_solid_window_is_told_nothing_about_blur() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());
    app.background_blur = Blur::Radius(24);

    assert_eq!(app.background_alpha, 1.0);
    assert_eq!(app.apply_window_blur().units(), 0);
}

#[test]
fn a_translucent_window_is_told_which_way_its_blur_went() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());
    app.background_alpha = 0.5;

    if cfg!(any(target_os = "windows", target_os = "macos")) {
        assert_ne!(app.apply_window_blur().units(), 0);
        app.background_blur = Blur::Radius(24);
        assert_ne!(app.apply_window_blur().units(), 0);
    }
}

/// A reload that only moves the radius still reaches the window - the
/// arm fires on any change, not just on crossing zero.
#[test]
fn changing_only_the_radius_still_reaches_the_window() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());

    let _ = app.apply_config(config_with_theme(
        "background.alpha = 0.5\nbackground.blur = 12",
    ));
    assert_eq!(app.background_blur, Blur::Radius(12));

    let widened = app.apply_config(config_with_theme(
        "background.alpha = 0.5\nbackground.blur = 40",
    ));
    assert_eq!(app.background_blur, Blur::Radius(40));
    if cfg!(target_os = "windows") {
        assert_ne!(widened.units(), 0, "the window was never told");
    }
}

/// No family in either section, so the assertions stay off the machine's
/// installed fonts - `resolve_font` only consults them for a family that
/// names something, and a test box need not have the one it named.
#[test]
fn apply_config_reaches_the_shared_text_size() {
    let mut app = test_app(1);
    let _ = app.apply_config(config_with_theme("editor.font.size = 22.0"));
    assert_eq!(app.editor_config.font_size(), 22.0);
    assert_eq!(app.editor_config.font(), Font::MONOSPACE);
}

#[test]
fn an_unreadable_configured_text_size_is_clamped_not_obeyed() {
    let mut app = test_app(1);
    let _ = app.apply_config(config_with_theme("editor.font.size = 0.0"));
    assert!(app.editor_config.font_size() >= 4.0);
}

#[test]
fn a_blank_family_takes_the_fallback_it_was_handed() {
    assert_eq!(resolve_font(None, Font::MONOSPACE), Font::MONOSPACE);
    assert_eq!(resolve_font(Some("   "), Font::MONOSPACE), Font::MONOSPACE);
    assert_eq!(resolve_font(None, Font::DEFAULT), Font::DEFAULT);
}

#[test]
fn apply_config_reaches_the_chrome_and_leaves_the_editor_alone() {
    let mut app = test_app(1);
    let _ = app.apply_config(config_with_theme("ui.font.size = 20.0"));
    assert_eq!(app.ui_text, UiText::new(Font::DEFAULT, 20.0));
    // The editor reads its own section, and that one didn't move.
    assert_eq!(
        app.editor_config.font_size(),
        jumppad_config::DEFAULT_FONT_SIZE
    );
}

/// Light and dark differing in every property a theme carries, so one
/// switch has to move all of them together.
fn config_with_both_slots() -> jumppad_config::Config {
    toml::from_str(
        r#"
        [mode]
        detection = "auto"
        theme.light = "day"
        theme.dark = "night"

        [themes.day]
        palette = "Solarized Light"
        editor.font.size = 15.0
        ui.font.size = 15.0

        [themes.night]
        palette = "Nord"
        editor.font.size = 21.0
        ui.font.size = 21.0
        "#,
    )
    .unwrap()
}

fn report(app: &mut JumpPadApp, reported: iced::theme::Mode) {
    let _ = app.update(Message::SystemAppearanceReported(reported));
}

/// The guard on there being one apply path: palette, editor font and
/// chrome font all move on the same message.
#[test]
fn an_os_switch_to_dark_swaps_the_whole_theme() {
    let mut app = test_app(1);
    let _ = app.apply_config(config_with_both_slots());
    report(&mut app, iced::theme::Mode::Light);
    assert_eq!(app.theme.to_string(), "Solarized Light");
    assert_eq!(app.editor_config.font_size(), 15.0);
    assert_eq!(app.ui_text, UiText::new(Font::DEFAULT, 15.0));

    report(&mut app, iced::theme::Mode::Dark);
    assert_eq!(app.theme.to_string(), "Nord");
    assert_eq!(app.editor_config.font_size(), 21.0);
    assert_eq!(app.ui_text, UiText::new(Font::DEFAULT, 21.0));
}

#[test]
fn an_os_switch_moves_nothing_while_the_mode_is_pinned() {
    let mut app = test_app(1);
    let mut config = config_with_both_slots();
    config.mode.detection = jumppad_config::Detection::Light;
    let _ = app.apply_config(config);

    report(&mut app, iced::theme::Mode::Dark);
    assert_eq!(app.theme.to_string(), "Solarized Light");
    // Recorded anyway - the test below is what that buys.
    assert_eq!(app.os_appearance, Some(Appearance::Dark));
}

/// Why the OS's answer is kept while pinned: turning `auto` back on
/// resolves against it instead of waiting for the next OS switch.
#[test]
fn turning_detection_back_to_automatic_uses_the_last_os_report() {
    let mut app = test_app(1);
    let mut pinned = config_with_both_slots();
    pinned.mode.detection = jumppad_config::Detection::Light;
    let _ = app.apply_config(pinned);
    report(&mut app, iced::theme::Mode::Dark);
    assert_eq!(app.theme.to_string(), "Solarized Light");

    let _ = app.apply_config(config_with_both_slots());
    assert_eq!(app.theme.to_string(), "Nord");
}

/// A reported mode of `None` is no preference, not a third theme.
#[test]
fn an_os_with_no_preference_leaves_the_light_slot_showing() {
    let mut app = test_app(1);
    let _ = app.apply_config(config_with_both_slots());
    report(&mut app, iced::theme::Mode::None);
    assert_eq!(app.theme.to_string(), "Solarized Light");
    assert_eq!(app.os_appearance, None);
}

#[test]
fn an_unknown_palette_falls_back_to_the_one_it_was_handed() {
    assert_eq!(resolve_palette("nonsense", Theme::Dark), Theme::Dark);
    assert_eq!(resolve_palette("nonsense", Theme::Light), Theme::Light);
}

/// The two crates have to agree on how the default palettes are spelled,
/// and only this one can check the spelling. Asserted through
/// `resolve_palette` rather than against the display names, since the
/// slots spell them the way `[themes.light]` and `[themes.dark]` do.
#[test]
fn both_default_palette_names_resolve_to_their_palettes() {
    assert_eq!(
        resolve_palette(
            Appearance::Light.default_palette(),
            Theme::Dracula
        ),
        Theme::Light
    );
    assert_eq!(
        resolve_palette(Appearance::Dark.default_palette(), Theme::Dracula),
        Theme::Dark
    );
}

/// The example the base theme exists for, through the app: no `[mode]`
/// section, one palette named once, and the two slots differing only in
/// the alpha they ask for.
#[test]
fn a_config_with_no_mode_section_shows_the_themes_named_after_the_slots() {
    let mut app = test_app(1);
    let config: jumppad_config::Config = toml::from_str(
        r#"
        [themes.base]
        palette = "Ferra"

        [themes.dark]
        background.alpha = 0.95

        [themes.light]
        background.alpha = 1.0
        "#,
    )
    .unwrap();

    let _ = app.apply_config(config);
    assert_eq!(app.theme.to_string(), "Ferra");
    assert_eq!(app.background_alpha, 1.0);

    report(&mut app, iced::theme::Mode::Dark);
    assert_eq!(app.theme.to_string(), "Ferra", "one palette, both slots");
    assert_eq!(app.background_alpha, 0.95);
}

#[test]
fn apply_config_takes_the_fonts_the_base_theme_names() {
    let mut app = test_app(1);
    let config: jumppad_config::Config = toml::from_str(
        r#"
        [themes.base]
        editor.font.size = 22.0

        [themes.light]
        ui.font.size = 20.0
        "#,
    )
    .unwrap();

    let _ = app.apply_config(config);
    assert_eq!(app.editor_config.font_size(), 22.0);
    assert_eq!(app.ui_text, UiText::new(Font::DEFAULT, 20.0));
}

/// `apply_config` diffs the whole `[themes]` table, so the base theme
/// needs no arm of its own to reach the screen.
#[test]
fn editing_only_the_base_theme_reapplies_the_showing_theme() {
    let mut app = test_app(1);
    let before: jumppad_config::Config = toml::from_str(
        "[themes.base]\npalette = \"Nord\"\n\n[themes.light]",
    )
    .unwrap();
    let after: jumppad_config::Config = toml::from_str(
        "[themes.base]\npalette = \"Dracula\"\n\n[themes.light]",
    )
    .unwrap();

    let _ = app.apply_config(before);
    assert_eq!(app.theme.to_string(), "Nord");

    let _ = app.apply_config(after);
    assert_eq!(app.theme.to_string(), "Dracula");
}

/// What the plan for replacing windows rests on: the caret lives in the
/// document, not in the widget tree, so the tree a new window builds
/// from scratch doesn't take it with it.
#[test]
fn the_caret_and_selection_belong_to_the_document_not_the_window() {
    use iced::widget::text_editor::{Action, Motion};

    let mut app = app_with_text(&["one two\nthree four"]);
    let _ = app.update(Message::Editor(
        0,
        EditorMessage::Action(Action::Move(Motion::Down)),
    ));
    let moved = app.tabs[0].editor.cursor_position();
    assert_ne!(moved, (0, 0), "the caret moved");

    // A window replacement rebuilds every widget from a fresh cache.
    // `view` is what a rebuild runs, and the caret has to outlive it.
    let _ = app.view();
    let _ = app.view();

    assert_eq!(app.tabs[0].editor.cursor_position(), moved);
}

#[test]
fn a_new_window_gives_its_focus_to_the_editor() {
    let app = test_app(1);
    assert_eq!(app.focus_target(), Some(editor_core::EDITOR_WIDGET_ID));
    assert_ne!(app.restore_focus().units(), 0, "and issues the operation");
}

/// The palette outranks the editor while it is on screen: that is where
/// the user was typing.
#[test]
fn a_new_window_under_an_open_palette_focuses_the_query() {
    let mut app = test_app(1);
    let tab = app.tabs[0].id;
    app.find.insert(
        tab,
        crate::find::FindState {
            query: "two".to_string(),
            open: true,
            ..Default::default()
        },
    );
    assert_eq!(app.focus_target(), Some(FIND_INPUT_ID));

    // A closed palette isn't on screen, so it isn't a focus target - its
    // query is kept only so reopening it starts where you left off.
    app.find.get_mut(&tab).unwrap().open = false;
    assert_eq!(app.focus_target(), Some(editor_core::EDITOR_WIDGET_ID));
}

#[test]
fn a_modal_leaves_focus_where_the_app_is_handling_it() {
    let mut app = test_app(1);
    app.modal = Some(Modal::Close(PendingClose {
        tab_id: app.tabs[0].id,
        title: "one".to_string(),
        focused: 0,
    }));

    assert_eq!(app.focus_target(), None);
    assert_eq!(app.restore_focus().units(), 0);
}

/// The case that reached a user: a window born transparent because
/// *some* theme wanted it, showing a solid theme, then reloaded to a
/// translucent one. No new window is called for - the window is already
/// transparent - so the platform setup a translucent window needs has to
/// be re-armed here or it never runs at all.
#[test]
fn a_window_that_only_becomes_translucent_later_is_still_set_up_for_it() {
    fn showing(alpha: &str) -> jumppad_config::Config {
        toml::from_str(&format!(
            r#"
            [mode]
            detection = "light"
            theme.light = "day"

            [themes.day]
            background.alpha = {alpha}

            [themes.night]
            background.alpha = 0.975
            "#
        ))
        .unwrap()
    }

    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());

    // Born transparent on account of the theme that isn't showing.
    let solid = showing("1.0");
    assert!(
        solid.wants_transparency(),
        "the unshown theme is what made the window transparent"
    );
    let _ = app.apply_config(solid.clone());
    assert_eq!(app.background_alpha, 1.0, "the showing theme is solid");

    app.surface_reset_frames = 0;
    app.shadow_refresh_frames = 0;

    let translucent = showing("0.5");
    assert!(
        !window::needs_replacing(&solid, &translucent),
        "the window is already transparent, so no new one is called for"
    );

    let _ = app.apply_config(translucent);
    assert_eq!(app.background_alpha, 0.5, "the alpha applied");
    if cfg!(target_os = "windows") {
        assert_ne!(
            app.surface_reset_frames, 0,
            "the redirection surface was never re-armed"
        );
    }
    if cfg!(target_os = "macos") {
        assert_ne!(app.shadow_refresh_frames, 0);
    }
}

/// A resize reallocates the redirection surface, and the replacement is
/// not zeroed - so the reset that runs once at `WindowReady` has to run
/// again on every resize, or the newly revealed edges keep whatever was
/// in that memory (see `windows.rs`).
#[test]
fn a_resize_re_arms_the_redirection_surface_reset() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());
    let _ = app.apply_config(config_with_theme("background.alpha = 0.5"));
    app.surface_reset_frames = 0;

    let _ = app.update(Message::WindowResized);

    if cfg!(target_os = "windows") {
        assert_ne!(
            app.surface_reset_frames, 0,
            "the surface was left holding whatever the resize gave it"
        );
    }
}

/// And only on the crossing: a theme swap between two translucent
/// themes has nothing new to tell the window.
#[test]
fn a_window_already_translucent_is_not_set_up_twice() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());

    let _ = app.apply_config(config_with_theme("background.alpha = 0.5"));
    app.surface_reset_frames = 0;
    app.shadow_refresh_frames = 0;

    let _ = app.apply_config(config_with_theme("background.alpha = 0.7"));
    assert_eq!(app.background_alpha, 0.7);
    assert_eq!(app.surface_reset_frames, 0);
    assert_eq!(app.shadow_refresh_frames, 0);
}

/// The sizes and line heights the chrome used when they were hardcoded,
/// so the default config still draws the frame it always drew.
#[test]
fn the_default_base_reproduces_the_sizes_the_chrome_was_built_with() {
    let ui = UiText::new(Font::DEFAULT, jumppad_config::DEFAULT_FONT_SIZE);
    assert_eq!(ui.base, 16.0);
    assert_eq!(ui.base * 0.75, 12.0);
    assert_eq!(ui.input_size(), 14.0);
    assert_eq!(Pixels((ui.base * 1.3).floor()), Pixels(20.0));
    assert_eq!(Pixels((ui.base * 0.75 * 1.3).ceil()), Pixels(16.0));
}

fn language(
    name: &str,
    syntax: Option<&str>,
    extensions: &[&str],
    comment: Option<jumppad_config::CommentSyntax>,
) -> jumppad_config::LanguageConfig {
    jumppad_config::LanguageConfig {
        name: name.to_string(),
        syntax: syntax.map(str::to_string),
        extensions: Some(
            extensions.iter().map(|ext| ext.to_string()).collect(),
        ),
        comment,
        ..Default::default()
    }
}

#[test]
fn apply_config_reaches_the_shared_comment_styles() {
    let mut app = test_app(1);
    let config = jumppad_config::Config {
        languages: vec![
            language(
                "Zig",
                Some("zig"),
                &["zig"],
                Some(jumppad_config::CommentSyntax::Single(
                    "// ".to_string(),
                )),
            ),
            language(
                "HTML",
                None,
                &["html"],
                Some(jumppad_config::CommentSyntax::Multi {
                    left: "<!--".to_string(),
                    right: "-->".to_string(),
                }),
            ),
        ],
        ..Default::default()
    };
    let _ = app.apply_config(config);
    assert_eq!(
        app.editor_config.comment_styles().get("zig"),
        Some(&jumppad_textarea::CommentStyle::Single("// ".to_string()))
    );
    assert_eq!(
        app.editor_config.comment_styles().get("html"),
        Some(&jumppad_textarea::CommentStyle::Multi {
            left: "<!--".to_string(),
            right: "-->".to_string(),
        })
    );
}

#[test]
fn a_new_app_starts_on_the_configured_indentation() {
    let app = test_app(1);
    assert_eq!(
        app.editor_config.indentation(),
        jumppad_textarea::Indentation::default(),
        "the shipped default is tabs at four"
    );
}

#[test]
fn apply_config_reaches_the_shared_indentation() {
    let mut app = test_app(1);
    let config = jumppad_config::Config {
        indentation: jumppad_config::IndentationConfig {
            style: jumppad_config::IndentationStyle::Spaces,
            width: 2,
        },
        ..Default::default()
    };

    let _ = app.apply_config(config);

    let indentation = app.editor_config.indentation();
    assert_eq!(
        indentation.style(),
        jumppad_textarea::IndentationStyle::Spaces
    );
    assert_eq!(indentation.width(), 2);
}

#[test]
fn an_out_of_range_indentation_width_is_pulled_into_range() {
    // The config crate holds no range of its own, so this is the only
    // thing standing between a typo and a buffer that ignores it.
    let mut app = test_app(1);
    let config = jumppad_config::Config {
        indentation: jumppad_config::IndentationConfig {
            style: jumppad_config::IndentationStyle::Tabs,
            width: 0,
        },
        ..Default::default()
    };

    let _ = app.apply_config(config);

    assert_eq!(app.editor_config.indentation().width(), 1);
}

#[test]
fn a_new_app_starts_on_the_default_word_separators() {
    // The default list is written out in two crates - `jumppad_config`
    // can't depend on the widget's copy and the widget can't depend on
    // the config's - so this is what keeps the two the same list.
    let app = test_app(1);
    assert_eq!(
        app.editor_config.word_separators(),
        jumppad_textarea::WordSeparators::new(
            jumppad_config::DEFAULT_WORD_SEPARATORS
        )
    );
    assert_eq!(
        jumppad_config::WordsConfig::default().separators,
        jumppad_config::DEFAULT_WORD_SEPARATORS
    );
}

#[test]
fn apply_config_reaches_the_shared_word_separators() {
    let mut app = test_app(1);
    let config = jumppad_config::Config {
        words: jumppad_config::WordsConfig {
            separators: ".,".to_string(),
        },
        ..Default::default()
    };

    let _ = app.apply_config(config);

    assert_eq!(
        app.editor_config.word_separators(),
        jumppad_textarea::WordSeparators::new(".,")
    );
}

#[test]
fn a_name_only_change_applies_nothing() {
    // A language has to be in effect before renaming it can prove anything -
    // the defaults name none, since the bundles under `syntaxes/` do.
    let mut app = test_app(1);
    let mut config = jumppad_config::Config::default();
    config.languages.push(language(
        "Zig",
        Some("zig"),
        &["zig"],
        Some(jumppad_config::CommentSyntax::Single("// ".to_string())),
    ));
    let _ = app.apply_config(config.clone());

    let before = app.editor_config.comment_styles();
    config.languages[0].name = "Renamed".to_string();

    let _ = app.apply_config(config);
    // Neither derived view moved, so the setter never ran.
    assert!(Arc::ptr_eq(&before, &app.editor_config.comment_styles()));
}

#[test]
fn restart_required_settings_mutate_no_live_state() {
    let mut app = test_app(1);
    let theme_before = app.theme.to_string();
    // A new grammar mapping without a comment style: the one setting
    // still waiting on a restart, now that the window ones don't.
    let mut config = jumppad_config::Config::default();
    config
        .languages
        .push(language("Zig", Some("zig"), &["zig"], None));

    let _ = app.apply_config(config.clone());
    assert_eq!(app.theme.to_string(), theme_before);
    assert_eq!(app.background_alpha, 1.0);
    // The new values still become the diff baseline, so the
    // restart-required log fires once per transition rather than on
    // every later unrelated reload.
    assert_eq!(app.config, config);
}

/// A window setting can only be honored by a new window, and only when
/// there is an old one to replace.
#[test]
fn a_window_setting_asks_for_a_replacement_window() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());
    let config = jumppad_config::Config {
        window: jumppad_config::WindowConfig { decorations: false },
        ..Default::default()
    };

    // A `Task` only reports how much work it carries, so the check is
    // that it carries some where a live-only reload carries none.
    let replacement = app.apply_config(config);
    assert_ne!(replacement.units(), 0, "no window replacement issued");
}

#[test]
fn a_live_only_reload_leaves_the_window_alone() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());

    let live_only =
        app.apply_config(config_with_theme(r#"palette = "Nord""#));
    assert_eq!(live_only.units(), 0, "nothing to do to the window");
    assert_eq!(app.theme.to_string(), "Nord", "but the theme still moved");
}

/// Turning the visor on takes a new window - undecorated and floating -
/// and the global hotkey that summons it.
#[test]
fn turning_the_visor_on_reaches_the_hotkey_as_well_as_the_window() {
    let mut app = test_app(1);
    app.window = Some(iced::window::Id::unique());
    let config = jumppad_config::Config {
        visor: jumppad_config::VisorConfig { enabled: true },
        ..Default::default()
    };

    let _ = app.apply_config(config);
    assert!(app.visor_enabled);
}

fn key_press(
    named: Named,
    modifiers: Modifiers,
    code: key::Code,
) -> Message {
    Message::KeyPressed(
        Key::Named(named),
        modifiers,
        key::Physical::Code(code),
    )
}

#[test]
fn request_close_queues_a_second_request_instead_of_showing_a_second_prompt()
 {
    let mut app = test_app(3);
    app.tabs[0].dirty = true;
    app.tabs[1].dirty = true;
    let tab0_id = app.tabs[0].id;
    let tab1_id = app.tabs[1].id;

    let _ = app.request_close(0);
    assert_eq!(close_prompt(&app).expect("a close prompt").tab_id, tab0_id);
    assert!(app.close_queue.is_empty());

    let _ = app.request_close(1);
    assert_eq!(close_prompt(&app).expect("a close prompt").tab_id, tab0_id); // unchanged
    assert_eq!(app.close_queue, vec![tab1_id]);
}

#[test]
fn request_close_does_not_duplicate_an_already_queued_id() {
    let mut app = test_app(3);
    app.tabs[0].dirty = true;
    app.tabs[1].dirty = true;
    let _ = app.request_close(0);
    let _ = app.request_close(1);
    let _ = app.request_close(1);
    assert_eq!(app.close_queue.len(), 1);
}

#[test]
fn close_confirmed_opens_the_next_queued_prompt() {
    let mut app = test_app(3);
    app.tabs[0].dirty = true;
    app.tabs[1].dirty = true;
    let tab1_id = app.tabs[1].id;
    let _ = app.request_close(0);
    let _ = app.request_close(1);

    let tab0_id = app.tabs[0].id;
    let _ = app
        .update(Message::CloseConfirmed(tab0_id, CloseDecision::DontSave));

    assert!(app.close_queue.is_empty());
    assert_eq!(close_prompt(&app).expect("a close prompt").tab_id, tab1_id);
}

#[test]
fn key_pressed_cycles_focused_choice_both_directions_with_wraparound() {
    let mut app = test_app(1);
    app.tabs[0].dirty = true;
    let _ = app.request_close(0);
    assert_eq!(close_prompt(&app).expect("a close prompt").focused, 0);

    let _ = app.update(key_press(
        Named::ArrowRight,
        Modifiers::empty(),
        key::Code::ArrowRight,
    ));
    assert_eq!(close_prompt(&app).expect("a close prompt").focused, 1);

    let _ = app.update(key_press(
        Named::Tab,
        Modifiers::empty(),
        key::Code::Tab,
    ));
    assert_eq!(close_prompt(&app).expect("a close prompt").focused, 2);

    // Wraps back around to 0.
    let _ = app.update(key_press(
        Named::ArrowRight,
        Modifiers::empty(),
        key::Code::ArrowRight,
    ));
    assert_eq!(close_prompt(&app).expect("a close prompt").focused, 0);

    // Shift+Tab goes backward, wrapping to the last choice.
    let _ =
        app.update(key_press(Named::Tab, Modifiers::SHIFT, key::Code::Tab));
    assert_eq!(close_prompt(&app).expect("a close prompt").focused, 2);

    let _ = app.update(key_press(
        Named::ArrowLeft,
        Modifiers::empty(),
        key::Code::ArrowLeft,
    ));
    assert_eq!(close_prompt(&app).expect("a close prompt").focused, 1);
}

#[test]
fn key_pressed_enter_resolves_whichever_choice_is_focused() {
    let mut app = test_app(2);
    app.tabs[0].dirty = true;
    let _ = app.request_close(0);
    // Move focus to "Don't Save" (index 1).
    let _ = app.update(key_press(
        Named::ArrowRight,
        Modifiers::empty(),
        key::Code::ArrowRight,
    ));
    let tabs_before = app.tabs.len();

    let _ = app.update(key_press(
        Named::Enter,
        Modifiers::empty(),
        key::Code::Enter,
    ));

    assert!(app.modal.is_none());
    assert_eq!(app.tabs.len(), tabs_before - 1); // Don't Save actually closed it
}

#[test]
fn key_pressed_escape_always_cancels_regardless_of_focus() {
    let mut app = test_app(2);
    app.tabs[0].dirty = true;
    let _ = app.request_close(0);
    // Move focus to "Don't Save" - Escape should still cancel, not "Don't Save".
    let _ = app.update(key_press(
        Named::ArrowRight,
        Modifiers::empty(),
        key::Code::ArrowRight,
    ));
    let tabs_before = app.tabs.len();

    let _ = app.update(key_press(
        Named::Escape,
        Modifiers::empty(),
        key::Code::Escape,
    ));

    assert!(app.modal.is_none());
    assert_eq!(app.tabs.len(), tabs_before); // nothing closed
}

#[test]
fn key_pressed_swallows_app_shortcuts_while_a_prompt_is_pending() {
    let mut app = test_app(1);
    app.tabs[0].dirty = true;
    let _ = app.request_close(0);
    let tabs_before = app.tabs.len();

    // Ctrl+N would normally fire NewTab (see hardcoded_default_still_fires_with_empty_overrides).
    let _ = app.update(Message::KeyPressed(
        Key::Character("n".into()),
        Modifiers::CTRL,
        key::Physical::Code(key::Code::KeyN),
    ));

    assert_eq!(app.tabs.len(), tabs_before);
    assert!(app.modal.is_some());
}

#[test]
fn editor_messages_are_ignored_while_a_prompt_is_pending() {
    let mut app = test_app(1);
    app.tabs[0].dirty = true;
    let generation_before = app.tabs[0].draft_generation;
    let _ = app.request_close(0);

    let _ = app.update(Message::Editor(0, EditorMessage::Undo));

    assert_eq!(app.tabs[0].draft_generation, generation_before);
}

#[test]
fn open_file_does_not_spawn_a_second_dialog_while_one_is_active() {
    let mut app = test_app(1);
    let _ = app.update(Message::OpenFile);
    assert!(app.file_dialog_active);
    // A second OpenFile while one's in flight is a no-op.
    let _ = app.update(Message::OpenFile);
    assert!(app.file_dialog_active);
}

#[test]
fn file_dialog_flag_resets_on_every_file_opened_outcome() {
    let mut app = test_app(1);
    app.file_dialog_active = true;
    let _ = app.update(Message::FileOpened(Err(OpenError::DialogClosed)));
    assert!(!app.file_dialog_active);

    app.file_dialog_active = true;
    let _ = app.update(Message::FileOpened(Err(OpenError::Io {
        path: PathBuf::from("/tmp/x"),
        kind: std::io::ErrorKind::NotFound,
    })));
    assert!(!app.file_dialog_active);
}

/// A fresh scratch directory per test, under the OS temp dir - the same
/// shape `session.rs`'s tests use.
fn argv_scratch_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("jumppad-argv-test-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn a_named_file_loads_into_the_startup_scratch_tab() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "hello").expect("write");

    let mut app = test_app(1);
    let id = app.tabs[0].id;
    let _ = app.update(Message::OpenPaths(vec![file.clone()]));

    assert_eq!(app.tabs.len(), 1, "no stray Untitled left behind");
    assert_eq!(app.tabs[0].id, id);
    assert_eq!(app.tabs[0].document.path.as_deref(), Some(file.as_path()));
}

#[test]
fn named_files_open_in_order_with_the_last_active() {
    let dir = argv_scratch_dir();
    let first = dir.join("a.txt");
    let second = dir.join("b.txt");
    std::fs::write(&first, "a").expect("write");
    std::fs::write(&second, "b").expect("write");

    let mut app = test_app(1);
    let _ =
        app.update(Message::OpenPaths(vec![first.clone(), second.clone()]));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs[0].document.path.as_deref(), Some(first.as_path()));
    assert_eq!(
        app.tabs[1].document.path.as_deref(),
        Some(second.as_path())
    );
    assert_eq!(app.active, 1, "the last one named is the one you land on");
}

#[test]
fn naming_a_file_that_does_not_exist_opens_an_empty_buffer_for_it() {
    let dir = argv_scratch_dir();
    let file = dir.join("brand-new.md");

    let mut app = test_app(1);
    let _ = app.update(Message::OpenPaths(vec![file.clone()]));

    assert_eq!(app.tabs.len(), 1);
    assert_eq!(
        app.tabs[0].document.path.as_deref(),
        Some(file.as_path()),
        "bound to the path, so the first save creates it"
    );
    assert!(app.tabs[0].editor.text().is_empty());
    assert!(!app.tabs[0].dirty, "nothing typed yet");
    assert!(
        app.error.is_none(),
        "a file you're about to create isn't an error"
    );
}

#[test]
fn naming_a_directory_surfaces_an_error_and_opens_nothing() {
    let dir = argv_scratch_dir();

    let mut app = test_app(1);
    let _ = app.update(Message::OpenPaths(vec![dir.clone()]));

    assert_eq!(app.tabs.len(), 1);
    assert!(app.tabs[0].document.path.is_none());
    let error = app.error.expect("an error row");
    assert!(error.contains("is a folder"), "got {error:?}");
}

#[test]
fn naming_an_already_open_file_focuses_its_tab() {
    let dir = argv_scratch_dir();
    let file = dir.join("restored.txt");
    std::fs::write(&file, "body").expect("write");

    let mut app = test_app(2);
    app.tabs[1].document.path = Some(file.clone());
    let _ = app.update(Message::OpenPaths(vec![file]));

    assert_eq!(app.tabs.len(), 2, "no duplicate of a restored tab");
    assert_eq!(app.active, 1);
}

#[test]
fn naming_no_files_changes_nothing() {
    let mut app = test_app(1);
    let _ = app.update(Message::OpenPaths(Vec::new()));

    assert_eq!(app.tabs.len(), 1);
    assert!(app.tabs[0].document.path.is_none());
    assert!(app.error.is_none());
}

/// An app whose one tab is backed by `path`, holding whatever is on disk
/// there and stamped against it - the state every external-change test
/// starts from. Its session dir is the scratch dir, so manifest writes
/// stay out of the real one.
fn app_watching(dir: &Path, path: &Path) -> JumpPadApp {
    let mut app = test_app(0);
    app.session_dir = dir.to_path_buf();
    let contents = std::fs::read_to_string(path).unwrap_or_default();
    let mut tab = Tab::from_file(
        0,
        path.to_path_buf(),
        &contents,
        &app.editor_factory,
    );
    tab.restamp();
    app.tabs = vec![tab];
    app.next_id = 1;
    app
}

fn reload_message(id: u64, path: &Path, contents: &str) -> Message {
    Message::DocumentReloaded(
        id,
        path.to_path_buf(),
        DiskStamp::of(path),
        Ok(Arc::new(contents.to_string())),
    )
}

#[test]
fn a_clean_tab_reloads_silently_when_its_file_changes() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "before").expect("write");
    let mut app = app_watching(&dir, &file);
    let stamped_at_open = app.tabs[0].disk;

    std::fs::write(&file, "after the change").expect("rewrite");
    let reads = app.resolve_disk_changes();

    assert_eq!(reads.len(), 1, "the clean tab is queued for a re-read");
    assert!(!app.tabs[0].dirty, "a reload is not an edit");
    assert!(!app.tabs[0].externally_changed, "nothing to warn about");
    assert_eq!(
        app.tabs[0].disk, stamped_at_open,
        "stamped when the read lands, not when it's queued"
    );

    let _ = app.update(reload_message(0, &file, "after the change"));
    assert_eq!(app.tabs[0].editor.text(), "after the change");
    assert!(!app.tabs[0].dirty);
    assert_eq!(app.tabs[0].disk, DiskStamp::of(&file));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dirty_tab_keeps_its_buffer_and_is_flagged_instead() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "before").expect("write");
    let mut app = app_watching(&dir, &file);
    app.tabs[0].dirty = true;
    app.tabs[0].editor.set_text("my unsaved edits");
    let stamp_before = app.tabs[0].disk;

    std::fs::write(&file, "someone else's version").expect("rewrite");
    let reads = app.resolve_disk_changes();

    assert!(reads.is_empty(), "unsaved edits are never overwritten");
    assert_eq!(app.tabs[0].editor.text(), "my unsaved edits");
    assert!(app.tabs[0].externally_changed);
    assert_eq!(
        app.tabs[0].disk, stamp_before,
        "the old stamp is what the save-time check compares against"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_deleted_file_leaves_the_tab_open_and_dirty() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "still wanted").expect("write");
    let mut app = app_watching(&dir, &file);

    std::fs::remove_file(&file).expect("delete");
    let reads = app.resolve_disk_changes();

    assert!(reads.is_empty(), "there's nothing to read");
    assert_eq!(app.tabs.len(), 1, "the tab stays open");
    assert_eq!(
        app.tabs[0].editor.text(),
        "still wanted",
        "content preserved"
    );
    assert!(app.tabs[0].dirty, "so the next save recreates the file");
    assert_eq!(app.tabs[0].disk, None);
    assert_eq!(
        app.tabs[0].document.path.as_deref(),
        Some(file.as_path()),
        "still bound to the path it will be written back to"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn our_own_save_does_not_read_as_an_external_change() {
    // The reload-loop guard: the save task stamps what it wrote, so the
    // watcher event it causes compares equal and sweeps to nothing.
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "original").expect("write");
    let mut app = app_watching(&dir, &file);
    app.tabs[0].dirty = true;

    std::fs::write(&file, "saved by jumppad").expect("the save");
    let _ = app.update(Message::FileSaved(
        0,
        Ok((file.clone(), DiskStamp::of(&file))),
    ));

    let reads = app.resolve_disk_changes();
    assert!(reads.is_empty(), "our own write must not trigger a reload");
    assert!(!app.tabs[0].dirty);
    assert!(!app.tabs[0].externally_changed);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reload_arriving_after_the_user_typed_is_dropped_and_flags_instead() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "before").expect("write");
    let mut app = app_watching(&dir, &file);
    std::fs::write(&file, "from another program").expect("rewrite");
    assert_eq!(app.resolve_disk_changes().len(), 1);

    // The user types while the read is in flight.
    let _ = app.update(Message::Editor(0, EditorMessage::Undo));
    assert!(app.tabs[0].dirty);

    let _ = app.update(reload_message(0, &file, "from another program"));

    assert_eq!(
        app.tabs[0].editor.text(),
        "before",
        "the in-flight reload must not clobber what was just typed"
    );
    assert!(app.tabs[0].externally_changed);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reload_for_a_path_the_tab_no_longer_holds_is_dropped() {
    // Save As can retarget a tab while a read is in flight.
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "before").expect("write");
    let mut app = app_watching(&dir, &file);
    app.tabs[0].document.path = Some(dir.join("saved-as.txt"));

    let _ = app.update(reload_message(0, &file, "content of the old path"));

    assert_eq!(app.tabs[0].editor.text(), "before");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_tabs_on_one_path_both_react() {
    // `tab_index_for` stops the same file opening twice, but Save As can
    // still leave two tabs pointing at one path - the sweep visits every
    // tab, not the first match.
    let dir = argv_scratch_dir();
    let file = dir.join("shared.txt");
    std::fs::write(&file, "before").expect("write");
    let mut app = app_watching(&dir, &file);
    let mut second =
        Tab::from_file(1, file.clone(), "before", &app.editor_factory);
    second.restamp();
    second.dirty = true;
    app.tabs.push(second);
    app.next_id = 2;

    std::fs::write(&file, "changed by another program").expect("rewrite");
    let reads = app.resolve_disk_changes();

    assert_eq!(reads.len(), 1, "only the clean tab re-reads");
    assert_eq!(reads[0].0, 0);
    assert!(app.tabs[1].externally_changed, "the dirty one is flagged");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_over_a_changed_file_reports_a_conflict() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "as opened").expect("write");
    let opened = SaveExpectation::Seen(DiskStamp::of(&file));

    assert!(!conflicts(&file, opened), "nothing moved");
    assert!(
        !conflicts(&file, SaveExpectation::Unchecked),
        "a Save As target has nothing to compare against"
    );

    std::fs::write(&file, "changed by another program").expect("rewrite");
    assert!(conflicts(&file, opened), "the file moved under the tab");
    assert!(
        !conflicts(&file, SaveExpectation::Unchecked),
        "and Unchecked still writes over it"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_appeared_where_the_tab_saw_none_is_a_conflict() {
    // `jumppad newnote.md` on a file that doesn't exist yet, and then
    // something else creates it. The buffer has never seen those bytes,
    // so a save would clobber them.
    let dir = argv_scratch_dir();
    let file = dir.join("newnote.md");
    let saw_nothing = SaveExpectation::Seen(None);
    assert!(!conflicts(&file, saw_nothing), "nothing there yet");

    std::fs::write(&file, "created by another program").expect("write");
    assert!(conflicts(&file, saw_nothing));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_deleted_file_is_not_a_conflict_to_save_over() {
    // The delete rule: there is nothing to clobber, so the save just
    // recreates the file - no prompt, whatever the tab last saw.
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "as opened").expect("write");
    let opened = SaveExpectation::Seen(DiskStamp::of(&file));

    std::fs::remove_file(&file).expect("delete");
    assert!(!conflicts(&file, opened));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A dirty tab whose file changed underneath it, with the conflict
/// already surfaced - the state both dialog and banner start from.
fn app_in_conflict(dir: &Path, file: &Path) -> JumpPadApp {
    std::fs::write(file, "as opened").expect("write");
    let mut app = app_watching(dir, file);
    app.tabs[0].dirty = true;
    app.tabs[0].editor.set_text("my unsaved edits");
    std::fs::write(file, "changed by another program").expect("rewrite");
    app.resolve_disk_changes();
    assert!(app.tabs[0].externally_changed);
    app
}

#[test]
fn a_conflicted_save_prompts_instead_of_writing() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    app.tabs[0].externally_changed = false; // the sweep hasn't run yet

    let _ = app.update(Message::FileSaved(0, Err(SaveError::Conflict)));

    assert_eq!(conflict_prompt(&app).expect("a conflict prompt").tab_id, 0);
    assert!(
        app.tabs[0].externally_changed,
        "the save is how it found out"
    );
    assert!(!app.file_dialog_active);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "changed by another program",
        "nothing was written"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn overwrite_writes_and_clears_the_flag() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    let _ = app.update(Message::FileSaved(0, Err(SaveError::Conflict)));

    let _ = app
        .update(Message::ConflictResolved(0, ConflictDecision::Overwrite));
    assert!(app.modal.is_none(), "the prompt is dismissed");

    // The re-run save lands the way any other successful save does.
    std::fs::write(&file, "my unsaved edits").expect("the overwrite");
    let _ = app.update(Message::FileSaved(
        0,
        Ok((file.clone(), DiskStamp::of(&file))),
    ));

    assert!(!app.tabs[0].dirty);
    assert!(!app.tabs[0].externally_changed);
    assert_eq!(app.tabs[0].disk, DiskStamp::of(&file));
    assert!(
        app.resolve_disk_changes().is_empty(),
        "and the overwrite doesn't read back as an external change"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_and_reload_replaces_the_buffer_and_goes_clean() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);

    let _ = app.update(Message::ConflictResolved(
        0,
        ConflictDecision::DiscardAndReload,
    ));
    let _ = app.update(Message::ConflictReloaded(
        0,
        file.clone(),
        DiskStamp::of(&file),
        Ok(Arc::new("changed by another program".to_string())),
    ));

    assert_eq!(app.tabs[0].editor.text(), "changed by another program");
    assert!(!app.tabs[0].dirty, "the buffer matches disk again");
    assert!(!app.tabs[0].externally_changed);
    assert_eq!(app.tabs[0].disk, DiskStamp::of(&file));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancelling_a_conflict_changes_nothing() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    let _ = app.update(Message::FileSaved(0, Err(SaveError::Conflict)));

    let _ =
        app.update(Message::ConflictResolved(0, ConflictDecision::Cancel));

    assert!(app.modal.is_none());
    assert!(app.tabs[0].dirty, "still unsaved");
    assert!(app.tabs[0].externally_changed, "still flagged");
    assert_eq!(app.tabs[0].editor.text(), "my unsaved edits");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "changed by another program"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn keep_mine_restamps_so_the_next_save_does_not_prompt() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);

    let _ = app.update(Message::AcknowledgeExternalChange(0));

    assert!(!app.tabs[0].externally_changed);
    assert!(app.tabs[0].dirty, "the edits are still unsaved");
    assert_eq!(app.tabs[0].editor.text(), "my unsaved edits");
    assert!(
        !conflicts(&file, SaveExpectation::Seen(app.tabs[0].disk)),
        "acknowledging records the version seen, so the next save goes through"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_conflict_during_a_close_prompt_save_aborts_the_close() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    // The "Save" branch of the unsaved-changes prompt.
    let _ = app.request_close(0);
    let _ = app.update(Message::CloseConfirmed(0, CloseDecision::Save));
    assert_eq!(app.pending_close_after_save, vec![0]);

    let _ = app.update(Message::FileSaved(0, Err(SaveError::Conflict)));

    assert_eq!(app.tabs.len(), 1, "the tab stays open");
    assert!(
        app.pending_close_after_save.is_empty(),
        "a failed save cancels the close it was for"
    );
    assert!(conflict_prompt(&app).is_some());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_conflict_waits_behind_the_close_prompt() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    let mut other = Tab::untitled(1, &app.editor_factory);
    other.dirty = true;
    app.tabs.push(other);
    app.next_id = 2;

    let _ = app.request_close(1);
    assert!(close_prompt(&app).is_some());

    // The conflict arrives while that prompt is up - queued, not stacked.
    let _ = app.update(Message::FileSaved(0, Err(SaveError::Conflict)));
    assert!(
        close_prompt(&app).is_some(),
        "the close prompt keeps the screen"
    );
    assert_eq!(app.conflict_queue, vec![0]);

    let _ = app.update(Message::CloseConfirmed(1, CloseDecision::Cancel));
    assert_eq!(conflict_prompt(&app).expect("queued conflict").tab_id, 0);
    assert!(app.conflict_queue.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_conflict_prompt_navigates_like_the_close_prompt() {
    // Both dialogs share one keyboard path - three choices, cycled mod 3,
    // Enter resolving whichever is focused and Escape always cancelling.
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    let _ = app.update(Message::FileSaved(0, Err(SaveError::Conflict)));
    assert_eq!(
        conflict_prompt(&app).unwrap().focused,
        2,
        "opens on Cancel - the other two both destroy someone's work"
    );

    let _ = app.update(key_press(
        Named::ArrowRight,
        Modifiers::empty(),
        key::Code::ArrowRight,
    ));
    assert_eq!(conflict_prompt(&app).unwrap().focused, 0, "wraps forward");
    let _ =
        app.update(key_press(Named::Tab, Modifiers::SHIFT, key::Code::Tab));
    assert_eq!(conflict_prompt(&app).unwrap().focused, 2, "and backward");

    // Focus "Discard & Reload" and take it.
    let _ = app.update(key_press(
        Named::ArrowLeft,
        Modifiers::empty(),
        key::Code::ArrowLeft,
    ));
    let _ = app.update(key_press(
        Named::Enter,
        Modifiers::empty(),
        key::Code::Enter,
    ));
    assert!(app.modal.is_none());
    let _ = app.update(Message::ConflictReloaded(
        0,
        file.clone(),
        DiskStamp::of(&file),
        Ok(Arc::new("changed by another program".to_string())),
    ));
    assert_eq!(app.tabs[0].editor.text(), "changed by another program");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn escape_cancels_the_conflict_prompt_whatever_is_focused() {
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    let _ = app.update(Message::FileSaved(0, Err(SaveError::Conflict)));
    // Focused on "Overwrite" - Escape must still cancel.
    let _ = app.update(key_press(
        Named::Escape,
        Modifiers::empty(),
        key::Code::Escape,
    ));

    assert!(app.modal.is_none());
    assert!(app.tabs[0].externally_changed);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "changed by another program"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_changed_on_disk_bar_is_inert_while_a_modal_is_up() {
    // The scrim only swallows clicks the widget tree routes through it,
    // and the bar sits underneath - so its two messages have to turn
    // themselves away, the way `Editor` and `FileDropped` do.
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    let mut other = Tab::untitled(1, &app.editor_factory);
    other.dirty = true;
    app.tabs.push(other);
    let _ = app.request_close(1);
    let stamp_before = app.tabs[0].disk;

    let _ = app.update(Message::AcknowledgeExternalChange(0));
    assert!(app.tabs[0].externally_changed, "still flagged");
    assert_eq!(app.tabs[0].disk, stamp_before, "and not re-stamped");

    let _ = app.update(Message::ReloadFromDisk(0));
    assert_eq!(app.tabs[0].editor.text(), "my unsaved edits");
    assert!(close_prompt(&app).is_some(), "the close prompt is still up");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_queued_prompt_that_went_clean_does_not_strand_the_rest() {
    // `request_close` closes a clean tab outright instead of prompting,
    // so draining has to keep going rather than assume a dialog opened.
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    let mut queued = Tab::untitled(1, &app.editor_factory);
    queued.dirty = true;
    let mut showing = Tab::untitled(2, &app.editor_factory);
    showing.dirty = true;
    app.tabs.push(queued);
    app.tabs.push(showing);
    app.next_id = 3;

    let _ = app.request_close(2); // this one gets the prompt
    let _ = app.request_close(1); // queued behind it
    let _ = app.update(Message::FileSaved(0, Err(SaveError::Conflict))); // queued too
    assert_eq!(app.close_queue, vec![1]);
    assert_eq!(app.conflict_queue, vec![0]);

    // Tab 1 goes clean while it waits - closing it must not swallow the
    // conflict waiting behind it.
    app.tabs[1].dirty = false;
    let _ = app.update(Message::CloseConfirmed(2, CloseDecision::Cancel));

    assert!(app.close_queue.is_empty());
    assert!(app.conflict_queue.is_empty());
    assert!(
        !app.tabs.iter().any(|tab| tab.id == 1),
        "the clean tab closed"
    );
    assert_eq!(
        conflict_prompt(&app)
            .expect("the queued conflict still shows")
            .tab_id,
        0
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unreadable_file_leaves_its_tab_alone() {
    // Only NotFound means "deleted". A read that failed for any other
    // reason must not dirty a tab the user never touched.
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "before").expect("write");
    let mut app = app_watching(&dir, &file);
    let stamp = app.tabs[0].disk;

    let _ = app.update(Message::DocumentReloaded(
        0,
        file.clone(),
        DiskStamp::of(&file),
        Err(std::io::ErrorKind::InvalidData),
    ));

    assert!(!app.tabs[0].dirty, "a failed read is not an edit");
    assert_eq!(app.tabs[0].disk, stamp);
    assert_eq!(app.tabs[0].editor.text(), "before");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reload_refreshes_the_find_state_of_the_tab_that_moved() {
    // The reloaded tab's match list holds ranges into text that just
    // went away - and it isn't necessarily the active tab.
    let mut app = app_with_text(&["active tab", "background tab"]);
    let dir = argv_scratch_dir();
    let file = dir.join("background.txt");
    std::fs::write(&file, "one two two").expect("write");
    app.session_dir = dir.clone();
    app.tabs[1].document.path = Some(file.clone());
    app.tabs[1].editor.set_text("one two two");
    app.tabs[1].restamp();

    // A palette open on the background tab, matching twice.
    let _ = app.update(Message::SelectTab(1));
    let _ = app.update(Message::OpenFind);
    let _ = app.update(Message::FindQueryChanged("two".into()));
    let _ = app.update(Message::SelectTab(0));
    assert_eq!(app.find[&app.tabs[1].id].matches.len(), 2);

    let _ = app.update(reload_message(app.tabs[1].id, &file, "one"));

    assert_eq!(
        app.find[&app.tabs[1].id].matches.len(),
        0,
        "the background tab's matches were re-searched against the new text"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_restored_tab_is_stamped_before_its_re_read_is_queued() {
    // Otherwise a focus sweep arriving first sees a clean, stamp-less
    // tab and reloads the file over the empty buffer it starts with -
    // one Ctrl+Z away from blanking the document.
    let dir = argv_scratch_dir();
    let file = dir.join("restored.txt");
    std::fs::write(&file, "on disk").expect("write");

    let mut app = test_app(0);
    app.session_dir = dir.clone();
    let mut tab = Tab::restored(
        0,
        Some(file.clone()),
        "",
        false,
        &app.editor_factory,
    );
    tab.restamp();
    app.tabs = vec![tab];

    assert!(
        app.resolve_disk_changes().is_empty(),
        "a sweep before the re-read lands must find nothing to do"
    );
    assert_eq!(
        app.tabs[0].editor.text(),
        "",
        "the buffer is left for the re-read"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_flagged_tab_says_so_in_its_title() {
    let factory = stub_factory();
    let mut tab =
        Tab::from_file(0, PathBuf::from("/tmp/notes.txt"), "", &factory);
    assert_eq!(tab.title(), "notes.txt");
    tab.dirty = true;
    assert_eq!(tab.title(), "notes.txt \u{2022}");
    tab.externally_changed = true;
    assert_eq!(tab.title(), "notes.txt \u{2022} \u{26a0}");
}

#[test]
fn the_overwrite_setting_skips_the_conflict_check() {
    // Read from `self.config` at save time, so a reloaded config.toml
    // takes effect without an `apply_config` arm.
    let dir = argv_scratch_dir();
    let file = dir.join("notes.txt");
    let mut app = app_in_conflict(&dir, &file);
    let tab = &app.tabs[0];
    assert!(app.config.files.save_conflict_resolution.asks());
    assert_eq!(
        app.save_expectation(tab, false),
        SaveExpectation::Seen(tab.disk),
        "the default setting checks against what the tab last saw"
    );
    assert_eq!(
        app.save_expectation(tab, true),
        SaveExpectation::Unchecked,
        "a Save As dialog asks its own overwrite question"
    );

    app.config.files.save_conflict_resolution =
        jumppad_config::SaveConflictResolution::Overwrite;
    assert_eq!(
        app.save_expectation(&app.tabs[0], false),
        SaveExpectation::Unchecked,
        "saves always win under the overwrite setting"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_watched_path_list_is_sorted_deduped_and_file_backed_only() {
    // It's the watcher subscription's identity: an unstable order would
    // tear the watcher down and rebuild it on every unrelated change.
    let mut app = test_app(3);
    app.tabs[0].document.path = Some(PathBuf::from("/tmp/zebra.txt"));
    app.tabs[1].document.path = Some(PathBuf::from("/tmp/apple.txt"));
    app.tabs[2].document.path = Some(PathBuf::from("/tmp/zebra.txt"));

    assert_eq!(
        app.watched_paths(),
        vec![
            PathBuf::from("/tmp/apple.txt"),
            PathBuf::from("/tmp/zebra.txt")
        ]
    );

    app.tabs[0].document.path = None;
    app.tabs[2].document.path = None;
    assert_eq!(app.watched_paths(), vec![PathBuf::from("/tmp/apple.txt")]);
}

#[test]
fn an_untitled_tab_is_never_swept() {
    let mut app = test_app(1);
    assert!(app.resolve_disk_changes().is_empty());
    assert!(!app.tabs[0].dirty);
    assert!(app.watched_paths().is_empty());
}

#[test]
fn hovering_files_over_the_window_toggles_the_drop_overlay() {
    let mut app = test_app(1);
    let _ = app.update(Message::FilesHovered(true));
    assert!(app.files_hovered);
    let _ = app.update(Message::FilesHovered(false));
    assert!(!app.files_hovered);
}

#[test]
fn dropping_a_file_clears_the_hover_overlay() {
    // Windows and macOS send no `FilesHoveredLeft` after a completed drop,
    // so the drop itself has to dismiss the overlay.
    let mut app = test_app(1);
    app.files_hovered = true;
    let _ =
        app.update(Message::FileDropped(PathBuf::from("/tmp/dropped.txt")));
    assert!(!app.files_hovered);
}

#[test]
fn dropping_onto_an_empty_scratch_tab_loads_into_it() {
    let mut app = test_app(1);
    let id = app.tabs[0].id;
    let _ = app.update(Message::DroppedFileRead(Ok((
        PathBuf::from("/tmp/dropped.txt"),
        Arc::new("body".to_string()),
    ))));

    assert_eq!(app.tabs.len(), 1, "no stray Untitled left behind");
    assert_eq!(app.tabs[0].id, id, "the tab keeps its id");
    assert_eq!(
        app.tabs[0].document.path.as_deref(),
        Some(Path::new("/tmp/dropped.txt"))
    );
}

#[test]
fn dropping_onto_a_file_backed_tab_opens_a_new_one() {
    let mut app = test_app(1);
    app.tabs[0].document.path =
        Some(PathBuf::from("/tmp/already-open.txt"));
    let _ = app.update(Message::DroppedFileRead(Ok((
        PathBuf::from("/tmp/dropped.txt"),
        Arc::new("body".to_string()),
    ))));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active, 1);
    assert_eq!(
        app.tabs[1].document.path.as_deref(),
        Some(Path::new("/tmp/dropped.txt"))
    );
}

#[test]
fn dropping_an_already_open_file_focuses_its_tab() {
    let mut app = test_app(2);
    app.tabs[1].document.path =
        Some(PathBuf::from("/tmp/already-open.txt"));
    let _ = app.update(Message::FileDropped(PathBuf::from(
        "/tmp/already-open.txt",
    )));

    assert_eq!(app.tabs.len(), 2, "no duplicate tab");
    assert_eq!(app.active, 1);
}

#[test]
fn dropping_a_file_leaves_an_in_flight_dialog_alone() {
    let mut app = test_app(1);
    app.file_dialog_active = true;
    let _ =
        app.update(Message::FileDropped(PathBuf::from("/tmp/dropped.txt")));
    assert!(app.file_dialog_active);

    let _ = app.update(Message::DroppedFileRead(Ok((
        PathBuf::from("/tmp/dropped.txt"),
        Arc::new("body".to_string()),
    ))));
    assert!(app.file_dialog_active);
}

#[test]
fn the_unsaved_changes_prompt_turns_drops_away() {
    let mut app = test_app(1);
    app.modal = Some(Modal::Close(PendingClose {
        tab_id: app.tabs[0].id,
        title: "Untitled".into(),
        focused: 0,
    }));
    app.files_hovered = true;
    let _ =
        app.update(Message::FileDropped(PathBuf::from("/tmp/dropped.txt")));

    assert!(!app.files_hovered, "the overlay still clears");
    assert!(app.tabs[0].document.path.is_none(), "nothing was opened");
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn an_unreadable_dropped_file_surfaces_an_error() {
    let mut app = test_app(1);
    let _ = app.update(Message::DroppedFileRead(Err(OpenError::Io {
        path: PathBuf::from("/tmp/dropped.bin"),
        kind: std::io::ErrorKind::InvalidData,
    })));

    assert!(app.error.is_some());
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn file_dialog_flag_resets_on_every_file_saved_outcome() {
    let mut app = test_app(1);
    let id = app.tabs[0].id;

    app.file_dialog_active = true;
    let _ =
        app.update(Message::FileSaved(id, Err(SaveError::DialogClosed)));
    assert!(!app.file_dialog_active);

    app.file_dialog_active = true;
    let _ = app.update(Message::FileSaved(
        id,
        Err(SaveError::Io {
            path: PathBuf::from("/tmp/x"),
            kind: std::io::ErrorKind::NotFound,
        }),
    ));
    assert!(!app.file_dialog_active);

    app.file_dialog_active = true;
    let _ = app.update(Message::FileSaved(
        id,
        Ok((PathBuf::from("/tmp/x"), None)),
    ));
    assert!(!app.file_dialog_active);
}

#[test]
fn save_tab_does_not_spawn_a_second_dialog_for_an_untitled_tab_while_one_is_active()
 {
    // A Save on a never-saved tab shows a file-picker just like Open File does.
    let mut app = test_app(1);
    assert!(app.tabs[0].document.path.is_none());
    app.file_dialog_active = true;

    let _ = app.save_active_tab(false);

    assert!(app.file_dialog_active);
}

/// Composites `wash` over `base` the way the renderer does, source-over.
fn composite(base: Color, wash: Color) -> Color {
    Color {
        r: wash.r * wash.a + base.r * (1.0 - wash.a),
        g: wash.g * wash.a + base.g * (1.0 - wash.a),
        b: wash.b * wash.a + base.b * (1.0 - wash.a),
        a: wash.a + base.a * (1.0 - wash.a),
    }
}

fn luminance(color: Color) -> f32 {
    (color.r + color.g + color.b) / 3.0
}

#[test]
fn darkening_wash_darkens_by_the_requested_amount_on_every_theme_bright_enough_to()
 {
    let mut checked = 0;
    for theme in Theme::ALL {
        let base = theme.extended_palette().background.base.color;
        // Near-black themes can't give up this much light without a solid
        // wash; they're capped by design, not mispredicted.
        if TAB_ROW_DARKEN / luminance(base) > WASH_ALPHA_CEILING {
            continue;
        }
        let washed = composite(base, darkening_wash(theme, TAB_ROW_DARKEN));
        let wanted = luminance(base) - TAB_ROW_DARKEN;
        assert!(
            (luminance(washed) - wanted).abs() < 0.001,
            "{theme}: washed to {}, wanted {wanted}",
            luminance(washed)
        );
        checked += 1;
    }
    assert!(checked > 0, "no theme was actually checked");
}

// The scrollbar thumb and the find palette used to be required to share
// one material (an invariant test lived here). The thumb now washes
// toward white on dark themes and toward black on light ones, while the
// find palette keeps washing toward black always - an intentional
// divergence, so that invariant no longer holds.

#[test]
fn scrollbar_thumb_wash_lightens_dark_themes_and_darkens_light_ones() {
    let mut checked_dark = 0;
    let mut checked_light = 0;
    for theme in Theme::ALL {
        let base = theme.extended_palette().background.base.color;
        let washed = jumppad_textarea::scrollbar_thumb_style(theme);
        let composited_luminance = luminance(composite(base, washed));
        if theme.extended_palette().is_dark {
            assert!(
                composited_luminance >= luminance(base),
                "{theme}: dark theme's thumb wash didn't lighten it"
            );
            checked_dark += 1;
        } else {
            assert!(
                composited_luminance <= luminance(base),
                "{theme}: light theme's thumb wash didn't darken it"
            );
            checked_light += 1;
        }
    }
    assert!(
        checked_dark > 0 && checked_light > 0,
        "need themes of both kinds to prove the branch runs"
    );
}

#[test]
fn darkening_wash_never_goes_solid() {
    // The whole point: the wash adds a sliver of opacity where painting a
    // pre-darkened copy of the background would add a full second layer.
    for theme in Theme::ALL {
        let wash = darkening_wash(theme, TAB_ROW_DARKEN);
        assert!(
            wash.a <= WASH_ALPHA_CEILING,
            "{theme}: wash alpha {} exceeds the ceiling",
            wash.a
        );
    }
}

#[test]
fn darkening_wash_paints_nothing_on_a_background_it_cannot_darken() {
    let theme = Theme::custom(
        "black".to_string(),
        iced::theme::Palette {
            background: Color::BLACK,
            ..Theme::Dark.palette()
        },
    );
    assert_eq!(darkening_wash(&theme, TAB_ROW_DARKEN).a, 0.0);
}

#[test]
fn premultiply_scales_encoded_rgb_by_alpha() {
    // White premultiplied is the alpha itself, per channel - directly on
    // the encoded values, with no linear round-trip to brighten them.
    let color = premultiply(Color {
        a: 0.25,
        ..Color::WHITE
    });
    assert_eq!(color.r, 0.25);
    assert_eq!(color.g, 0.25);
    assert_eq!(color.b, 0.25);
    assert_eq!(color.a, 0.25, "alpha must survive untouched");
}

#[test]
fn premultiply_leaves_black_black() {
    // Why dark themes never showed the bug: rgb ~ 0 premultiplies to itself.
    let color = premultiply(Color {
        a: 0.25,
        ..Color::BLACK
    });
    assert_eq!(color.r, 0.0);
    assert_eq!(color.g, 0.0);
    assert_eq!(color.b, 0.0);
    assert!((color.a - 0.25).abs() < 1e-6);
}

#[test]
fn a_light_theme_at_low_alpha_stays_see_through_once_premultiplied() {
    // The reported symptom, as arithmetic. A compositor doing
    // `src_rgb + (1 - a) * desktop` over a *straight* white background
    // computes `1.0 + anything` on every channel - saturated, opaque, and
    // completely insensitive to alpha, which is why a light theme looked
    // solid even at 0.1 while a dark one looked fine. Premultiplied, the
    // desktop keeps its full `1 - a` share of the result.
    let alpha = 0.1;
    let straight = Color {
        a: alpha,
        ..Color::WHITE
    };
    let premultiplied = premultiply(straight);

    let over_desktop =
        |src: Color, desktop: f32| src.r + (1.0 - src.a) * desktop;

    // Straight: black desktop and white desktop composite identically.
    assert_eq!(
        over_desktop(straight, 0.0),
        over_desktop(straight, 1.0) - 0.9
    );
    assert!(
        over_desktop(straight, 0.0) >= 1.0,
        "already saturated before the desktop is added"
    );

    // Premultiplied: the window contributes its 0.1 and the desktop the rest.
    assert!((over_desktop(premultiplied, 0.0) - 0.1).abs() < 1e-6);
    assert!((over_desktop(premultiplied, 1.0) - 1.0).abs() < 1e-6);
}

#[test]
fn a_translucent_window_hands_the_compositor_whatever_the_gate_asks_for() {
    // The two binaries have to agree on screen, so `style` is the single
    // place the premultiply decision is made - not a per-platform branch
    // that can drift. Asserted against the gate rather than a literal so
    // this test says the same thing on every host it runs on.
    let mut app = test_app(1);
    app.background_alpha = 0.5;
    let theme = app.theme();

    let scaled = iced::theme::default(&theme)
        .background_color
        .scale_alpha(0.5);
    let expected = if CLEAR_COLOR_NEEDS_PREMULTIPLY {
        premultiply(scaled)
    } else {
        scaled
    };

    assert_eq!(app.style(&theme).background_color, expected);
}

#[test]
fn premultiplying_leaves_the_configured_alpha_alone() {
    // Whatever the RGB does, the alpha channel *is* the transparency the
    // user configured - premultiplying must not eat into it.
    let mut app = test_app(1);
    app.background_alpha = 0.5;
    assert!(
        (app.style(&app.theme()).background_color.a - 0.5).abs() < 1e-6
    );
}

#[test]
fn an_opaque_window_is_never_premultiplied() {
    // At alpha 1.0 the window isn't transparent at all and premultiplying
    // would be a no-op anyway - but the branch is skipped outright, so a
    // solid window renders identically on both backends.
    let app = test_app(1);
    let theme = app.theme();
    assert_eq!(
        app.style(&theme).background_color,
        iced::theme::default(&theme).background_color
    );
}

/// `switch_active` treats an unmoved index as a no-op, but the first tab
/// at startup - and the stand-in for a closed last tab - lands on one with
/// a brand-new editor under it.
#[test]
fn creating_a_tab_at_the_already_active_index_still_lands_on_it() {
    let mut app = test_app(0);
    let _ = app.new_tab();

    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.active, 0);
}

#[test]
fn closing_the_last_tab_replaces_it() {
    let mut app = test_app(1);

    let _ = app.close_tab(0);

    assert_eq!(app.tabs.len(), 1, "replaced, never left empty");
}

#[test]
fn the_active_tab_paints_no_background_of_its_own() {
    // It has to read as one surface with the editor below it, and the
    // window background already paints that surface.
    let theme = Theme::Dark;
    assert!(tab_frame_style(&theme, true).background.is_none());
    assert!(tab_frame_style(&theme, false).background.is_some());
}
