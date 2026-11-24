use std::iter;

use crate::{LuaVersion, chunk_start, has_shebang, index_to_offset, line_breaks};

/// Where each line of a source starts, with lines broken as `version` breaks
/// them and the first one starting past a BOM its loader skips.
#[derive(Debug, Clone)]
pub struct LineIndex {
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(source: &[u8], version: LuaVersion) -> Self {
        let first_line_start = chunk_start(source, version);
        let chunk = &source[first_line_start..];
        // Loaders skip a shebang line up to its LF, so a CR in it starts no
        // line.
        let shebang_length = if has_shebang(chunk, version) {
            memchr::memchr(b'\n', chunk).unwrap_or(chunk.len())
        } else {
            0
        };
        let scan_start = first_line_start + shebang_length;
        let rest = &source[scan_start..];
        let line_starts = iter::once(index_to_offset(first_line_start))
            .chain(
                line_breaks(rest, version)
                    .map(|line_break| index_to_offset(scan_start + line_break.end)),
            )
            .collect();
        Self { line_starts }
    }

    /// The one-based line `offset` is on.
    pub fn line(&self, offset: u32) -> u32 {
        index_to_offset(self.line_starts.partition_point(|&start| start <= offset)).max(1)
    }

    /// The one-based line and byte column of `offset`.
    pub fn line_column(&self, offset: u32) -> (u32, u32) {
        let line = self.line(offset);
        let line_start = self.line_starts[(line - 1) as usize];
        // An offset in a BOM before the first line is in its first column.
        (line, offset.saturating_sub(line_start) + 1)
    }
}
