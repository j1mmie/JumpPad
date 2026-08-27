use iced::keyboard::{self, key};
use syntax_registry::GrammarLookup;

use super::*;

fn press(
    modifiers: keyboard::Modifiers,
    physical: key::Code,
    key: keyboard::Key,
) -> KeyPress {
    press_with_text(modifiers, physical, key, None)
}

fn press_with_text(
    modifiers: keyboard::Modifiers,
    physical: key::Code,
    key: keyboard::Key,
    text: Option<&str>,
) -> KeyPress {
    KeyPress {
        key: key.clone(),
        modified_key: key,
        physical_key: key::Physical::Code(physical),
        modifiers,
        text: text.map(Into::into),
        status: Status::Focused { is_hovered: false },
    }
}

/// An editor with no highlighting and default alpha, for cursor/selection tests.
fn plain_editor(text: &str) -> TextArea {
    let registry = SyntaxRegistry::new(Vec::new(), GrammarLookup::default(), || {});
    TextArea::new(
        text,
        &registry,
        None,
        SharedEditorConfig::new(1.0, Arc::new(|_: &KeyPress| None)),
    )
}

/// The invariant the `source` cache exists to maintain: it holds exactly
/// what rebuilding from `Content` would have produced. Everything below
/// that mutates an editor ends by checking this - a cache that drifts
/// feeds the highlighter stale text, which misaligns every span after
/// the point where it diverged.
fn assert_source_is_synced(editor: &TextArea) {
    assert_eq!(
        editor.source.as_str(),
        editor.content.text(),
        "the cached source drifted from the document"
    );
}

#[test]
fn a_new_editor_starts_with_a_synced_source() {
    // Covers the line-ending shapes where seeding the cache from the
    // input `&str` rather than from `Content` could have diverged.
    for doc in
        ["", "one line", "trailing\n", "a\nb\nc", "crlf\r\nlines\r\n"]
    {
        let editor = plain_editor(doc);
        assert_source_is_synced(&editor);
        assert_eq!(editor.text(), editor.content.text(), "doc: {doc:?}");
    }
}

#[test]
fn redraws_reuse_the_cached_source_instead_of_rebuilding_it() {
    // `view` runs per redraw; rebuilding the document string there is
    // the cost this cache removes, so consecutive builds must hand out
    // the same allocation and compare equal (no highlighter re-run).
    let editor = plain_editor("fn main() {}\n");
    let first = editor.highlighter_settings();
    let second = editor.highlighter_settings();
    assert!(
        Arc::ptr_eq(&first.source, &second.source),
        "a redraw must not rebuild the document string"
    );
    assert!(
        first == second,
        "unchanged settings must not re-run the highlighter"
    );
}

#[test]
fn a_selection_drag_does_not_invalidate_the_cached_source() {
    // The regression this cache exists for: dragging a selection emits a
    // stream of non-edit actions, each one causing a redraw. None of
    // them change the text, so none may rebuild the source.
    let mut editor = plain_editor("hello world\nsecond line");
    let before = editor.source.clone();

    for action in [
        text_editor::Action::Click(iced::Point::new(4.0, 2.0)),
        text_editor::Action::Drag(iced::Point::new(30.0, 2.0)),
        text_editor::Action::Drag(iced::Point::new(60.0, 14.0)),
        text_editor::Action::Move(Motion::Right),
        text_editor::Action::Select(Motion::Down),
        text_editor::Action::SelectWord,
        text_editor::Action::SelectLine,
        text_editor::Action::SelectAll,
        text_editor::Action::Scroll { lines: 3 },
    ] {
        let edited = editor.update(EditorMessage::Action(action.clone()));
        assert!(!edited, "{action:?} is not an edit");
        assert!(
            Arc::ptr_eq(&before, &editor.source),
            "{action:?} must not rebuild the cached source"
        );
    }
    assert_source_is_synced(&editor);
}

#[test]
fn a_pixel_scroll_is_not_an_edit_and_does_not_rebuild_the_source() {
    // Same contract as the non-edit actions above: `Scroll` moves the
    // view, never the text, so the highlighter must not be re-run for it.
    // It arrives on its own message rather than as a `text_editor::Action`
    // because `Action::Scroll` can't carry a fractional distance.
    let mut editor = plain_editor("hello world\nsecond line");
    let before = editor.source.clone();

    let edited = editor.update(EditorMessage::Scroll(7.5));

    assert!(!edited, "a scroll is not an edit");
    assert!(
        Arc::ptr_eq(&before, &editor.source),
        "a scroll must not rebuild the cached source"
    );
    assert_source_is_synced(&editor);
}

#[test]
fn an_edit_rebuilds_the_cached_source() {
    let mut editor = plain_editor("hello");
    editor.move_cursor_to(0, 5);
    let before = editor.source.clone();

    let edited = editor.update(EditorMessage::Action(
        text_editor::Action::Edit(text_editor::Edit::Insert('!')),
    ));

    assert!(edited);
    assert!(
        !Arc::ptr_eq(&before, &editor.source),
        "an edit must mint a new source so the highlighter re-runs"
    );
    assert_eq!(editor.source.as_str(), "hello!");
    assert_source_is_synced(&editor);
}

#[test]
fn set_text_rebuilds_the_cached_source() {
    let mut editor = plain_editor("original");
    editor.set_text("replaced\nwith more");
    assert_eq!(editor.text(), "replaced\nwith more");
    assert_source_is_synced(&editor);
}

#[test]
fn undo_and_redo_keep_the_cached_source_in_sync() {
    let mut editor = plain_editor("hello");
    editor.move_cursor_to(0, 5);
    editor.update(EditorMessage::Action(text_editor::Action::Edit(
        text_editor::Edit::Insert('!'),
    )));
    assert_eq!(editor.text(), "hello!");

    // Undo restores the pre-edit text, which `update` read out of the
    // cache - so a stale cache would record the wrong snapshot here.
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "hello");
    assert_source_is_synced(&editor);

    assert!(editor.update(EditorMessage::Redo));
    assert_eq!(editor.text(), "hello!");
    assert_source_is_synced(&editor);
}

/// Types `text` a character at a time, the way a person would.
fn type_out(editor: &mut TextArea, text: &str) {
    for character in text.chars() {
        let action = if character == '\n' {
            text_editor::Edit::Enter
        } else {
            text_editor::Edit::Insert(character)
        };
        editor.update(edit(action));
    }
}

#[test]
fn each_typed_word_is_its_own_undo_step() {
    let mut editor = plain_editor("");
    type_out(&mut editor, "Hello my name is Jimmie");

    // The space rides with the word it follows.
    for expected in [
        "Hello my name is ",
        "Hello my name ",
        "Hello my ",
        "Hello ",
        "",
    ] {
        assert!(editor.update(EditorMessage::Undo), "{expected:?}");
        assert_eq!(editor.text(), expected);
        assert_source_is_synced(&editor);
    }
    assert!(!editor.update(EditorMessage::Undo));
}

#[test]
fn redo_walks_the_words_back_in() {
    let mut editor = plain_editor("");
    type_out(&mut editor, "one two three");
    while editor.update(EditorMessage::Undo) {}
    assert_eq!(editor.text(), "");

    for expected in ["one ", "one two ", "one two three"] {
        assert!(editor.update(EditorMessage::Redo), "{expected:?}");
        assert_eq!(editor.text(), expected);
        assert_source_is_synced(&editor);
    }
}

#[test]
fn enter_ends_an_undo_step() {
    let mut editor = plain_editor("");
    type_out(&mut editor, "first\nsecond");
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "first\n");
    assert_source_is_synced(&editor);
}

#[test]
fn moving_the_caret_ends_an_undo_step() {
    // Typing here, clicking there, then typing again is two edits, not
    // one - without this they fold into a single step.
    let mut editor = plain_editor("ab");
    editor.move_cursor_to(0, 2);
    editor.update(edit(text_editor::Edit::Insert('X')));
    editor.update(EditorMessage::Action(text_editor::Action::Move(
        text_editor::Motion::Home,
    )));
    editor.update(edit(text_editor::Edit::Insert('Y')));
    assert_eq!(editor.text(), "YabX");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "abX");
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "ab");
}

#[test]
fn a_run_of_deletes_still_coalesces_on_the_timer() {
    // Backspace has no word boundary to break on, so the timer is what
    // keeps a held key from becoming one enormous step.
    let mut editor = plain_editor("abcdef");
    editor.move_cursor_to(0, 6);
    for _ in 0..3 {
        editor.update(edit(text_editor::Edit::Backspace));
    }
    assert_eq!(editor.text(), "abc");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "abcdef");
    assert!(!editor.update(EditorMessage::Undo));
}

#[test]
fn a_typed_word_stores_only_the_word() {
    // With word-sized steps, a delta that rounded out to whole lines
    // would carry the growing line on every keystroke.
    let mut editor = plain_editor("Hello ");
    editor.move_cursor_to(0, 6);
    type_out(&mut editor, "my ");

    let (delta, _) = editor
        .history
        .undo(&editor.source, editor.cursor_state())
        .expect("a word to undo");
    assert_eq!(delta.replacement(), "");
    assert_eq!(delta.source_range(), 6..9);
}

#[test]
fn undo_and_redo_round_trip_byte_exactly() {
    // Every shape where endings are load-bearing: the last line, which
    // carries none, and CRLF, which is two bytes.
    let cases: &[(&str, (usize, usize), text_editor::Edit)] = &[
        ("a\nb\nc", (1, 1), text_editor::Edit::Insert('X')),
        ("a\nb\nc", (0, 0), text_editor::Edit::Insert('X')),
        ("a\nb\nc", (2, 1), text_editor::Edit::Insert('X')),
        ("a\nb\nc", (1, 1), text_editor::Edit::Enter),
        ("a\nb\nc", (2, 1), text_editor::Edit::Enter),
        ("a\nb\nc", (2, 1), text_editor::Edit::Backspace),
        ("a\nb\nc", (2, 0), text_editor::Edit::Backspace),
        ("a\r\nb\r\nc", (1, 1), text_editor::Edit::Insert('X')),
        ("a\r\nb\r\nc", (2, 1), text_editor::Edit::Enter),
        ("a\r\nb\r\nc", (1, 0), text_editor::Edit::Backspace),
        ("a\r\nb\r\nc", (2, 1), text_editor::Edit::Backspace),
        ("", (0, 0), text_editor::Edit::Insert('X')),
        ("trailing\n", (1, 0), text_editor::Edit::Insert('X')),
        ("one line", (0, 3), text_editor::Edit::Insert('X')),
    ];

    for (doc, cursor, action) in cases {
        let what = format!("{doc:?} at {cursor:?} {action:?}");
        let mut editor = plain_editor(doc);
        editor.move_cursor_to(cursor.0, cursor.1);
        assert!(editor.update(edit(action.clone())), "no edit: {what}");
        let edited = editor.text();
        assert_source_is_synced(&editor);

        assert!(editor.update(EditorMessage::Undo), "no undo: {what}");
        assert_eq!(editor.text(), *doc, "undo: {what}");
        assert_source_is_synced(&editor);

        assert!(editor.update(EditorMessage::Redo), "no redo: {what}");
        assert_eq!(editor.text(), edited, "redo: {what}");
        assert_source_is_synced(&editor);
    }
}

#[test]
fn undo_carries_only_the_lines_the_edit_touched() {
    // The whole point: an undo costs the lines it changes.
    let doc: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let mut editor = plain_editor(&doc);
    editor.move_cursor_to(30, 4);
    editor.update(edit(text_editor::Edit::Insert('X')));
    assert!(editor.update(EditorMessage::Undo));

    assert_eq!(editor.text(), doc);
    assert_source_is_synced(&editor);
    // The highlighter resumes at the change, not at the top.
    assert_eq!(editor.highlighter_settings().edited_from, Some(30));
}

#[test]
fn an_edit_reports_the_topmost_line_it_touched() {
    // Recoloring starts at the selection's anchor, not the caret.
    let mut editor = plain_editor("a\nb\nc\nd\ne");
    editor.restore_selection(
        SavedSelection {
            anchor: (1, 0),
            kind: SelectionKind::Range,
        },
        (3, 1),
    );
    editor.update(edit(text_editor::Edit::Insert('X')));
    assert_eq!(editor.highlighter_settings().edited_from, Some(1));
}

#[test]
fn a_whole_document_undo_gives_way_to_a_rebuild() {
    // Past `LINES_WORTH_SPLICING` the splice stands down, but the text
    // still has to come back.
    let long: String = (0..TextArea::LINES_WORTH_SPLICING + 10)
        .map(|i| format!("line {i}\n"))
        .collect();
    let mut editor = plain_editor("short");
    editor.reload_text(&long);
    assert_eq!(editor.text(), long);

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "short");
    assert_source_is_synced(&editor);
    assert_eq!(editor.highlighter_settings().edited_from, None);

    assert!(editor.update(EditorMessage::Redo));
    assert_eq!(editor.text(), long);
    assert_source_is_synced(&editor);
}

#[test]
fn a_comment_toggle_undoes_byte_exactly() {
    // Toggle-comment splices in place, so endings survive it too.
    for doc in ["a\nb\nc", "a\r\nb\r\nc", "last"] {
        let mut editor = rust_editor(doc);
        editor.move_cursor_to(0, 0);
        assert!(editor.update(EditorMessage::ToggleComment), "{doc:?}");
        let commented = editor.text();
        assert!(commented.contains("// "), "{doc:?}");
        assert_source_is_synced(&editor);

        assert!(editor.update(EditorMessage::Undo), "{doc:?}");
        assert_eq!(editor.text(), doc, "{doc:?}");
        assert_source_is_synced(&editor);

        assert!(editor.update(EditorMessage::Redo), "{doc:?}");
        assert_eq!(editor.text(), commented, "{doc:?}");
        assert_source_is_synced(&editor);
    }
}

#[test]
fn the_undo_depth_follows_the_shared_setting() {
    let registry = SyntaxRegistry::new(Vec::new(), GrammarLookup::default(), || {});
    let settings =
        SharedEditorConfig::new(1.0, Arc::new(|_: &KeyPress| None));
    settings.set_undo_depth(1);
    let mut editor = TextArea::new("a", &registry, None, settings.clone());

    // Two isolated steps, of which only the most recent may survive.
    editor.move_cursor_to(0, 1);
    editor.update(EditorMessage::CopyLineDown);
    editor.update(EditorMessage::CopyLineDown);
    let before_undo = editor.text();

    assert!(editor.update(EditorMessage::Undo));
    assert_ne!(editor.text(), before_undo);
    assert!(!editor.update(EditorMessage::Undo), "depth of 1 keeps one");
}

/// The measurement behind `LINES_WORTH_SPLICING`. Run by hand:
/// `cargo test --release -- --ignored --nocapture`. Sees buffer work
/// only, not shaping, so every rebuild number it prints is a floor.
#[test]
#[ignore = "a measurement, not an assertion"]
fn what_an_undo_costs_on_a_long_document() {
    use std::time::Instant;

    const LINES: usize = 20_000;
    let doc: String = (0..LINES).map(|i| format!("line {i}\n")).collect();

    let mut editor = plain_editor(&doc);
    editor.move_cursor_to(LINES / 2, 4);
    editor.update(edit(text_editor::Edit::Insert('X')));
    let started = Instant::now();
    assert!(editor.update(EditorMessage::Undo));
    println!("undo of one line in {LINES}: {:?}", started.elapsed());

    let mut editor = plain_editor(&doc);
    let started = Instant::now();
    editor.replace_document(&doc);
    println!("rebuild of {LINES} (unshaped): {:?}", started.elapsed());

    for reach in [1, 10, 100, 500, 1_000, 2_000, 5_000] {
        let replacement: String =
            (0..reach).map(|i| format!("other {i}\n")).collect();
        let mut editor = plain_editor(&doc);
        let started = Instant::now();
        editor.paste_over((0, 0), (reach, 0), replacement);
        println!(
            "splice of {reach} lines into {LINES}: {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn reload_text_replaces_the_document_and_is_undoable() {
    let mut editor = plain_editor("on disk");
    editor.reload_text("changed underneath");
    assert_eq!(editor.text(), "changed underneath");

    // An external reload is a change like any other, so Ctrl+Z brings
    // the pre-reload text back.
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "on disk");
}

#[test]
fn reload_text_keeps_the_cached_source_in_sync() {
    let mut editor = plain_editor("one\ntwo");
    editor.reload_text("one\ntwo\nthree");
    assert_source_is_synced(&editor);

    editor.update(EditorMessage::Undo);
    assert_source_is_synced(&editor);
}

#[test]
fn reloading_identical_text_records_nothing() {
    let mut editor = plain_editor("unchanged");
    editor.move_cursor_to(0, 4);
    editor.update(edit(text_editor::Edit::Insert('!')));
    editor.update(EditorMessage::Undo);
    assert_eq!(editor.text(), "unchanged");

    // A stamp that moved without the bytes moving must not cost the
    // redo the user just set up.
    editor.reload_text("unchanged");

    assert!(editor.update(EditorMessage::Redo), "the redo survived");
    assert_eq!(editor.text(), "unch!anged");
}

#[test]
fn reload_text_clamps_a_caret_past_the_end_of_a_shortened_file() {
    let mut editor = plain_editor("first line\nsecond line\nthird line");
    editor.move_cursor_to(2, 10);

    editor.reload_text("first line");

    assert_eq!(editor.cursor_position(), (0, 10));
}

/// Selects `hello` in "hello world" as a plain drag-style range, cursor
/// at the far end - the starting point for the undo tests below.
fn editor_with_hello_selected() -> TextArea {
    let mut editor = plain_editor("hello world");
    editor.restore_selection(
        SavedSelection {
            anchor: (0, 0),
            kind: SelectionKind::Range,
        },
        (0, 5),
    );
    assert_eq!(editor.content.selection().as_deref(), Some("hello"));
    editor
}

fn edit(edit: text_editor::Edit) -> EditorMessage {
    EditorMessage::Action(text_editor::Action::Edit(edit))
}

/// The message an action arrives as - the route the word-boundary ones
/// have to take, since `Content::perform` never sees them.
fn action(action: text_editor::Action) -> EditorMessage {
    EditorMessage::Action(action)
}

fn editor_with_style(
    text: &str,
    extension: &str,
    style: CommentStyle,
) -> TextArea {
    let registry = SyntaxRegistry::new(Vec::new(), GrammarLookup::default(), || {});
    let settings =
        SharedEditorConfig::new(1.0, Arc::new(|_: &KeyPress| None));
    settings.set_comment_styles([(extension.to_string(), style)].into());
    TextArea::new(text, &registry, Some(extension), settings)
}

/// An editor opened as a `.rs` file with `// ` configured.
fn rust_editor(text: &str) -> TextArea {
    editor_with_style(text, "rs", CommentStyle::Single("// ".to_string()))
}

/// An editor opened as an `.html` file with `<!--` / `-->` configured.
fn html_editor(text: &str) -> TextArea {
    editor_with_style(
        text,
        "html",
        CommentStyle::Multi {
            left: "<!--".to_string(),
            right: "-->".to_string(),
        },
    )
}

/// An editor whose `[indentation]` is set the way a config would set it.
fn indented_editor(
    text: &str,
    style: IndentationStyle,
    width: u16,
) -> TextArea {
    let registry = SyntaxRegistry::new(Vec::new(), GrammarLookup::default(), || {});
    let settings =
        SharedEditorConfig::new(1.0, Arc::new(|_: &KeyPress| None));
    settings.set_indentation(Indentation::new(style, width));
    TextArea::new(text, &registry, None, settings)
}

#[test]
fn the_tabs_style_indents_with_one_tab_character() {
    let mut editor = indented_editor("", IndentationStyle::Tabs, 4);
    assert!(editor.update(EditorMessage::Indent));
    assert_eq!(editor.text(), "\t");
    assert_eq!(editor.cursor_position(), (0, 1));
    assert_source_is_synced(&editor);
}

#[test]
fn the_tabs_style_inserts_one_character_however_wide_it_draws() {
    // The width is a drawing instruction here, not a count of anything
    // that reaches the document.
    for width in [2, 8] {
        let mut editor = indented_editor("", IndentationStyle::Tabs, width);
        editor.update(EditorMessage::Indent);
        assert_eq!(editor.text(), "\t", "at width {width}");
    }
}

#[test]
fn the_spaces_style_indents_to_the_next_stop() {
    let mut editor = indented_editor("ab", IndentationStyle::Spaces, 4);
    editor.move_cursor_to(0, 2);
    assert!(editor.update(EditorMessage::Indent));
    assert_eq!(editor.text(), "ab  ", "two spaces reach column 4");
    assert_eq!(editor.cursor_position(), (0, 4));
    assert_source_is_synced(&editor);
}

#[test]
fn the_spaces_style_counts_from_the_caret_not_the_line_start() {
    let mut editor =
        indented_editor("abcdefghijkl", IndentationStyle::Spaces, 8);
    // The user-facing spec example: caret at column 12, next stop at 16.
    editor.move_cursor_to(0, 12);
    editor.update(EditorMessage::Indent);
    assert_eq!(editor.text(), "abcdefghijkl    ");
}

#[test]
fn the_spaces_style_measures_a_tabbed_line_as_it_is_drawn() {
    // The line already holds a tab, which covers four columns of its own
    // - so the caret after it is on a stop and gets a whole width.
    let mut editor = indented_editor("\t", IndentationStyle::Spaces, 4);
    editor.move_cursor_to(0, 1);
    editor.update(EditorMessage::Indent);
    assert_eq!(editor.text(), "\t    ");
}

#[test]
fn an_indent_replaces_the_selection_it_lands_on() {
    let mut editor =
        indented_editor("keep drop", IndentationStyle::Tabs, 4);
    select_range(&mut editor, (0, 5), (0, 9));
    assert!(editor.update(EditorMessage::Indent));
    assert_eq!(editor.text(), "keep \t");
    assert_source_is_synced(&editor);
}

#[test]
fn an_indent_over_a_selection_counts_from_where_it_starts() {
    // Not from the caret at the far end: the spaces land where the
    // selection did, so they have to reach the stop from there.
    let mut editor = indented_editor("abc", IndentationStyle::Spaces, 4);
    // Dragged right to left, so the caret is the *earlier* end here and
    // the anchor the later one - the indent has to take the smaller.
    select_range(&mut editor, (0, 3), (0, 1));
    editor.update(EditorMessage::Indent);
    assert_eq!(editor.text(), "a   ", "three spaces from column 1");
}

#[test]
fn an_indent_rides_with_the_word_before_it_and_closes_the_step() {
    // The rule every other whitespace follows (see `ends_undo_step`):
    // the indent goes back with the word it followed, and what is typed
    // after it is a step of its own. Both styles, since only one of them
    // gets that for free from `Edit::Insert`.
    for (style, indented) in [
        (IndentationStyle::Tabs, "ab\t"),
        (IndentationStyle::Spaces, "ab  "),
    ] {
        let mut editor = indented_editor("", style, 4);
        type_out(&mut editor, "ab");
        editor.update(EditorMessage::Indent);
        assert_eq!(editor.text(), indented, "{style:?}");
        type_out(&mut editor, "c");

        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), indented, "{style:?}");
        assert!(editor.update(EditorMessage::Undo));
        assert_eq!(editor.text(), "", "{style:?}");
        assert_source_is_synced(&editor);
    }
}

#[test]
fn typing_after_an_indent_is_its_own_undo_step() {
    let mut editor = indented_editor("", IndentationStyle::Tabs, 4);
    editor.update(EditorMessage::Indent);
    type_out(&mut editor, "x");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "\t");
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "");
}

#[test]
fn tab_indents_every_line_a_selection_spans_and_keeps_it() {
    let mut editor = indented_editor("aaa\nbbb", IndentationStyle::Tabs, 4);
    select_range(&mut editor, (0, 1), (1, 2));

    assert!(editor.update(EditorMessage::Indent));
    assert_eq!(editor.text(), "\taaa\n\tbbb");
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (0, 2),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (1, 3));
    assert_source_is_synced(&editor);
}

#[test]
fn a_selection_ending_at_column_zero_is_not_indented_on_that_line() {
    // The rule every line command already follows: a selection whose
    // bottom edge sits at column 0 merely starts that line.
    let mut editor =
        indented_editor("aaa\nbbb\nccc", IndentationStyle::Tabs, 4);
    select_range(&mut editor, (0, 0), (2, 0));

    assert!(editor.update(EditorMessage::Indent));
    assert_eq!(editor.text(), "\taaa\n\tbbb\nccc");
    assert_source_is_synced(&editor);
}

#[test]
fn a_selection_reaching_the_next_line_indents_rather_than_replacing() {
    // Replacing it would join the two lines, which is nobody's idea of
    // what Tab does - even though only the first line is covered.
    let mut editor = indented_editor("aaa\nbbb", IndentationStyle::Tabs, 4);
    select_range(&mut editor, (0, 1), (1, 0));

    assert!(editor.update(EditorMessage::Indent));
    assert_eq!(editor.text(), "\taaa\nbbb");
}

#[test]
fn a_triple_clicked_line_is_indented_rather_than_replaced() {
    let mut editor = indented_editor("aaa\nbbb", IndentationStyle::Tabs, 4);
    editor.move_cursor_to(0, 1);
    editor.content.perform(text_editor::Action::SelectLine);

    assert!(editor.update(EditorMessage::Indent));
    assert_eq!(editor.text(), "\taaa\nbbb");
}

#[test]
fn a_block_indent_reaches_each_lines_own_next_stop() {
    let mut editor =
        indented_editor("aaa\n  bbb", IndentationStyle::Spaces, 4);
    select_range(&mut editor, (0, 0), (1, 5));

    editor.update(EditorMessage::Indent);
    assert_eq!(editor.text(), "    aaa\n    bbb");
}

#[test]
fn shift_tab_outdents_every_line_a_selection_spans_and_keeps_it() {
    let mut editor =
        indented_editor("\taaa\n\tbbb", IndentationStyle::Tabs, 4);
    select_range(&mut editor, (0, 1), (1, 2));

    assert!(editor.update(EditorMessage::Outdent));
    assert_eq!(editor.text(), "aaa\nbbb");
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (0, 0),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (1, 1));
    assert_source_is_synced(&editor);
}

#[test]
fn shift_tab_outdents_the_caret_line_with_nothing_selected() {
    let mut editor = indented_editor("\taaa", IndentationStyle::Tabs, 4);
    editor.move_cursor_to(0, 2);

    assert!(editor.update(EditorMessage::Outdent));
    assert_eq!(editor.text(), "aaa");
    assert_eq!(editor.cursor_position(), (0, 1));
    assert_source_is_synced(&editor);
}

#[test]
fn a_block_already_at_the_margin_has_no_outdent_to_make() {
    let mut editor = indented_editor("aaa\nbbb", IndentationStyle::Tabs, 4);
    select_range(&mut editor, (0, 0), (1, 3));

    assert!(!editor.update(EditorMessage::Outdent));
    assert_eq!(editor.text(), "aaa\nbbb");
}

#[test]
fn each_block_indent_is_its_own_undo_step() {
    let mut editor = indented_editor("aaa\nbbb", IndentationStyle::Tabs, 4);
    select_range(&mut editor, (0, 0), (1, 3));

    editor.update(EditorMessage::Indent);
    editor.update(EditorMessage::Indent);
    assert_eq!(editor.text(), "\t\taaa\n\t\tbbb");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "\taaa\n\tbbb");
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "aaa\nbbb");
    assert_source_is_synced(&editor);
}

#[test]
fn a_blank_line_inside_an_indented_block_is_left_alone() {
    let mut editor =
        indented_editor("aaa\n\nbbb", IndentationStyle::Tabs, 4);
    select_range(&mut editor, (0, 0), (2, 3));

    assert!(editor.update(EditorMessage::Indent));
    assert_eq!(editor.text(), "\taaa\n\n\tbbb");
    assert_source_is_synced(&editor);
}

#[test]
fn toggle_comment_round_trips_text_and_cursor() {
    let mut editor = rust_editor("    let x = 1;");
    editor.move_cursor_to(0, 8);

    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "    // let x = 1;");
    assert_eq!(editor.cursor_position(), (0, 11));
    assert_source_is_synced(&editor);

    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "    let x = 1;");
    assert_eq!(editor.cursor_position(), (0, 8));
    assert_source_is_synced(&editor);
}

#[test]
fn toggle_comment_covers_a_multi_line_selection_and_keeps_it() {
    let mut editor = rust_editor("aaa\nbbb");
    editor.restore_selection(
        SavedSelection {
            anchor: (0, 1),
            kind: SelectionKind::Range,
        },
        (1, 2),
    );

    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "// aaa\n// bbb");
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (0, 4),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (1, 5));
    assert_source_is_synced(&editor);
}

#[test]
fn a_selection_ending_at_column_zero_leaves_that_line_alone() {
    let mut editor = rust_editor("aaa\nbbb\nccc");
    editor.restore_selection(
        SavedSelection {
            anchor: (0, 0),
            kind: SelectionKind::Range,
        },
        (2, 0),
    );
    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "// aaa\n// bbb\nccc");
}

#[test]
fn toggle_without_a_configured_style_is_a_clean_no_op() {
    let mut editor = plain_editor("text");
    assert!(!editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "text");
    assert!(
        !editor.update(EditorMessage::Undo),
        "no phantom history entry"
    );
}

#[test]
fn a_toggle_between_keystrokes_stays_its_own_undo_step() {
    // All three edits land inside one coalesce window; the toggle must
    // not fold into either typing burst.
    let mut editor = rust_editor("fn main() {}");
    let _ = editor.update(edit(text_editor::Edit::Insert('a')));
    assert!(editor.update(EditorMessage::ToggleComment));
    let _ = editor.update(edit(text_editor::Edit::Insert('b')));
    assert_eq!(editor.text(), "// abfn main() {}");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "// afn main() {}");
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "afn main() {}");
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "fn main() {}");
    assert_source_is_synced(&editor);
}

#[test]
fn undo_of_a_selection_toggle_restores_the_selection() {
    let mut editor = rust_editor("aaa\nbbb");
    editor.restore_selection(
        SavedSelection {
            anchor: (0, 1),
            kind: SelectionKind::Range,
        },
        (1, 2),
    );
    assert!(editor.update(EditorMessage::ToggleComment));

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "aaa\nbbb");
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (0, 1),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (1, 2));
}

#[test]
fn crlf_line_endings_survive_a_toggle_round_trip() {
    let mut editor = rust_editor("aaa\r\nbbb");
    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "// aaa\r\nbbb");
    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "aaa\r\nbbb");
    assert_source_is_synced(&editor);
}

/// The user-facing acceptance example for multi-line styles: wrap two
/// list items, keeping the same characters selected.
#[test]
fn multi_toggle_keeps_the_selected_characters() {
    let text = "    <ul>\n\
                \x20       <li>Do you have an Internet connection? </li>\n\
                \x20       <li>Is anti-virus software or a firewall preventing ROBLOX from accessing the Internet?</li>     \n\
                \x20   </ul>";
    let mut editor = html_editor(text);
    editor.restore_selection(
        SavedSelection {
            anchor: (1, 24),
            kind: SelectionKind::Range,
        },
        (2, 25),
    );
    let selected_before = editor.content.selection();

    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(
        editor.text(),
        "    <ul>\n\
         \x20       <!--<li>Do you have an Internet connection? </li>\n\
         \x20       <li>Is anti-virus software or a firewall preventing ROBLOX from accessing the Internet?</li>     -->\n\
         \x20   </ul>"
    );
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (1, 28),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (2, 25));
    assert_eq!(
        editor.content.selection(),
        selected_before,
        "same characters selected"
    );
    assert_source_is_synced(&editor);

    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), text);
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (1, 24),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (2, 25));
    assert_source_is_synced(&editor);
}

#[test]
fn multi_toggle_on_a_caret_line_round_trips() {
    let mut editor = html_editor("    foo");
    editor.move_cursor_to(0, 7);
    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "    <!--foo-->");
    assert_eq!(editor.cursor_position(), (0, 11), "caret stays before -->");

    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "    foo");
    assert_eq!(editor.cursor_position(), (0, 7));
    assert_source_is_synced(&editor);
}

#[test]
fn undo_of_a_multi_toggle_restores_text_and_selection() {
    let mut editor = html_editor("aaa\nbbb");
    editor.restore_selection(
        SavedSelection {
            anchor: (0, 1),
            kind: SelectionKind::Range,
        },
        (1, 2),
    );
    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "<!--aaa\nbbb-->");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "aaa\nbbb");
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (0, 1),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (1, 2));
}

#[test]
fn crlf_survives_a_multi_toggle_round_trip() {
    let mut editor = html_editor("aaa\r\nbbb\r\nccc");
    editor.restore_selection(
        SavedSelection {
            anchor: (0, 0),
            kind: SelectionKind::Range,
        },
        (1, 3),
    );
    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "<!--aaa\r\nbbb-->\r\nccc");
    assert!(editor.update(EditorMessage::ToggleComment));
    assert_eq!(editor.text(), "aaa\r\nbbb\r\nccc");
    assert_source_is_synced(&editor);
}

/// Selects `anchor` through `cursor` as a plain range, the shape a
/// shift+arrow or a mouse drag leaves behind.
fn select_range(
    editor: &mut TextArea,
    anchor: (usize, usize),
    cursor: (usize, usize),
) {
    editor.restore_selection(
        SavedSelection {
            anchor,
            kind: SelectionKind::Range,
        },
        cursor,
    );
}

#[test]
fn a_no_op_splice_reproduces_the_document() {
    // The splice helper's contract, over the same line-ending shapes
    // `a_new_editor_starts_with_a_synced_source` covers: swapping lines
    // for copies of themselves has to be byte-identical.
    for doc in
        ["", "one line", "trailing\n", "a\nb\nc", "crlf\r\nlines\r\n"]
    {
        let mut editor = plain_editor(doc);
        let before = editor.text();
        let count = editor.content.line_count();
        let same = editor.covered_text((0, count - 1));
        editor.splice_lines_in_place(0..count, &same);
        editor.resync_source();
        assert_eq!(editor.text(), before, "{doc:?}");
        assert_source_is_synced(&editor);
    }
}

#[test]
fn a_spliced_in_last_line_borrows_the_documents_ending() {
    // The last line carries no ending of its own, so a copy landing past
    // it has nothing to inherit - without the fallback this splices a
    // lone LF into a CRLF document.
    let mut editor = plain_editor("aaa\r\nbbb");
    let doubled = vec!["bbb".to_string(), "bbb".to_string()];
    editor.splice_lines_in_place(1..2, &doubled);
    editor.resync_source();
    assert_eq!(editor.text(), "aaa\r\nbbb\r\nbbb");
    assert_source_is_synced(&editor);
}

#[test]
fn delete_line_removes_the_caret_line_and_keeps_the_column() {
    let mut editor = plain_editor("aaa\nbbb\nccc");
    editor.move_cursor_to(1, 2);
    assert!(editor.update(EditorMessage::DeleteLine));
    assert_eq!(editor.text(), "aaa\nccc");
    assert_eq!(editor.cursor_position(), (1, 2));
    assert_source_is_synced(&editor);
}

#[test]
fn delete_line_at_the_end_clamps_the_caret_into_the_document() {
    let mut editor = plain_editor("aaa\nbbbb");
    editor.move_cursor_to(1, 4);
    assert!(editor.update(EditorMessage::DeleteLine));
    assert_eq!(editor.text(), "aaa");
    assert_eq!(editor.cursor_position(), (0, 3));
    assert_source_is_synced(&editor);
}

#[test]
fn delete_line_covers_a_multi_line_selection_and_collapses_it() {
    let mut editor = plain_editor("aaa\nbbb\nccc\nddd");
    select_range(&mut editor, (1, 1), (2, 2));
    assert!(editor.update(EditorMessage::DeleteLine));
    assert_eq!(editor.text(), "aaa\nddd");
    assert_eq!(editor.selection(), None);
    assert_eq!(editor.cursor_position(), (1, 2));
    assert_source_is_synced(&editor);
}

#[test]
fn delete_line_can_empty_the_document() {
    let mut editor = plain_editor("only");
    assert!(editor.update(EditorMessage::DeleteLine));
    assert_eq!(editor.text(), "");
    assert_eq!(editor.cursor_position(), (0, 0));
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "only");
    assert_source_is_synced(&editor);
}

#[test]
fn delete_line_on_an_empty_document_records_nothing() {
    let mut editor = plain_editor("");
    assert!(!editor.update(EditorMessage::DeleteLine));
    assert!(
        !editor.update(EditorMessage::Undo),
        "no phantom history entry"
    );
}

#[test]
fn crlf_survives_a_delete_line() {
    let mut editor = plain_editor("aaa\r\nbbb\r\nccc");
    editor.move_cursor_to(1, 0);
    assert!(editor.update(EditorMessage::DeleteLine));
    assert_eq!(editor.text(), "aaa\r\nccc");
    assert_source_is_synced(&editor);
}

#[test]
fn move_line_up_swaps_with_the_line_above_and_carries_the_caret() {
    let mut editor = plain_editor("aaa\nbbb\nccc");
    editor.move_cursor_to(1, 2);
    assert!(editor.update(EditorMessage::MoveLineUp));
    assert_eq!(editor.text(), "bbb\naaa\nccc");
    assert_eq!(editor.cursor_position(), (0, 2));
    assert_source_is_synced(&editor);
}

#[test]
fn move_line_down_swaps_with_the_line_below_and_carries_the_caret() {
    let mut editor = plain_editor("aaa\nbbb\nccc");
    editor.move_cursor_to(1, 2);
    assert!(editor.update(EditorMessage::MoveLineDown));
    assert_eq!(editor.text(), "aaa\nccc\nbbb");
    assert_eq!(editor.cursor_position(), (2, 2));
    assert_source_is_synced(&editor);
}

#[test]
fn move_line_up_at_the_top_is_a_clean_no_op() {
    let mut editor = plain_editor("aaa\nbbb");
    editor.move_cursor_to(0, 1);
    assert!(!editor.update(EditorMessage::MoveLineUp));
    assert_eq!(editor.text(), "aaa\nbbb");
    assert!(
        !editor.update(EditorMessage::Undo),
        "no phantom history entry"
    );
}

#[test]
fn move_line_down_at_the_bottom_is_a_clean_no_op() {
    let mut editor = plain_editor("aaa\nbbb");
    editor.move_cursor_to(1, 1);
    assert!(!editor.update(EditorMessage::MoveLineDown));
    assert_eq!(editor.text(), "aaa\nbbb");
    assert!(
        !editor.update(EditorMessage::Undo),
        "no phantom history entry"
    );
}

#[test]
fn an_edge_no_op_leaves_the_redo_stack_alone() {
    // `record_isolated` clears redo unconditionally, so a no-op that
    // recorded anyway would silently throw away a redo.
    let mut editor = plain_editor("aaa\nbbb");
    editor.move_cursor_to(0, 1);
    assert!(editor.update(EditorMessage::DeleteLine));
    assert!(editor.update(EditorMessage::Undo));
    assert!(!editor.update(EditorMessage::MoveLineUp));
    assert!(editor.update(EditorMessage::Redo));
    assert_eq!(editor.text(), "bbb");
}

#[test]
fn move_line_up_keeps_a_multi_line_selection_on_the_moved_block() {
    let mut editor = plain_editor("aaa\nbbb\nccc\nddd");
    select_range(&mut editor, (1, 1), (2, 2));
    assert!(editor.update(EditorMessage::MoveLineUp));
    assert_eq!(editor.text(), "bbb\nccc\naaa\nddd");
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (0, 1),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (1, 2));
    assert_eq!(editor.content.selection().as_deref(), Some("bb\ncc"));
    assert_source_is_synced(&editor);
}

#[test]
fn moving_the_last_line_up_keeps_the_crlf_endings() {
    // The last line carries no ending; moving it up promotes it to a
    // separator position, where it has to borrow the document's.
    let mut editor = plain_editor("aaa\r\nbbb");
    editor.move_cursor_to(1, 0);
    assert!(editor.update(EditorMessage::MoveLineUp));
    assert_eq!(editor.text(), "bbb\r\naaa");
    assert_source_is_synced(&editor);
}

#[test]
fn move_line_down_swaps_with_a_trailing_empty_line() {
    // "a\nb\n" is three lines, the last one empty - it swaps like any other.
    let mut editor = plain_editor("aaa\nbbb\n");
    editor.move_cursor_to(1, 0);
    assert!(editor.update(EditorMessage::MoveLineDown));
    assert_eq!(editor.text(), "aaa\n\nbbb");
    assert_source_is_synced(&editor);
}

#[test]
fn move_line_up_then_down_round_trips() {
    let mut editor = plain_editor("aaa\nbbb\nccc");
    editor.move_cursor_to(1, 1);
    assert!(editor.update(EditorMessage::MoveLineUp));
    assert!(editor.update(EditorMessage::MoveLineDown));
    assert_eq!(editor.text(), "aaa\nbbb\nccc");
    assert_eq!(editor.cursor_position(), (1, 1));
    assert_source_is_synced(&editor);
}

#[test]
fn undo_of_a_move_restores_the_text_and_the_selection() {
    let mut editor = plain_editor("aaa\nbbb\nccc");
    select_range(&mut editor, (1, 0), (1, 3));
    assert!(editor.update(EditorMessage::MoveLineDown));
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "aaa\nbbb\nccc");
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (1, 0),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (1, 3));
    assert_source_is_synced(&editor);
}

#[test]
fn copy_line_down_duplicates_the_line_and_lands_on_the_copy() {
    let mut editor = plain_editor("aaa\nbbb");
    editor.move_cursor_to(0, 2);
    assert!(editor.update(EditorMessage::CopyLineDown));
    assert_eq!(editor.text(), "aaa\naaa\nbbb");
    assert_eq!(editor.cursor_position(), (1, 2));
    assert_source_is_synced(&editor);
}

#[test]
fn copy_line_up_writes_the_same_text_but_stays_on_the_upper_copy() {
    let mut editor = plain_editor("aaa\nbbb");
    editor.move_cursor_to(0, 2);
    assert!(editor.update(EditorMessage::CopyLineUp));
    assert_eq!(editor.text(), "aaa\naaa\nbbb");
    assert_eq!(editor.cursor_position(), (0, 2));
    assert_source_is_synced(&editor);
}

#[test]
fn copy_line_down_shifts_a_selection_by_the_block_height() {
    let mut editor = plain_editor("aaa\nbbb\nccc");
    select_range(&mut editor, (0, 1), (1, 2));
    assert!(editor.update(EditorMessage::CopyLineDown));
    assert_eq!(editor.text(), "aaa\nbbb\naaa\nbbb\nccc");
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (2, 1),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (3, 2));
    assert_source_is_synced(&editor);
}

#[test]
fn duplicating_the_last_line_keeps_the_crlf_endings() {
    let mut editor = plain_editor("aaa\r\nbbb");
    editor.move_cursor_to(1, 0);
    assert!(editor.update(EditorMessage::CopyLineDown));
    assert_eq!(editor.text(), "aaa\r\nbbb\r\nbbb");
    assert_source_is_synced(&editor);
}

#[test]
fn copy_line_down_on_a_single_line_document() {
    let mut editor = plain_editor("only");
    assert!(editor.update(EditorMessage::CopyLineDown));
    assert_eq!(editor.text(), "only\nonly");
    assert_eq!(editor.cursor_position(), (1, 0));
    assert_source_is_synced(&editor);
}

#[test]
fn a_line_command_stays_its_own_undo_step() {
    // Same rule as toggle-comment: it neither joins the typing burst
    // before it nor absorbs the keystroke after.
    let mut editor = plain_editor("aaa\nbbb");
    editor.move_cursor_to(0, 3);
    assert!(editor.update(edit(text_editor::Edit::Insert('x'))));
    assert!(editor.update(EditorMessage::MoveLineDown));
    assert!(editor.update(edit(text_editor::Edit::Insert('y'))));
    assert_eq!(editor.text(), "bbb\naaaxy");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "bbb\naaax");
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "aaax\nbbb");
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "aaa\nbbb");
    assert_source_is_synced(&editor);
}

#[test]
fn undo_of_a_cut_reselects_the_cut_text() {
    // The reported bug. A cut publishes exactly one `Edit::Delete`, so
    // this is the whole cut path as the widget produces it.
    let mut editor = editor_with_hello_selected();
    assert!(editor.update(edit(text_editor::Edit::Delete)));
    assert_eq!(editor.text(), " world");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "hello world");
    assert_eq!(editor.content.selection().as_deref(), Some("hello"));
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (0, 0),
            kind: SelectionKind::Range
        })
    );
    assert_eq!(editor.cursor_position(), (0, 5));
}

#[test]
fn undo_of_a_paste_over_a_selection_reselects_the_replaced_text() {
    let mut editor = editor_with_hello_selected();
    assert!(editor.update(edit(text_editor::Edit::Paste(Arc::new(
        "bye".to_string()
    )))));
    assert_eq!(editor.text(), "bye world");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "hello world");
    assert_eq!(editor.content.selection().as_deref(), Some("hello"));
}

#[test]
fn undo_of_typing_over_a_selection_reselects_the_replaced_text() {
    // Matches VS Code, which restores `beforeCursorState` uniformly for
    // every edit - typing included, not just cut and paste.
    let mut editor = editor_with_hello_selected();
    assert!(editor.update(edit(text_editor::Edit::Insert('X'))));
    assert_eq!(editor.text(), "X world");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "hello world");
    assert_eq!(editor.content.selection().as_deref(), Some("hello"));
}

#[test]
fn redo_after_undoing_a_cut_leaves_no_selection() {
    let mut editor = editor_with_hello_selected();
    editor.update(edit(text_editor::Edit::Delete));
    editor.update(EditorMessage::Undo);

    assert!(editor.update(EditorMessage::Redo));
    assert_eq!(editor.text(), " world");
    assert_eq!(editor.selection(), None);
}

#[test]
fn undo_restores_a_double_clicked_word_still_selected() {
    // Typing over a double-clicked word and undoing brings the word
    // back selected, the same as undoing over any other selection.
    let mut editor = plain_editor("hello world");
    editor.move_cursor_to(0, 8); // inside "world"
    editor.update(action(text_editor::Action::SelectWord));
    assert_eq!(editor.content.selection().as_deref(), Some("world"));

    editor.update(edit(text_editor::Edit::Insert('X')));
    assert_eq!(editor.text(), "hello X");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "hello world");
    assert_eq!(editor.content.selection().as_deref(), Some("world"));
    assert_eq!(
        editor.selection(),
        Some(SavedSelection {
            anchor: (0, 6),
            kind: SelectionKind::Range,
        })
    );
}

#[test]
fn undo_restores_a_line_selection_as_a_line_selection() {
    let mut editor = plain_editor("first line\nsecond line");
    editor.move_cursor_to(1, 3);
    editor.content.perform(text_editor::Action::SelectLine);

    editor.update(edit(text_editor::Edit::Insert('X')));
    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "first line\nsecond line");
    assert_eq!(editor.content.selection().as_deref(), Some("second line"));
    assert_eq!(
        editor.selection().map(|s| s.kind),
        Some(SelectionKind::Line)
    );
}

#[test]
fn undo_of_plain_typing_leaves_a_collapsed_caret() {
    // Nothing was selected before the edit, so nothing may be selected
    // after the undo - in particular not a degenerate zero-width range.
    let mut editor = plain_editor("hello");
    editor.move_cursor_to(0, 5);
    editor.update(edit(text_editor::Edit::Insert('!')));

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "hello");
    assert_eq!(editor.selection(), None);
    assert_eq!(editor.cursor_position(), (0, 5));
}

#[test]
fn undo_of_a_word_delete_reselects_the_deleted_word() {
    // `word_delete_backward` is a `Binding::Sequence`, and a sequence
    // publishes each element as its own message - so the `Select` lands
    // first and the `Backspace` records a caret that already has the word
    // selected. Undo therefore brings it back selected. VS Code collapses
    // the caret here instead; accepted, since it reads the same as undoing
    // a cut and the alternative is an atomic word-delete command.
    let mut editor = plain_editor("hello world");
    editor.move_cursor_to(0, 11);
    assert!(!editor.update(EditorMessage::Action(
        text_editor::Action::Select(Motion::Left.widen())
    )));
    assert!(editor.update(edit(text_editor::Edit::Backspace)));
    assert_eq!(editor.text(), "hello ");

    assert!(editor.update(EditorMessage::Undo));
    assert_eq!(editor.text(), "hello world");
    assert_eq!(editor.content.selection().as_deref(), Some("world"));
}

#[test]
fn a_selection_restoring_undo_keeps_the_cached_source_in_sync() {
    // Restoring a selection replays `SelectWord`/`SelectLine`/`move_to`,
    // none of which are edits - so the cache must be resynced exactly
    // once, by the content swap, and not again by the restore.
    let mut editor = editor_with_hello_selected();
    editor.update(edit(text_editor::Edit::Delete));
    let after_edit = editor.source.clone();

    editor.update(EditorMessage::Undo);
    assert_source_is_synced(&editor);
    assert!(
        !Arc::ptr_eq(&after_edit, &editor.source),
        "undo changed the text"
    );
}

#[test]
fn settings_over_separately_allocated_equal_sources_compare_unequal() {
    // Documents the deliberate trade-off in `HighlighterSettings::eq`:
    // it compares source pointers, not bytes. Two identical but
    // separately allocated strings therefore compare unequal, costing
    // one redundant reparse. That direction is harmless; the reverse -
    // missing a real change - would leave stale colors on screen, and
    // only `resync_source` ever mints a new pointer.
    let settings = |source: &str| HighlighterSettings {
        source: Arc::new(source.to_string()),
        grammar: None,
        revision: 0,
        matches: Arc::new(Vec::new()),
        current_match: None,
        foreground_alpha: 1.0,
        edited_from: None,
    };
    assert!(settings("ab") != settings("ab"));
}

#[test]
fn range_selection_round_trips_through_save_and_restore() {
    let mut editor = plain_editor("hello\nworld");
    let saved = SavedSelection {
        anchor: (0, 2),
        kind: SelectionKind::Range,
    };
    editor.restore_selection(saved, (1, 4));
    assert_eq!(editor.selection(), Some(saved));
    assert_eq!(editor.cursor_position(), (1, 4));
}

#[test]
fn word_selection_round_trips_through_save_and_restore() {
    let mut editor = plain_editor("hello world");
    // A double click: Click places the cursor, SelectWord selects around
    // it - and saves as an ordinary range, since the word's bounds are
    // measured here rather than left implied for the buffer to redo.
    editor.move_cursor_to(0, 8); // inside "world"
    editor.update(action(text_editor::Action::SelectWord));
    assert_eq!(editor.content.selection().as_deref(), Some("world"));

    let saved = editor.selection().expect("word selection should save");
    assert_eq!(
        saved,
        SavedSelection {
            anchor: (0, 6),
            kind: SelectionKind::Range
        }
    );
    let cursor = editor.cursor_position();
    assert_eq!(cursor, (0, 11));

    // Simulate the tab going away and coming back: clear, then restore.
    editor.move_cursor_to(0, 0);
    assert_eq!(editor.selection(), None);
    editor.restore_selection(saved, cursor);
    assert_eq!(editor.content.selection().as_deref(), Some("world"));
    assert_eq!(editor.selection(), Some(saved));
}

#[test]
fn line_selection_round_trips_through_save_and_restore() {
    let mut editor = plain_editor("first line\nsecond line");
    editor.move_cursor_to(1, 3);
    editor.content.perform(text_editor::Action::SelectLine);
    let saved = editor.selection().expect("line selection should save");
    assert_eq!(saved.kind, SelectionKind::Line);

    editor.move_cursor_to(0, 0);
    editor.restore_selection(saved, (1, 3));
    assert_eq!(editor.content.selection().as_deref(), Some("second line"));
}

/// One press of the word-left / word-right motion, as `Binding::Move`
/// builds it from Ctrl+Left / Ctrl+Right.
fn word_motion(motion: Motion) -> EditorMessage {
    action(text_editor::Action::Move(motion.widen()))
}

fn word_selection(motion: Motion) -> EditorMessage {
    action(text_editor::Action::Select(motion.widen()))
}

#[test]
fn a_double_click_takes_the_word_the_caret_is_in() {
    let mut editor = plain_editor("alpha beta gamma");
    editor.move_cursor_to(0, 8); // inside "beta"
    editor.update(action(text_editor::Action::SelectWord));

    assert_eq!(editor.content.selection().as_deref(), Some("beta"));
    assert_eq!(editor.cursor_position(), (0, 10));
}

#[test]
fn a_double_click_takes_the_run_it_lands_in_when_that_is_no_word() {
    // Punctuation is taken as its own run, and whitespace as its own -
    // which is what keeps a double click on an operator off the words
    // either side of it.
    let mut editor = plain_editor("value ==  1");
    editor.move_cursor_to(0, 7); // between the two `=`
    editor.update(action(text_editor::Action::SelectWord));
    assert_eq!(editor.content.selection().as_deref(), Some("=="));

    editor.move_cursor_to(0, 9); // between the two spaces
    editor.update(action(text_editor::Action::SelectWord));
    assert_eq!(editor.content.selection().as_deref(), Some("  "));

    // A caret with a word on one side of it takes the word, whichever
    // side that is - which is why only a gap of two or more can be
    // double clicked into at all.
    editor.move_cursor_to(0, 10);
    editor.update(action(text_editor::Action::SelectWord));
    assert_eq!(editor.content.selection().as_deref(), Some("1"));
}

#[test]
fn a_double_click_stops_where_the_configured_separators_say() {
    // The whole point of the setting: `-` is on the default list, so a
    // hyphenated name is two words - and a config that drops it makes
    // the same text one.
    let mut editor = plain_editor("font-size");
    editor.move_cursor_to(0, 2);
    editor.update(action(text_editor::Action::SelectWord));
    assert_eq!(editor.content.selection().as_deref(), Some("font"));

    editor
        .settings
        .set_word_separators(WordSeparators::new(":"));
    editor.move_cursor_to(0, 2);
    editor.update(action(text_editor::Action::SelectWord));
    assert_eq!(editor.content.selection().as_deref(), Some("font-size"));
}

#[test]
fn the_word_motions_land_on_the_far_side_of_each_word() {
    // Word-right ends on the word ahead, word-left starts on the word
    // behind - so a round trip crosses the same words rather than
    // sticking on the boundary between two of them.
    let mut editor = plain_editor("alpha beta");
    editor.move_cursor_to(0, 0);

    editor.update(word_motion(Motion::Right));
    assert_eq!(editor.cursor_position(), (0, 5));
    editor.update(word_motion(Motion::Right));
    assert_eq!(editor.cursor_position(), (0, 10));
    editor.update(word_motion(Motion::Left));
    assert_eq!(editor.cursor_position(), (0, 6));
    editor.update(word_motion(Motion::Left));
    assert_eq!(editor.cursor_position(), (0, 0));
    // Already at the end of the document: nowhere left to go.
    editor.update(word_motion(Motion::Left));
    assert_eq!(editor.cursor_position(), (0, 0));
}

#[test]
fn a_word_motion_stops_at_a_run_of_separators() {
    let mut editor = plain_editor("foo(bar)");
    editor.move_cursor_to(0, 0);

    for column in [3, 4, 7, 8] {
        editor.update(word_motion(Motion::Right));
        assert_eq!(editor.cursor_position(), (0, column));
    }
}

#[test]
fn a_word_motion_honours_the_configured_separators() {
    let mut editor = plain_editor("font-size: 12");
    editor
        .settings
        .set_word_separators(WordSeparators::new(":"));
    editor.move_cursor_to(0, 0);

    editor.update(word_motion(Motion::Right));
    assert_eq!(editor.cursor_position(), (0, 9), "past \"font-size\"");
}

#[test]
fn a_word_motion_crosses_a_line_once_this_one_has_run_out() {
    let mut editor = plain_editor(
        "one
two",
    );
    editor.move_cursor_to(0, 3);

    editor.update(word_motion(Motion::Right));
    assert_eq!(editor.cursor_position(), (1, 0));
    editor.update(word_motion(Motion::Left));
    assert_eq!(editor.cursor_position(), (0, 3));
}

#[test]
fn a_word_motion_over_a_selection_only_collapses_it() {
    // iced's rule for every motion, and the word ones keep it: the caret
    // lands on the edge it was heading for and goes no further.
    let mut editor = plain_editor("alpha beta gamma");
    editor.restore_selection(
        SavedSelection {
            anchor: (0, 6),
            kind: SelectionKind::Range,
        },
        (0, 10),
    );

    editor.update(word_motion(Motion::Left));
    assert_eq!(editor.cursor_position(), (0, 6));
    assert_eq!(editor.selection(), None);
}

#[test]
fn shift_and_a_word_motion_extend_the_selection() {
    let mut editor = plain_editor("alpha beta");
    editor.move_cursor_to(0, 0);

    editor.update(word_selection(Motion::Right));
    assert_eq!(editor.content.selection().as_deref(), Some("alpha"));
    editor.update(word_selection(Motion::Right));
    assert_eq!(editor.content.selection().as_deref(), Some("alpha beta"));
    // ...and back onto the anchor, which leaves nothing selected rather
    // than an empty range.
    editor.update(word_selection(Motion::Left));
    editor.update(word_selection(Motion::Left));
    assert_eq!(editor.selection(), None);
    assert_eq!(editor.cursor_position(), (0, 0));
}

#[test]
fn a_word_delete_takes_the_word_the_separators_name() {
    // `word_delete_backward` is a `Select` then a `Backspace`, so the
    // separators decide what it deletes.
    let delete_word_left = |separators: &str| {
        let mut editor = plain_editor("font-size");
        editor
            .settings
            .set_word_separators(WordSeparators::new(separators));
        editor.move_cursor_to(0, 9);
        editor.update(word_selection(Motion::Left));
        editor.update(edit(text_editor::Edit::Backspace));
        assert_source_is_synced(&editor);
        editor.text()
    };

    assert_eq!(delete_word_left(word::DEFAULT_SEPARATORS), "font-");
    assert_eq!(delete_word_left(""), "");
}

#[test]
fn a_drag_out_of_a_double_clicked_word_takes_whole_words() {
    let mut editor = plain_editor("alpha beta gamma");
    editor.move_cursor_to(0, 2); // inside "alpha"
    editor.update(action(text_editor::Action::SelectWord));
    assert_eq!(editor.content.selection().as_deref(), Some("alpha"));

    // A drag's own hit test needs a laid-out buffer to answer; what it
    // produces is a caret, and these are the carets it would produce -
    // part-way into the word on either side of the one it started in.
    let drag_to = |editor: &mut TextArea, column| {
        editor.content.move_to(Cursor {
            position: Position { line: 0, column },
            selection: None,
        });
        editor.extend_word_selection();
    };

    drag_to(&mut editor, 13);
    assert_eq!(
        editor.content.selection().as_deref(),
        Some("alpha beta gamma")
    );
    drag_to(&mut editor, 1);
    assert_eq!(editor.content.selection().as_deref(), Some("alpha"));
}

#[test]
fn a_word_motion_leaves_up_and_down_aiming_at_the_new_column() {
    // cosmic-text remembers the column an Up or Down is aiming for until
    // a sideways motion clears it, and a word motion that only sets the
    // cursor would leave it standing - so Down after word-left would go
    // back to the column word-left had just left.
    let mut editor = plain_editor("alpha beta\nalpha beta");
    editor.move_cursor_to(0, 8);
    editor.update(action(text_editor::Action::Move(Motion::Down)));
    assert_eq!(editor.cursor_position(), (1, 8), "the column is now aimed");
    editor.update(action(text_editor::Action::Move(Motion::Up)));

    editor.update(word_motion(Motion::Left));
    assert_eq!(editor.cursor_position(), (0, 6));
    editor.update(action(text_editor::Action::Move(Motion::Down)));
    assert_eq!(editor.cursor_position(), (1, 6));
}

#[test]
fn a_word_drag_lasts_until_something_else_moves_the_caret() {
    let mut editor = plain_editor("alpha beta");
    editor.move_cursor_to(0, 2);
    editor.update(action(text_editor::Action::SelectWord));
    assert!(editor.word_drag_from.is_some());

    editor.update(action(text_editor::Action::Click(iced::Point::ORIGIN)));
    assert!(editor.word_drag_from.is_none(), "a click ends the drag");
}

#[test]
fn selection_is_none_without_a_selection() {
    let mut editor = plain_editor("hello");
    editor.move_cursor_to(0, 3);
    assert_eq!(editor.selection(), None);
    assert_eq!(editor.cursor_position(), (0, 3));
}

#[test]
fn move_cursor_to_clears_a_leftover_selection() {
    let mut editor = plain_editor("hello world");
    editor.restore_selection(
        SavedSelection {
            anchor: (0, 0),
            kind: SelectionKind::Range,
        },
        (0, 5),
    );
    assert!(editor.selection().is_some());
    editor.move_cursor_to(0, 2);
    assert_eq!(editor.selection(), None);
    assert_eq!(editor.cursor_position(), (0, 2));
}

#[test]
fn clamp_position_clamps_line_and_column_to_the_document() {
    let content = Content::with_text("hello\nhi");
    assert_eq!(
        clamp_position(&content, (9, 9)),
        Position { line: 1, column: 2 }
    );
    assert_eq!(
        clamp_position(&content, (0, 3)),
        Position { line: 0, column: 3 }
    );
}

#[test]
fn clamp_position_backs_up_to_a_char_boundary() {
    // 'é' occupies bytes 1..3, so byte offset 2 is mid-character.
    let content = Content::with_text("héllo");
    assert_eq!(
        clamp_position(&content, (0, 2)),
        Position { line: 0, column: 1 }
    );
}

#[test]
fn binding_for_covers_every_editor_action() {
    use jumppad_actions::Category;

    for action in Action::in_category(Category::Editor) {
        assert!(
            binding_for(action).is_some(),
            "{action} is an editor action with no binding"
        );
    }
    // And nothing else: an app action must fall through to the shell
    // rather than be swallowed here.
    for action in Action::in_category(Category::App) {
        assert!(binding_for(action).is_none(), "{action} is not ours");
    }

    assert!(matches!(
        binding_for(Action::WordDeleteBackward),
        Some(Binding::Sequence(_))
    ));
    assert!(matches!(
        binding_for(Action::DocumentStart),
        Some(Binding::Move(Motion::DocumentStart))
    ));
    assert!(matches!(
        binding_for(Action::SelectDocumentEnd),
        Some(Binding::Select(Motion::DocumentEnd))
    ));
    assert!(matches!(
        binding_for(Action::MoveLineUp),
        Some(Binding::Custom(EditorMessage::MoveLineUp))
    ));
}

/// Stands in for the resolver the app supplies. Which *chords* map to
/// which actions is `jumppad_keybinds`' business and is tested there;
/// what this crate owns is turning a resolved action into a binding, so
/// these hand the answer over directly.
fn resolving(
    action: Option<Action>,
) -> impl Fn(&KeyPress) -> Option<Action> {
    move |_| action
}

#[test]
fn a_resolved_editor_action_becomes_its_binding() {
    let event = press(
        keyboard::Modifiers::ALT,
        key::Code::ArrowUp,
        keyboard::Key::Named(key::Named::ArrowUp),
    );
    assert!(matches!(
        key_binding(event, &resolving(Some(Action::MoveLineUp))),
        Some(Binding::Custom(EditorMessage::MoveLineUp))
    ));
}

#[test]
fn a_tab_press_becomes_an_indent_instead_of_going_nowhere() {
    // The bug this fixes: Tab arrives carrying "\t", and iced's own
    // dispatch drops it as a control character - so an unresolved Tab
    // produced no binding, was never captured, and did nothing at all.
    // Resolving it first is what gets it a binding.
    let tab = || {
        press_with_text(
            keyboard::Modifiers::empty(),
            key::Code::Tab,
            keyboard::Key::Named(key::Named::Tab),
            Some("\t"),
        )
    };
    assert!(matches!(
        key_binding(tab(), &resolving(Some(Action::Indent))),
        Some(Binding::Custom(EditorMessage::Indent))
    ));
    assert!(
        key_binding(tab(), &resolving(None)).is_none(),
        "iced's own dispatch still has nothing for Tab"
    );
}

#[test]
fn a_resolved_app_action_falls_through_instead_of_being_swallowed() {
    // `binding_for` returns `None` for an action the shell owns, and the
    // press has to stay unclaimed so the shell still sees it.
    let event = press(
        keyboard::Modifiers::CTRL,
        key::Code::KeyN,
        keyboard::Key::Character("n".into()),
    );
    assert!(matches!(
        key_binding(event, &resolving(Some(Action::NewTab))),
        None | Some(Binding::Unfocus)
    ));
}

#[test]
fn an_unresolved_press_reaches_iceds_own_dispatch() {
    let event = press_with_text(
        keyboard::Modifiers::empty(),
        key::Code::KeyN,
        keyboard::Key::Character("n".into()),
        Some("n"),
    );
    assert!(matches!(
        key_binding(event, &resolving(None)),
        Some(Binding::Insert('n'))
    ));
}

#[test]
fn command_held_unrecognized_character_does_not_fall_through_to_insert() {
    // `Modifiers::CTRL` stands in for `command()`, which resolves per-OS
    // at compile time.
    let event = press_with_text(
        keyboard::Modifiers::CTRL,
        key::Code::KeyN,
        keyboard::Key::Character("n".into()),
        Some("n"),
    );
    assert!(key_binding(event, &resolving(None)).is_none());
}

#[test]
fn unfocused_status_returns_none_even_for_a_resolved_action() {
    let mut event = press(
        keyboard::Modifiers::CTRL,
        key::Code::KeyZ,
        keyboard::Key::Character("z".into()),
    );
    event.status = Status::Active;
    assert!(key_binding(event, &resolving(Some(Action::Redo))).is_none());
}

#[test]
fn apply_alpha_at_full_solid_returns_the_color_unchanged() {
    let color = iced::Color::from_rgba(0.2, 0.4, 0.6, 0.9);
    assert_eq!(apply_alpha(color, 1.0), color);
}

#[test]
fn apply_alpha_scales_the_alpha_channel_only() {
    let color = iced::Color::from_rgba(0.2, 0.4, 0.6, 0.8);
    let scaled = apply_alpha(color, 0.5);
    assert_eq!((scaled.r, scaled.g, scaled.b), (0.2, 0.4, 0.6));
    assert!((scaled.a - 0.4).abs() < f32::EPSILON);
}

#[test]
fn editor_style_leaves_background_and_value_alone_at_full_solid() {
    let theme = Theme::ALL[0].clone();
    let default = text_editor::default(&theme, text_editor::Status::Active);
    let style = editor_style(&theme, text_editor::Status::Active, 1.0, 1.0);
    assert_eq!(style.background, default.background);
    assert_eq!(style.value, default.value);
}

#[test]
fn editor_style_drops_its_background_when_translucent_but_keeps_the_value()
{
    let theme = Theme::ALL[0].clone();
    let default = text_editor::default(&theme, text_editor::Status::Active);
    let style = editor_style(&theme, text_editor::Status::Active, 0.5, 1.0);
    assert_eq!(style.background, Background::Color(Color::TRANSPARENT));
    assert_eq!(style.value, default.value); // foreground untouched

    let style = editor_style(&theme, text_editor::Status::Active, 1.0, 0.3);
    assert_eq!(style.background, default.background); // background untouched
    assert_ne!(style.value, default.value);
}

#[test]
fn an_ordinary_edit_reports_only_the_text_moving() {
    // An empty list arrives after every edit while find is closed;
    // minting a new `Arc` would read as a find change.
    let mut editor = plain_editor("a\nb\nc\nd");
    editor.set_find_matches(Vec::new(), None);
    let before = editor.highlighter_settings();

    editor.move_cursor_to(2, 1);
    editor.update(edit(text_editor::Edit::Insert('X')));
    editor.set_find_matches(Vec::new(), None);
    let after = editor.highlighter_settings();

    assert!(after.only_the_text_moved_since(&before));
    assert_eq!(after.edited_from, Some(2));
}

#[test]
fn an_edit_that_also_moves_the_find_matches_recolors_from_the_top() {
    // Match coloring can land on a line no edit went near.
    let mut editor = plain_editor("a\nb\nc\nd");
    editor.set_find_matches(Vec::new(), None);
    let before = editor.highlighter_settings();

    editor.move_cursor_to(2, 1);
    editor.update(edit(text_editor::Edit::Insert('X')));
    editor.set_find_matches(
        vec![FindMatch {
            line: 2,
            start: 0,
            end: 1,
        }],
        Some(0),
    );
    let after = editor.highlighter_settings();

    assert!(!after.only_the_text_moved_since(&before));
}

#[test]
fn the_highlighter_resumes_at_the_edit_and_rewinds_for_anything_else() {
    let mut editor = plain_editor("a\nb\nc\nd\ne\nf");
    editor.set_find_matches(Vec::new(), None);
    let mut highlighter =
        TreeSitterHighlighter::new(&editor.highlighter_settings());
    assert_eq!(highlighter.current_line(), 0);

    editor.move_cursor_to(4, 1);
    editor.update(edit(text_editor::Edit::Insert('X')));
    editor.set_find_matches(Vec::new(), None);
    highlighter.update(&editor.highlighter_settings());
    assert_eq!(
        highlighter.current_line(),
        4,
        "an edit resumes where it is"
    );

    // iced reports its own topmost changed line afterwards; the lower of
    // the two has to win.
    highlighter.change_line(2);
    assert_eq!(highlighter.current_line(), 2);
    highlighter.change_line(5);
    assert_eq!(highlighter.current_line(), 2, "the lower one stands");

    // A find change is not an edit, so it goes back to the top.
    editor.set_find_matches(
        vec![FindMatch {
            line: 4,
            start: 0,
            end: 1,
        }],
        Some(0),
    );
    highlighter.update(&editor.highlighter_settings());
    assert_eq!(highlighter.current_line(), 0);
}

#[test]
fn set_foreground_alpha_reaches_color_for() {
    // The only test that writes the global, restored at the end so the
    // parallel test threads never see a scaled alpha.
    let settings =
        SharedEditorConfig::new(1.0, Arc::new(|_: &KeyPress| None));
    settings.set_foreground_alpha(0.25);
    let keyword = Highlighted::Syntax(HighlightCategory::Keyword);
    let color = color_for(keyword);
    let base = base_color_for(keyword);
    settings.set_foreground_alpha(1.0);

    assert_eq!((color.r, color.g, color.b), (base.r, base.g, base.b));
    assert!((color.a - base.a * 0.25).abs() < f32::EPSILON);
}

#[test]
fn shared_editor_config_round_trips_its_settings() {
    let settings =
        SharedEditorConfig::new(0.8, Arc::new(|_: &KeyPress| None));
    assert_eq!(settings.background_alpha(), 0.8);
    settings.set_background_alpha(0.4);
    assert_eq!(settings.background_alpha(), 0.4);
    // Out-of-range input clamps rather than propagating.
    settings.set_background_alpha(2.0);
    assert_eq!(settings.background_alpha(), 1.0);

    // The resolver is swappable so a `keybinds.toml` reload reaches tabs
    // that already exist.
    assert_eq!(
        (settings.resolver())(&press(
            keyboard::Modifiers::CTRL,
            key::Code::KeyZ,
            keyboard::Key::Character("z".into()),
        )),
        None
    );
    settings.set_resolver(Arc::new(|_: &KeyPress| Some(Action::Undo)));
    assert_eq!(
        (settings.resolver())(&press(
            keyboard::Modifiers::CTRL,
            key::Code::KeyZ,
            keyboard::Key::Character("z".into()),
        )),
        Some(Action::Undo)
    );
}

/// Builds a highlighter over `source` with `matches` already applied.
fn highlighter_with(
    source: &str,
    matches: Vec<FindMatch>,
    current: Option<usize>,
) -> TreeSitterHighlighter {
    TreeSitterHighlighter::new(&HighlighterSettings {
        source: Arc::new(source.to_string()),
        grammar: None,
        revision: 0,
        matches: Arc::new(matches),
        current_match: current,
        foreground_alpha: 1.0,
        edited_from: None,
    })
}

#[test]
fn highlight_line_emits_a_span_per_match_on_that_line() {
    let source = "find me\nand me";
    let matches = vec![
        FindMatch {
            line: 0,
            start: 5,
            end: 7,
        },
        FindMatch {
            line: 1,
            start: 4,
            end: 6,
        },
    ];
    let mut highlighter = highlighter_with(source, matches, Some(1));

    let first: Vec<_> = highlighter.highlight_line("find me").collect();
    assert_eq!(first, vec![(5..7, Highlighted::Match)]);

    // The second is the current match, so it gets the distinct color.
    let second: Vec<_> = highlighter.highlight_line("and me").collect();
    assert_eq!(second, vec![(4..6, Highlighted::CurrentMatch)]);
}

#[test]
fn match_spans_come_after_syntax_spans_so_they_win_on_overlap() {
    // iced feeds these to `AttrsList::add_span`, where the last span
    // covering a byte wins - so a match must be emitted last to be seen.
    let mut highlighter = highlighter_with(
        "keyword",
        vec![FindMatch {
            line: 0,
            start: 0,
            end: 7,
        }],
        None,
    );
    // Stand in for a grammar by injecting a syntax span directly.
    highlighter.spans = Arc::new(vec![syntax_registry::HighlightSpan {
        start: 0,
        end: 7,
        category: HighlightCategory::Keyword,
    }]);

    let spans: Vec<_> = highlighter.highlight_line("keyword").collect();
    assert_eq!(
        spans,
        vec![
            (0..7, Highlighted::Syntax(HighlightCategory::Keyword)),
            (0..7, Highlighted::Match),
        ],
        "the match span must be last"
    );
}

#[test]
fn settings_differing_only_in_find_state_are_not_equal() {
    // Guards the hand-written `PartialEq`: iced re-runs the highlighter
    // only when settings compare unequal, so dropping the find fields
    // there would leave match coloring frozen on screen.
    let matches = Arc::new(vec![FindMatch {
        line: 0,
        start: 0,
        end: 2,
    }]);
    let base = HighlighterSettings {
        source: Arc::new("ab".to_string()),
        grammar: None,
        revision: 0,
        matches: matches.clone(),
        current_match: None,
        foreground_alpha: 1.0,
        edited_from: None,
    };

    let same = HighlighterSettings { ..base.clone() };
    assert!(base == same);

    let moved_current = HighlighterSettings {
        current_match: Some(0),
        ..base.clone()
    };
    assert!(base != moved_current, "a new current match must invalidate");

    let other_matches = HighlighterSettings {
        matches: Arc::new(vec![FindMatch {
            line: 9,
            start: 1,
            end: 2,
        }]),
        ..base.clone()
    };
    assert!(base != other_matches, "a new match list must invalidate");
}

#[test]
fn settings_differing_only_in_registry_revision_are_not_equal() {
    // The whole mechanism for picking up a late-loading injection
    // target: nothing else about the settings moves when one resolves.
    let base = HighlighterSettings {
        source: Arc::new("ab".to_string()),
        grammar: None,
        revision: 0,
        matches: Arc::new(Vec::new()),
        current_match: None,
        foreground_alpha: 1.0,
        edited_from: None,
    };
    let loaded_something = HighlighterSettings {
        revision: 1,
        ..base.clone()
    };
    assert!(base != loaded_something);
}

#[test]
fn settings_differing_only_in_foreground_alpha_are_not_equal() {
    // How a config reload's new alpha reaches text already on screen:
    // nothing else about the settings moves when only alpha changes.
    let base = HighlighterSettings {
        source: Arc::new("ab".to_string()),
        grammar: None,
        revision: 0,
        matches: Arc::new(Vec::new()),
        current_match: None,
        foreground_alpha: 1.0,
        edited_from: None,
    };
    let faded = HighlighterSettings {
        foreground_alpha: 0.5,
        ..base.clone()
    };
    assert!(base != faded);
}

#[test]
fn set_find_matches_reaches_the_highlighter_settings() {
    let mut editor = plain_editor("find me");
    editor.set_find_matches(
        vec![FindMatch {
            line: 0,
            start: 5,
            end: 7,
        }],
        Some(0),
    );
    assert_eq!(editor.find_matches.len(), 1);
    assert_eq!(editor.find_current, Some(0));

    editor.set_find_matches(Vec::new(), None);
    assert!(editor.find_matches.is_empty());
    assert_eq!(editor.find_current, None);
}
