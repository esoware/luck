use core::ops::Range;

/// Offsets are `u32` to keep spans small, which caps a file at 4 GB.
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

    /// Empty if the span is out of range or splits a character, which a span lexed from `source`
    /// never does.
    #[inline]
    #[must_use]
    pub fn text(self, source: &str) -> &str {
        source.get(Range::<usize>::from(self)).unwrap_or_default()
    }
}

impl From<Span> for Range<usize> {
    #[inline]
    fn from(span: Span) -> Self {
        offset_to_index(span.start)..offset_to_index(span.end)
    }
}

/// Lossless on every target with at least 32-bit pointers, where the fallback compiles away.
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
