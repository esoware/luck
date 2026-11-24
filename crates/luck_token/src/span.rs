use std::ops::Range;

/// Offsets are `u32` to keep spans small, which caps a file at 4 GiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    #[inline]
    pub fn from_indices(start: usize, end: usize) -> Self {
        Self::new(index_to_offset(start), index_to_offset(end))
    }

    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self { start: self.start.min(other.start), end: self.end.max(other.end) }
    }

    /// This span moved `offset` bytes later, saturating at the 4 GiB cap.
    #[must_use]
    pub const fn shifted(self, offset: u32) -> Self {
        Self::new(self.start.saturating_add(offset), self.end.saturating_add(offset))
    }

    /// Empty if the span is out of range, which a span lexed from `source` never is.
    #[inline]
    pub fn text(self, source: &[u8]) -> &[u8] {
        source.get(Range::<usize>::from(self)).unwrap_or_default()
    }
}

impl From<Span> for Range<usize> {
    #[inline]
    fn from(span: Span) -> Self {
        offset_to_index(span.start)..offset_to_index(span.end)
    }
}

#[inline]
pub const fn offset_to_index(offset: u32) -> usize {
    offset as usize
}

/// Narrows a position in source text, a byte offset or a line number, to the
/// `u32` a span holds. Every position in a lexed source fits. A source over
/// 4 GiB, which the lexer refuses with one error, is still indexed by lines to
/// report it, and its positions past `u32::MAX` clamp there.
#[inline]
pub fn index_to_offset(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}
