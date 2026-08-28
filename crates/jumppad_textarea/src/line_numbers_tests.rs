use super::*;

#[test]
fn a_short_document_still_gets_the_narrowest_column() {
    for line_count in [0, 1, 9, 100] {
        assert_eq!(Column::digits_for(line_count), MIN_DIGIT_COUNT);
    }
}

#[test]
fn the_column_widens_with_the_longest_number() {
    assert_eq!(Column::digits_for(1_000), 4);
    assert_eq!(Column::digits_for(9_999), 4);
    assert_eq!(Column::digits_for(10_000), 5);
}

#[test]
fn the_column_leaves_one_blank_digit_beside_the_text() {
    // Three digits, ten pixels each.
    let column = Column::new(3, 30.0);

    assert_eq!(column.width(), 40.0);
}

#[test]
fn numbers_of_different_lengths_end_flush_with_each_other() {
    // Three digits, ten pixels each, starting at five.
    let column = Column::new(3, 30.0);

    assert_eq!(column.number_left_edge(3, 5.0), 5.0);
    assert_eq!(column.number_left_edge(2, 5.0), 15.0);
    assert_eq!(column.number_left_edge(1, 5.0), 25.0);
}

#[test]
fn the_text_gets_what_the_column_leaves() {
    let column = Column::new(3, 30.0);

    assert_eq!(column.text_width(200.0), 160.0);
    // A strip narrower than the column itself leaves nothing, rather than a
    // negative width for cosmic-text to wrap against.
    assert_eq!(column.text_width(10.0), 0.0);
}

#[test]
fn a_column_too_wide_for_its_window_leaves_no_room() {
    let column = Column::new(3, 30.0);

    // 40 for the column, 80 for eight characters of text.
    assert!(column.leaves_room_in(120.0));
    assert!(!column.leaves_room_in(119.0));
}
