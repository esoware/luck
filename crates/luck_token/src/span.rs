/// Byte range in source text. Uses u32 (not usize) for compact storage - 4 GB file limit.
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

    #[must_use]
    pub const fn len(&self) -> u32 {
        self.end - self.start
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start == self.end
    }

    #[must_use]
    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// Spans are `u32` workspace-wide (4GB cap); diagnostic/ariadne boundaries want `usize`.
/// This is the single conversion point for that boundary.
impl From<Span> for std::ops::Range<usize> {
    fn from(span: Span) -> Self {
        span.start as usize..span.end as usize
    }
}

/// One-based `(line, column)` for a byte `offset` into `source`. Columns
/// count bytes from the line start, matching ariadne's `IndexType::Byte`
/// rendering so a position quoted in diagnostic text lines up with what the
/// CLI prints. An out-of-range offset clamps to the end of the source.
#[must_use]
pub fn line_col(source: &str, offset: u32) -> (u32, u32) {
    let end = (offset as usize).min(source.len());
    let mut line = 1u32;
    let mut line_start = 0usize;
    for (idx, &byte) in source.as_bytes()[..end].iter().enumerate() {
        if byte == b'\n' {
            line += 1;
            line_start = idx + 1;
        }
    }
    (line, (end - line_start) as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_basics() {
        let span = Span::new(0, 10);
        assert_eq!(span.len(), 10);
        assert!(!span.is_empty());
        assert!(Span::new(5, 5).is_empty());
    }

    #[test]
    fn span_merge() {
        let a = Span::new(5, 10);
        let b = Span::new(20, 30);
        let merged = a.merge(b);
        assert_eq!(merged, Span::new(5, 30));
    }

    #[test]
    fn span_conversion_to_usize_range() {
        let range: std::ops::Range<usize> = Span::new(2, 8).into();
        assert_eq!(range, 2..8);
    }

    #[test]
    fn line_col_is_one_based_byte_columns() {
        let source = "local x\ndo local y\nend";
        assert_eq!(line_col(source, 0), (1, 1));
        assert_eq!(line_col(source, 6), (1, 7));
        // First byte of line 2 (the `d` in `do`).
        assert_eq!(line_col(source, 8), (2, 1));
        // `y` on line 2.
        assert_eq!(line_col(source, 17), (2, 10));
    }

    #[test]
    fn line_col_clamps_out_of_range() {
        let source = "abc\ndef";
        assert_eq!(line_col(source, 999), (2, 4));
    }
}
