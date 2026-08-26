use super::*;
use iced::Point;

#[test]
fn shift_click_becomes_a_selection_extending_drag() {
    let click = EditorMessage::Action(text_editor::Action::Click(
        Point::new(3.0, 7.0),
    ));
    assert!(matches!(
        click.with_shift_click(true),
        EditorMessage::Action(text_editor::Action::Drag(position))
            if position == Point::new(3.0, 7.0)
    ));
}

#[test]
fn plain_click_stays_a_click() {
    let click =
        EditorMessage::Action(text_editor::Action::Click(Point::ORIGIN));
    assert!(matches!(
        click.with_shift_click(false),
        EditorMessage::Action(text_editor::Action::Click(_))
    ));
}

#[test]
fn shift_double_click_word_selection_is_untouched() {
    let double = EditorMessage::Action(text_editor::Action::SelectWord);
    assert!(matches!(
        double.with_shift_click(true),
        EditorMessage::Action(text_editor::Action::SelectWord)
    ));
}

#[test]
fn non_action_messages_pass_through() {
    assert!(matches!(
        EditorMessage::Undo.with_shift_click(true),
        EditorMessage::Undo
    ));
}
