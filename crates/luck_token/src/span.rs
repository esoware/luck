use core::ops::Range;

/// Byte range in source text. Offsets are `u32` to keep spans small, which
/// caps a file at 4 GB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Span over byte indices `start..end`, narrowed by [`index_to_offset`].
    #[inline]
    #[must_use]
    pub fn from_indices(start: usize, end: usize) -> Self {
        Self::new(index_to_offset(start), index_to_offset(end))
    }

    #[must_use]
    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self { start: self.start.min(other.start), end: self.end.max(other.end) }
    }

    /// This span moved `offset` bytes later, saturating at the 4 GB cap.
    #[must_use]
    pub const fn shifted(self, offset: u32) -> Self {
        Self::new(self.start.saturating_add(offset), self.end.saturating_add(offset))
    }

    /// The text of `source` under this span. Empty if the span is out of range
    /// or splits a character, which a span lexed from `source` never does.
    #[inline]
    #[must_use]
    pub fn text(self, source: &str) -> &str {
        source.get(Range::<usize>::from(self)).unwrap_or_default()
    }
}

/// Widens a span for APIs that take `usize` ranges, such as ariadne.
impl From<Span> for Range<usize> {
    #[inline]
    fn from(span: Span) -> Self {
        offset_to_index(span.start)..offset_to_index(span.end)
    }
}

/// Widens a `u32` source offset to a `usize` index. Lossless on every target
/// with at least 32-bit pointers, where the fallback compiles away.
#[inline]
#[must_use]
pub fn offset_to_index(offset: u32) -> usize {
    usize::try_from(offset).unwrap_or(usize::MAX)
}

/// Narrows a `usize` index to a `u32` source offset, saturating instead of
/// wrapping. Lexing refuses sources over `u32::MAX` bytes, so an index into a
/// lexed source always fits.
#[inline]
#[must_use]
pub fn index_to_offset(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
