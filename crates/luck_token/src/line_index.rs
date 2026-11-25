use std::iter;

use crate::index_to_offset;

/// Where each line of a source starts, with lines broken as Luau breaks them.
#[derive(Debug, Clone)]
pub struct LineIndex {
    line_starts: Vec<u32>,
}

impl LineIndex {
    /// A line starts past each LF, which ends every line break, a CRLF's
    /// included.
    pub fn new(source: &[u8]) -> Self {
        let line_starts = iter::once(0)
            .chain(memchr::memchr_iter(b'\n', source).map(|lf| index_to_offset(lf + 1)))
            .collect();
        Self { line_starts }
    }

    /// The one-based line `offset` is on.
    pub fn line(&self, offset: u32) -> u32 {
        index_to_offset(self.line_starts.partition_point(|&start| start <= offset))
    }

    /// The one-based line and byte column of `offset`.
    pub fn line_column(&self, offset: u32) -> (u32, u32) {
        let line = self.line(offset);
        let line_start = self.line_starts[(line - 1) as usize];
        (line, offset - line_start + 1)
    }
}
