use core::ops::Range;

use luck_token::{Span, index_to_offset};

#[test]
fn span_len_and_is_empty_measure_the_range() {
    let span = Span::new(0, 10);
    assert_eq!(span.len(), 10);
    assert!(!span.is_empty());
    assert!(Span::new(5, 5).is_empty());
}

#[test]
fn span_merge_covers_both_spans() {
    let left = Span::new(5, 10);
    let right = Span::new(20, 30);
    let merged = left.merge(right);
    assert_eq!(merged, Span::new(5, 30));
}

#[test]
fn span_widens_to_a_usize_range() {
    let range: Range<usize> = Span::new(2, 8).into();
    assert_eq!(range, 2..8);
}

#[test]
fn span_from_indices_matches_new() {
    assert_eq!(Span::from_indices(2, 8), Span::new(2, 8));
}

#[test]
fn span_shifted_moves_both_ends_saturating_at_the_cap() {
    assert_eq!(Span::new(2, 8).shifted(10), Span::new(12, 18));
    assert_eq!(Span::new(2, u32::MAX).shifted(1).end, u32::MAX);
}

#[test]
fn span_text_reads_the_covered_source() {
    assert_eq!(Span::new(6, 7).text("local x = 1"), "x");
}

#[test]
fn span_text_is_empty_for_a_range_that_splits_or_overruns_the_source() {
    assert_eq!(Span::new(1, 2).text("\u{e9}"), "");
    assert_eq!(Span::new(0, 9).text("abc"), "");
}

#[test]
fn index_to_offset_saturates_instead_of_wrapping() {
    assert_eq!(index_to_offset(7), 7);
    assert_eq!(index_to_offset(usize::MAX), u32::MAX);
}

#[test]
fn bug_span_is_empty_agrees_with_len_on_an_inverted_span() {
    let span = Span::new(7, 3);
    assert_eq!(span.len(), 0);
    assert!(span.is_empty(), "a span whose length is 0 must report that it is empty");
}
