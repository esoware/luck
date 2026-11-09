//! Mapping between byte offsets and LSP positions, whose `character` counts
//! UTF-16 code units.
//!
//! Byte offsets are `u32` to match `luck_token::Span`, which caps files at
//! 4 GB.

use core::ops;

use luck_token::{Span, index_to_offset, offset_to_index};
use tower_lsp::lsp_types::{Position, Range};

#[derive(Debug)]
pub(crate) struct LineIndex {
    /// Byte offset of the first character on each line. Line N starts at
    /// `line_starts[N]` and ends just before `line_starts[N + 1]` (or end-of-file).
    line_starts: Vec<u32>,
    /// Total source length in bytes, kept to clamp out-of-bounds requests.
    source_length: u32,
}

impl LineIndex {
    pub(crate) fn new(source: &str) -> Self {
        let mut line_starts = Vec::with_capacity(source.len().div_ceil(40));
        line_starts.push(0);
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index_to_offset(index).saturating_add(1));
            }
        }
        Self { line_starts, source_length: index_to_offset(source.len()) }
    }

    /// Byte offsets where `line` starts and where the next line starts, or
    /// the end of the file on the last line.
    fn line_bounds(&self, line: usize) -> (u32, u32) {
        let mut starts = self.line_starts.iter().skip(line).copied();
        let start = starts.next().unwrap_or_default();
        (start, starts.next().unwrap_or(self.source_length))
    }

    /// Convert a byte offset into an LSP `Position` (UTF-16 character index).
    pub(crate) fn position(&self, source: &str, offset: u32) -> Position {
        let clamped = offset.min(self.source_length);
        let line = match self.line_starts.binary_search(&clamped) {
            Ok(exact) => exact,
            // `Err(insert)` returns the index where `clamped` *would* go; the
            // line containing `clamped` is therefore `insert - 1`.
            Err(insert) => insert.saturating_sub(1),
        };
        let (line_start, line_end) = self.line_bounds(line);
        let upper = clamped.min(line_end);
        // An offset inside a multi-byte character maps to column 0.
        let line_text = Span::new(line_start, upper).text(source);
        Position {
            line: index_to_offset(line),
            character: index_to_offset(line_text.encode_utf16().count()),
        }
    }

    /// Convert an LSP `Position` (line + UTF-16 character) into a byte offset.
    ///
    /// A line past the end clamps to the last line, and a character past the
    /// end of its line clamps to the line end.
    pub(crate) fn offset(&self, source: &str, position: Position) -> u32 {
        let last_line = self.line_starts.len().saturating_sub(1);
        let line = offset_to_index(position.line).min(last_line);
        let (line_start, line_end) = self.line_bounds(line);
        let line_span = Span::new(line_start, line_end);
        let Some(terminated_line) = source.get(ops::Range::<usize>::from(line_span)) else {
            return line_end;
        };
        // LSP clamps a character past the line's end to the line end. The
        // walk excludes the terminator so an overshooting column can't land
        // on the next line.
        let line_text = terminated_line.strip_suffix('\n').map_or(terminated_line, |without_lf| {
            without_lf.strip_suffix('\r').unwrap_or(without_lf)
        });
        let mut remaining_utf16_units = position.character;
        for (byte_offset, character) in line_text.char_indices() {
            match remaining_utf16_units.checked_sub(index_to_offset(character.len_utf16())) {
                Some(utf16_units) => remaining_utf16_units = utf16_units,
                None => return line_start.saturating_add(index_to_offset(byte_offset)),
            }
        }
        line_start.saturating_add(index_to_offset(line_text.len()))
    }

    pub(crate) fn range(&self, source: &str, span: Span) -> Range {
        Range { start: self.position(source, span.start), end: self.position(source, span.end) }
    }

    /// The byte range an LSP `Range` covers, or `None` when its end comes
    /// before its start or past the end of `source`.
    pub(crate) fn byte_range(&self, source: &str, range: Range) -> Option<ops::Range<usize>> {
        let byte_range: ops::Range<usize> =
            Span::new(self.offset(source, range.start), self.offset(source, range.end)).into();
        (byte_range.start <= byte_range.end && byte_range.end <= source.len()).then_some(byte_range)
    }

    pub(crate) fn full_document_range(&self, source: &str) -> Range {
        Range {
            start: Position { line: 0, character: 0 },
            end: self.position(source, self.source_length),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_offset_roundtrip_ascii() {
        let source = "hello\nworld\nfoo";
        let index = LineIndex::new(source);
        let position = index.position(source, 6);
        assert_eq!(position, Position { line: 1, character: 0 });
        assert_eq!(index.offset(source, position), 6);
    }

    #[test]
    fn position_handles_multi_byte() {
        // U+00E4 is 2 bytes in UTF-8 and 1 UTF-16 unit.
        let source = "\u{e4}\nb";
        let index = LineIndex::new(source);
        let position = index.position(source, 3);
        assert_eq!(position, Position { line: 1, character: 0 });
    }

    #[test]
    fn position_handles_surrogate_pair() {
        // 'a' + emoji (U+1F600, 2 UTF-16 units, 4 UTF-8 bytes) + 'b'.
        let source = "a\u{1F600}b";
        let index = LineIndex::new(source);
        // 'b' is at byte offset 5 -> line 0, character 3 (1 + 2 surrogate units).
        let position = index.position(source, 5);
        assert_eq!(position, Position { line: 0, character: 3 });
        assert_eq!(index.offset(source, position), 5);
    }

    #[test]
    fn offset_clamps_an_out_of_range_position() {
        let source = "abc";
        let index = LineIndex::new(source);
        let position = Position { line: 99, character: 99 };
        let offset = index.offset(source, position);
        assert_eq!(offset, 3);
    }

    #[test]
    fn bug_a_bare_cr_ends_a_line() {
        // LSP defines `\r` as an EOL alongside `\n` and `\r\n`, and Lua's
        // lexer counts it as a line break too.
        let source = "local a = 1\rlocal b = 2";
        let index = LineIndex::new(source);
        let b_offset = index_to_offset(source.rfind('b').expect("b"));
        assert_eq!(
            index.position(source, b_offset),
            Position { line: 1, character: 6 },
            "text after a bare `\\r` sits on the next line"
        );
        assert_eq!(
            index.offset(source, Position { line: 1, character: 6 }),
            b_offset,
            "line 1 must start after the bare `\\r`"
        );
    }
}
