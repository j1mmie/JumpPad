use super::*;

/// Replays a delta the way `TextArea` does, so a test can state its
/// expectation as the document it wants back.
fn apply(text: &str, delta: &TextDelta) -> String {
    let mut applied = text.to_owned();
    applied.replace_range(delta.source_range(), delta.replacement());
    applied
}

#[test]
fn identical_documents_have_no_delta() {
    assert_eq!(TextDelta::between("a\nb\nc", "a\nb\nc"), None);
}

#[test]
fn a_delta_covers_only_the_characters_that_changed() {
    let delta = TextDelta::between("a\nb\nc", "a\nB\nc")
        .expect("the middle line changed");
    assert_eq!(delta.source_range(), 2..3);
    assert_eq!(delta.replacement(), "B");
    assert_eq!(delta.first_line(), 1);
}

#[test]
fn typing_a_word_stores_the_word_and_not_the_line() {
    // The reason this is character-precise: with word-sized undo steps,
    // line-rounding made each step carry the whole line again.
    let delta =
        TextDelta::between("Hello ", "Hello my ").expect("a word arrived");
    assert_eq!(delta.replacement(), "my ");
    assert_eq!(delta.source_range(), 6..6);
}

#[test]
fn applying_a_delta_produces_the_document_it_was_taken_from() {
    for (before, after) in [
        ("a\nb\nc", "a\nB\nc"),         // replace a middle line
        ("a\nb\nc", "a\nb\nc\nd"),      // append past the end
        ("a\nb\nc", "a\nc"),            // delete a middle line
        ("a\nb\nc", "a\nb"),            // delete the last line
        ("a\nb\nc", "X\nb\nc"),         // change the first line
        ("a\nb\nc", "a\nb\nC"),         // change the last line
        ("a\nb\nc", "a\nb1\nb2\nc"),    // one line becomes two
        ("a\nb\nc", ""),                // clear the document
        ("", "a\nb\nc"),                // fill an empty one
        ("a\r\nb\r\nc", "a\r\nB\r\nc"), // CRLF stays CRLF
        ("a\nb\nc", "a\nb\nc\n"),       // gain a trailing blank line
        ("a\nx", "ax"),                 // two lines join into one
        ("ax", "a\nx"),                 // and split apart again
    ] {
        let delta = TextDelta::between(before, after)
            .unwrap_or_else(|| panic!("{before:?} -> {after:?} changed"));
        assert_eq!(apply(before, &delta), after, "{before:?} -> {after:?}");
    }
}

#[test]
fn an_inverted_delta_walks_the_change_back() {
    for (before, after) in [
        ("a\nb\nc", "a\nB\nc"),
        ("a\nb\nc", "a\nb\nc\nd"),
        ("a\nb\nc", "a\nc"),
        ("a\nb\nc", "a\nb"),
        ("a\nb\nc", ""),
        ("", "a\nb\nc"),
        ("a\r\nb\r\nc", "a\r\nB\r\nc"),
        ("a\nx", "ax"),
    ] {
        let delta = TextDelta::between(before, after)
            .unwrap_or_else(|| panic!("{before:?} -> {after:?} changed"));
        let back = delta.inverted();
        assert_eq!(apply(after, &back), before, "{before:?} -> {after:?}");
        assert_eq!(back.inverted(), delta, "{before:?} -> {after:?}");
    }
}

#[test]
fn a_deletion_takes_only_the_characters_it_removes() {
    let delta = TextDelta::between("a\nb\nc", "a\nc").expect("a line went");
    assert_eq!(delta.source_range(), 2..4);
    assert_eq!(delta.replacement(), "");
}

#[test]
fn an_insertion_past_the_last_line_carries_its_ending() {
    let delta =
        TextDelta::between("a\nb", "a\nb\nc").expect("a line arrived");
    assert_eq!(delta.source_range(), 3..3);
    assert_eq!(delta.replacement(), "\nc");
}

#[test]
fn multibyte_edits_land_on_character_boundaries() {
    // Both ends of each of these meets mid-codepoint.
    for (before, after, replacement) in [
        ("héllo", "hello", "e"),
        ("hello", "héllo", "é"),
        ("héllo", "hÖllo", "Ö"),
        ("wörld", "wörld!", "!"),
        ("é", "ö", "ö"),
    ] {
        let delta = TextDelta::between(before, after)
            .unwrap_or_else(|| panic!("{before:?} -> {after:?} changed"));
        assert_eq!(delta.replacement(), replacement, "{before:?}");
        assert_eq!(apply(before, &delta), after, "{before:?}");
        assert_eq!(
            apply(after, &delta.inverted()),
            before,
            "{before:?} inverted"
        );
    }
}

#[test]
fn deleting_an_ending_takes_the_ending_and_nothing_else() {
    let delta = TextDelta::between("a\nx", "ax").expect("an ending went");
    assert_eq!(delta.source_range(), 1..2);
    assert_eq!(delta.replacement(), "");
    assert_eq!(apply("a\nx", &delta), "ax");
}

#[test]
fn line_count_counts_the_wider_side() {
    let grew = TextDelta::between("a\nb\nc", "a\nb1\nb2\nb3\nc")
        .expect("one line became three");
    assert_eq!(grew.line_count(), 3);
    assert_eq!(grew.inverted().line_count(), 3);
}
