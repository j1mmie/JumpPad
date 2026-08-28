use super::*;

/// A text size that makes an em a round ten pixels, so a `Sizing` in ems and
/// the pixels it comes out as can be read against each other.
const TEXT_SIZE: f32 = 10.0;

/// Three digits ten pixels each, so the numbers measure their own 30 pixels
/// before any minimum is held against them.
fn three_digits(sizing: Sizing) -> Column {
    Column::new(3, 30.0, sizing, TEXT_SIZE)
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
fn the_gap_is_added_beside_the_numbers() {
    // Three digits' worth of numbers, and a gap of one em on top.
    let column = three_digits(Sizing::new(0.0, 1.0));

    assert_eq!(column.width(), 40.0);
}

/// The whole point of a floor: a file being typed into keeps its text still
/// until it really does need another digit.
#[test]
fn a_minimum_wider_than_the_digits_pads_the_column() {
    let column = three_digits(Sizing::new(5.0, 1.0));

    // Five ems of numbers, plus the one-em gap.
    assert_eq!(column.width(), 60.0);
}

#[test]
fn a_minimum_narrower_than_the_digits_leaves_them_alone() {
    let column = three_digits(Sizing::new(1.0, 1.0));

    assert_eq!(column.width(), 40.0);
}

/// A padded column is still a column of digits: the numbers sit against its
/// right edge, so the padding shows up as space on the left.
#[test]
fn a_padded_column_still_ends_its_numbers_flush() {
    let column = three_digits(Sizing::new(5.0, 1.0));

    // Numbers 50 wide from a left edge of 5, so they end at 55 whatever
    // their length - and a digit is still the ten pixels it measured.
    assert_eq!(column.number_left_edge(3, 5.0), 25.0);
    assert_eq!(column.number_left_edge(1, 5.0), 45.0);
}

#[test]
fn numbers_of_different_lengths_end_flush_with_each_other() {
    let column = three_digits(Sizing::new(0.0, 1.0));

    assert_eq!(column.number_left_edge(3, 5.0), 5.0);
    assert_eq!(column.number_left_edge(2, 5.0), 15.0);
    assert_eq!(column.number_left_edge(1, 5.0), 25.0);
}

/// What ems buy: the same setting at twice the text size takes twice the
/// room, so a column keeps its proportions however big the document is set.
#[test]
fn the_same_sizing_scales_with_the_text() {
    let sizing = Sizing::new(5.0, 1.0);

    let small = Column::new(3, 30.0, sizing, TEXT_SIZE);
    let large = Column::new(3, 60.0, sizing, TEXT_SIZE * 2.0);

    assert_eq!(large.width(), small.width() * 2.0);
}

#[test]
fn the_text_gets_what_the_column_leaves() {
    let column = three_digits(Sizing::new(0.0, 1.0));

    assert_eq!(column.text_width(200.0), 160.0);
    // A strip narrower than the column itself leaves nothing, rather than a
    // negative width for cosmic-text to wrap against.
    assert_eq!(column.text_width(10.0), 0.0);
}

#[test]
fn a_column_too_wide_for_its_window_leaves_no_room() {
    let column = three_digits(Sizing::new(0.0, 1.0));

    // 40 for the column, 80 for eight characters of text.
    assert!(column.leaves_room_in(120.0));
    assert!(!column.leaves_room_in(119.0));
}

/// Both settings arrive from a hand-edited `config.toml`, so a nonsense one
/// has to land somewhere usable rather than break the layout.
#[test]
fn a_nonsense_sizing_is_brought_back_into_range() {
    let negative = Sizing::new(-4.0, -1.0);
    assert_eq!(negative, Sizing::new(0.0, 0.0));

    let enormous = Sizing::new(1_000.0, 1_000.0);
    assert_eq!(
        enormous,
        Sizing::new(*SIZING_RANGE.end(), *SIZING_RANGE.end())
    );

    // `NaN` has no end of the range to clamp to, so it falls back instead.
    assert_eq!(Sizing::new(f32::NAN, f32::NAN), Sizing::default());
}

/// A monospace digit typically draws at 0.6 em, so the shipped defaults are
/// the three digits and the one-digit gap the column was hard-coded to
/// before either was a setting - which is what keeps an existing window
/// exactly where it was.
#[test]
fn the_shipped_sizing_is_what_the_numbers_used_to_measure() {
    let digit = DEFAULT_GAP;

    assert!((DEFAULT_MINIMUM - digit * 3.0).abs() < 1e-6);
    assert_eq!(Sizing::default(), Sizing::new(DEFAULT_MINIMUM, DEFAULT_GAP));
}
