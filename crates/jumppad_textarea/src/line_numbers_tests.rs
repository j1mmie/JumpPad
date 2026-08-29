use super::*;

/// Three digits ten pixels each, so a character is a round ten pixels and a
/// padding in characters can be read against the width it comes out as.
fn three_digits(padding: Padding) -> Column {
    Column::new(3, 30.0, padding)
}

#[test]
fn the_column_counts_the_digits_the_longest_number_needs() {
    assert_eq!(Column::digits_for(0), 1);
    assert_eq!(Column::digits_for(9), 1);
    assert_eq!(Column::digits_for(10), 2);
    assert_eq!(Column::digits_for(999), 3);
    assert_eq!(Column::digits_for(1_000), 4);
    assert_eq!(Column::digits_for(10_000), 5);
}

#[test]
fn the_padding_sits_either_side_of_the_numbers() {
    // Three digits, with one character left and two right.
    let column = three_digits(Padding::new(1.0, 2.0));

    assert_eq!(column.width(), 60.0);
}

#[test]
fn a_column_with_no_padding_is_exactly_its_digits() {
    let column = three_digits(Padding::new(0.0, 0.0));

    assert_eq!(column.width(), 30.0);
}

/// The left padding moves the numbers along with it; the right padding moves
/// only the text. That is the whole reason they are two settings.
#[test]
fn the_left_padding_moves_the_numbers_and_the_right_one_does_not() {
    let left = three_digits(Padding::new(2.0, 1.0));
    let right = three_digits(Padding::new(0.0, 3.0));

    // Same total width either way.
    assert_eq!(left.width(), right.width());
    // But two characters of left padding put the numbers twenty pixels in.
    assert_eq!(left.number_left_edge(3, 5.0), 25.0);
    assert_eq!(right.number_left_edge(3, 5.0), 5.0);
}

#[test]
fn numbers_of_different_lengths_end_flush_with_each_other() {
    let column = three_digits(Padding::default());

    // A character of left padding, then the numbers ending at 45 whatever
    // their length.
    assert_eq!(column.number_left_edge(3, 5.0), 15.0);
    assert_eq!(column.number_left_edge(2, 5.0), 25.0);
    assert_eq!(column.number_left_edge(1, 5.0), 35.0);
}

/// What characters buy: the same padding beside a face that draws twice as
/// wide takes twice the room, so a column keeps its proportions whatever the
/// document is set in.
#[test]
fn the_same_padding_scales_with_the_face() {
    let padding = Padding::new(2.0, 1.0);

    let narrow = Column::new(3, 30.0, padding);
    let wide = Column::new(3, 60.0, padding);

    assert_eq!(wide.width(), narrow.width() * 2.0);
}

#[test]
fn the_text_gets_what_the_column_leaves() {
    let column = three_digits(Padding::default());

    assert_eq!(column.text_width(200.0), 150.0);
    // A strip narrower than the column itself leaves nothing, rather than a
    // negative width for cosmic-text to wrap against.
    assert_eq!(column.text_width(10.0), 0.0);
}

#[test]
fn a_column_too_wide_for_its_window_leaves_no_room() {
    let column = three_digits(Padding::default());

    // 50 for the column, 80 for eight characters of text.
    assert!(column.leaves_room_in(130.0));
    assert!(!column.leaves_room_in(129.0));
}

/// Both paddings arrive from a hand-edited `config.toml`, so a nonsense one
/// has to land somewhere usable rather than break the layout.
#[test]
fn a_nonsense_padding_is_brought_back_into_range() {
    assert_eq!(Padding::new(-4.0, -1.0), Padding::new(0.0, 0.0));

    let enormous = Padding::new(1_000.0, 1_000.0);
    assert_eq!(
        enormous,
        Padding::new(*PADDING_RANGE.end(), *PADDING_RANGE.end())
    );

    // `NaN` has no end of the range to clamp to, so it falls back instead.
    assert_eq!(Padding::new(f32::NAN, f32::NAN), Padding::default());
}

#[test]
fn a_column_is_padded_by_one_character_either_side_by_default() {
    assert_eq!(
        Padding::default(),
        Padding::new(DEFAULT_PADDING, DEFAULT_PADDING)
    );
    assert_eq!(DEFAULT_PADDING, 1.0);
}
