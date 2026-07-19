//! Mapping between LSP `Position` (UTF-16 line/character) and byte offsets.
//!
//! The LSP spec uses UTF-16 code units for `Position.character` by default. We
//! compute line starts once on document load/change and translate per request.
//! Byte offsets are `u32` to match `luck_token::Span`, which caps files at 4 GB.

use tower_lsp::lsp_types::{Position, Range};

#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the first character on each line. Line N starts at
    /// `line_starts[N]` and ends just before `line_starts[N + 1]` (or end-of-file).
    line_starts: Vec<u32>,
    /// Total source length in bytes - kept so we can clamp out-of-bounds requests.
    total_len: u32,
}

impl LineIndex {
    #[must_use]
    pub fn new(source: &str) -> Self {
        let mut line_starts = Vec::with_capacity(source.len() / 40 + 1);
        line_starts.push(0);
        for (idx, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                // The next line begins after the newline byte.
                line_starts.push((idx as u32) + 1);
            }
        }
        Self {
            line_starts,
            total_len: source.len() as u32,
        }
    }

    /// Convert a byte offset into an LSP `Position` (UTF-16 character index).
    #[must_use]
    pub fn position(&self, source: &str, offset: u32) -> Position {
        let clamped = offset.min(self.total_len);
        let line_idx = match self.line_starts.binary_search(&clamped) {
            Ok(exact) => exact,
            // `Err(idx)` returns the index where `clamped` *would* go; the
            // line containing `clamped` is therefore `idx - 1`.
            Err(insert) => insert.saturating_sub(1),
        };
        let line_start = self.line_starts[line_idx];
        let line_byte_end = self
            .line_starts
            .get(line_idx + 1)
            .copied()
            .unwrap_or(self.total_len);
        // `clamped` may be past end-of-line on the final line. Walk the slice
        // up to `clamped`, counting UTF-16 code units.
        let upper = clamped.min(line_byte_end);
        let line_slice = &source.as_bytes()[line_start as usize..upper as usize];
        // Should not happen for valid source, but degrade gracefully on bad UTF-8.
        let line_str = std::str::from_utf8(line_slice).unwrap_or_default();
        let utf16_units: u32 = line_str.chars().map(|ch| ch.len_utf16() as u32).sum();
        Position {
            line: line_idx as u32,
            character: utf16_units,
        }
    }

    /// Convert an LSP `Position` (line + UTF-16 character) into a byte offset.
    /// Out-of-range positions clamp to the end of the file.
    #[must_use]
    pub fn offset(&self, source: &str, position: Position) -> u32 {
        if self.line_starts.is_empty() {
            return 0;
        }
        let line_idx = (position.line as usize).min(self.line_starts.len() - 1);
        let line_start = self.line_starts[line_idx];
        let line_end = self
            .line_starts
            .get(line_idx + 1)
            .copied()
            .unwrap_or(self.total_len);
        let line_slice = &source.as_bytes()[line_start as usize..line_end as usize];
        let line_str = match std::str::from_utf8(line_slice) {
            Ok(s) => s,
            Err(_) => return line_end,
        };
        // LSP: a character offset past the line's end clamps TO the line
        // end. The walked slice must therefore exclude the terminator, or
        // an overshooting column consumes the `\n` and lands on the NEXT
        // line - flipping range-overlap results by one byte.
        let line_str = line_str
            .strip_suffix('\n')
            .map(|without_lf| without_lf.strip_suffix('\r').unwrap_or(without_lf))
            .unwrap_or(line_str);
        // Walk characters, accumulating UTF-16 units until we hit `position.character`.
        let mut remaining_units = position.character;
        let mut bytes_consumed: u32 = 0;
        for ch in line_str.chars() {
            let ch_utf16 = ch.len_utf16() as u32;
            if remaining_units < ch_utf16 {
                break;
            }
            remaining_units -= ch_utf16;
            bytes_consumed += ch.len_utf8() as u32;
            if remaining_units == 0 {
                break;
            }
        }
        line_start + bytes_consumed
    }

    /// Convert a span (byte range) to an LSP `Range`.
    #[must_use]
    pub fn range(&self, source: &str, start: u32, end: u32) -> Range {
        Range {
            start: self.position(source, start),
            end: self.position(source, end),
        }
    }

    /// LSP `Range` covering the entire document. Used for full-document
    /// formatting edits.
    #[must_use]
    pub fn full_document_range(&self, source: &str) -> Range {
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: self.position(source, self.total_len),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_offset_roundtrip_ascii() {
        let source = "hello\nworld\nfoo";
        let idx = LineIndex::new(source);
        // 'w' at offset 6 -> line 1 char 0
        let pos = idx.position(source, 6);
        assert_eq!(
            pos,
            Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(idx.offset(source, pos), 6);
    }

    #[test]
    fn position_handles_multi_byte() {
        // 'ä' is 2 bytes UTF-8, 1 UTF-16 unit.
        let source = "ä\nb";
        let idx = LineIndex::new(source);
        let pos = idx.position(source, 3);
        assert_eq!(
            pos,
            Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn position_handles_surrogate_pair() {
        // 'a' + emoji (U+1F600, 2 UTF-16 units, 4 UTF-8 bytes) + 'b'.
        let source = "a\u{1F600}b";
        let idx = LineIndex::new(source);
        // 'b' is at byte offset 5 -> line 0, character 3 (1 + 2 surrogate units).
        let pos = idx.position(source, 5);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(idx.offset(source, pos), 5);
    }

    #[test]
    fn out_of_range_position_clamps() {
        let source = "abc";
        let idx = LineIndex::new(source);
        let pos = Position {
            line: 99,
            character: 99,
        };
        let offset = idx.offset(source, pos);
        assert_eq!(offset, 3);
    }
}
