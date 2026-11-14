//! Byte offsets are `u32` to match `luck_token::Span`, which caps files at
//! 4 GB.

use core::ops;

use luck_token::{Span, index_to_offset, offset_to_index};
use tower_lsp_server::ls_types::{Position, Range};

#[derive(Debug)]
pub(crate) struct LineIndex {
    /// Byte offset of the first character on each line. Line N starts at
    /// `line_starts[N]` and ends just before `line_starts[N + 1]` (or end-of-file).
    line_starts: Vec<u32>,
    /// Each character that takes more UTF-8 bytes than UTF-16 units, so a
    /// column costs a search, not a walk along its line.
    wide_chars: Vec<WideChar>,
    /// Total source length in bytes, kept to clamp out-of-bounds requests.
    source_length: u32,
}

#[derive(Debug, Clone, Copy)]
struct WideChar {
    start: u32,
    end: u32,
    /// How many more bytes than units this character and every one before
    /// it take.
    extra_through: u32,
}

impl WideChar {
    /// How many more bytes than units every character before this one takes.
    const fn extra_before(self) -> u32 {
        // Only a 4-byte character takes two units.
        let units = if self.end.saturating_sub(self.start) == 4 { 2 } else { 1 };
        self.extra_through.saturating_sub(self.end.saturating_sub(self.start).saturating_sub(units))
    }
}

impl LineIndex {
    pub(crate) fn new(source: &str) -> Self {
        let mut line_starts = Vec::with_capacity(source.len().div_ceil(40));
        line_starts.push(0);
        let mut wide_chars = Vec::new();
        let mut extra_bytes = 0u32;
        let bytes = source.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            // LSP's EOLs, the client's for every dialect, are LF, CRLF, and CR.
            let is_line_end =
                *b == b'\n' || (*b == b'\r' && bytes.get(i.saturating_add(1)) != Some(&b'\n'));
            if is_line_end {
                line_starts.push(index_to_offset(i).saturating_add(1));
            }
            // A lead byte gives its character's length, and only a 4-byte one
            // takes two UTF-16 units.
            let (length, extra) = match *b {
                0xC0..=0xDF => (2, 1),
                0xE0..=0xEF => (3, 2),
                0xF0..=0xFF => (4, 2),
                _ => continue,
            };
            extra_bytes = extra_bytes.saturating_add(extra);
            wide_chars.push(WideChar {
                start: index_to_offset(i),
                end: index_to_offset(i.saturating_add(length)),
                extra_through: extra_bytes,
            });
        }
        Self { line_starts, wide_chars, source_length: index_to_offset(source.len()) }
    }

    /// How many more UTF-8 bytes than UTF-16 units the text before `offset`
    /// takes.
    fn extra_bytes_before(&self, offset: u32) -> u32 {
        let index = self.wide_chars.partition_point(|wide| wide.end <= offset);
        index
            .checked_sub(1)
            .and_then(|last| self.wide_chars.get(last))
            .map_or(0, |wide| wide.extra_through)
    }

    fn line_bounds(&self, line: usize) -> (u32, u32) {
        let mut starts = self.line_starts.iter().skip(line).copied();
        let start = starts.next().unwrap_or_default();
        (start, starts.next().unwrap_or(self.source_length))
    }

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
        let character = if source.is_char_boundary(offset_to_index(upper)) {
            let extra =
                self.extra_bytes_before(upper).saturating_sub(self.extra_bytes_before(line_start));
            upper.saturating_sub(line_start).saturating_sub(extra)
        } else {
            0
        };
        Position { line: index_to_offset(line), character }
    }

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
        let without_lf = terminated_line.strip_suffix('\n').unwrap_or(terminated_line);
        let line_text = without_lf.strip_suffix('\r').unwrap_or(without_lf);
        let text_end = line_start.saturating_add(index_to_offset(line_text.len()));
        let column = position.character;
        let extra_at_line_start = self.extra_bytes_before(line_start);
        // The units from the line's start to `offset`, with `extra` more bytes
        // than units before it.
        let units_to = |offset: u32, extra: u32| {
            offset
                .saturating_sub(line_start)
                .saturating_sub(extra.saturating_sub(extra_at_line_start))
        };
        let first = self.wide_chars.partition_point(|wide| wide.start < line_start);
        let last = self.wide_chars.partition_point(|wide| wide.start < text_end);
        let on_line = self.wide_chars.get(first..last).unwrap_or_default();
        // The first wide character that ends past the column, and the last
        // boundary before it.
        let next = on_line.partition_point(|wide| units_to(wide.end, wide.extra_through) <= column);
        let (anchor, anchor_extra) = next
            .checked_sub(1)
            .and_then(|previous| on_line.get(previous))
            .map_or((line_start, extra_at_line_start), |wide| (wide.end, wide.extra_through));
        let narrow_offset =
            anchor.saturating_add(column.saturating_sub(units_to(anchor, anchor_extra)));
        match on_line.get(next) {
            // A column inside a character lands on its start.
            Some(wide) if units_to(wide.start, wide.extra_before()) <= column => wide.start,
            Some(_) => narrow_offset,
            // A column past the line's end clamps to it.
            None => narrow_offset.min(text_end),
        }
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

    /// The column of every offset is the UTF-16 length of the line before
    /// it, and every column of every line maps back to the last character
    /// boundary at or before it.
    #[test]
    fn positions_and_offsets_count_utf16_units_on_every_line() {
        let source = "\
a\u{e4}\u{6f22}\u{1F600}b
\u{1F600}\r
\u{e4}x\ry\u{6f22}

zz";
        let index = LineIndex::new(source);
        for offset in 0..=source.len() {
            let position = index.position(source, index_to_offset(offset));
            let line_start = index.line_starts[offset_to_index(position.line)];
            let expected = source
                .get(offset_to_index(line_start)..offset)
                .map_or(0, |line| index_to_offset(line.encode_utf16().count()));
            assert_eq!(position.character, expected, "offset {offset}");
        }
        for (line_number, &line_start) in index.line_starts.iter().enumerate() {
            let line = index_to_offset(line_number);
            let (_, line_end) = index.line_bounds(line_number);
            let terminated = Span::new(line_start, line_end).text(source);
            let line_text = terminated.trim_end_matches(['\n', '\r']);
            let utf16_length = index_to_offset(line_text.encode_utf16().count());
            for column in 0..utf16_length.saturating_add(3) {
                let mut walked = line_start;
                let mut remaining = column;
                for c in line_text.chars() {
                    let Some(rest) = remaining.checked_sub(index_to_offset(c.len_utf16())) else {
                        break;
                    };
                    remaining = rest;
                    walked = walked.saturating_add(index_to_offset(c.len_utf8()));
                }
                let position = Position { line, character: column };
                assert_eq!(index.offset(source, position), walked, "line {line} column {column}");
            }
        }
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
    fn a_bare_cr_ends_a_line() {
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
