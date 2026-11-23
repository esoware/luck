//! Byte offsets are `u32` to match `luck_token::Span`, which caps files at
//! 4 GB.

use core::ops;

use ls_types::{Position, PositionEncodingKind, Range};
use luck_token::{Span, index_to_offset, offset_to_index};

/// What a column counts, which `initialize` settles for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PositionEncoding {
    /// Bytes, the server's own offsets, so no column needs a search.
    Utf8,
    /// UTF-16 code units, which LSP assumes of a client that names none.
    Utf16,
    /// Characters.
    Utf32,
}

impl PositionEncoding {
    /// UTF-8 if the client offers it, since a column then costs nothing,
    /// or else the first of the other two that it offers, or UTF-16.
    pub(crate) fn negotiate(offered: &[PositionEncodingKind]) -> Self {
        if offered.contains(&PositionEncodingKind::UTF8) {
            return Self::Utf8;
        }
        offered
            .iter()
            .find_map(|kind| {
                if *kind == PositionEncodingKind::UTF16 {
                    Some(Self::Utf16)
                } else if *kind == PositionEncodingKind::UTF32 {
                    Some(Self::Utf32)
                } else {
                    None
                }
            })
            .unwrap_or(Self::Utf16)
    }

    pub(crate) const fn kind(self) -> PositionEncodingKind {
        match self {
            Self::Utf8 => PositionEncodingKind::UTF8,
            Self::Utf16 => PositionEncodingKind::UTF16,
            Self::Utf32 => PositionEncodingKind::UTF32,
        }
    }

    /// How many more bytes than units a character of `length` UTF-8 bytes
    /// takes.
    const fn extra_bytes(self, length: u32) -> u32 {
        let units = match self {
            Self::Utf8 => length,
            // Only a 4-byte character takes two units.
            Self::Utf16 => {
                if length == 4 {
                    2
                } else {
                    1
                }
            }
            Self::Utf32 => 1,
        };
        length.saturating_sub(units)
    }
}

#[derive(Debug)]
pub(crate) struct LineIndex {
    /// Byte offset of the first character on each line. Line N starts at
    /// `line_starts[N]` and ends just before `line_starts[N + 1]` (or end-of-file).
    line_starts: Vec<u32>,
    /// Each character that takes more bytes than units, so a column costs a
    /// search, not a walk along its line. Under UTF-8, none does.
    wide_chars: Vec<WideChar>,
    /// Total source length in bytes, kept to clamp out-of-bounds requests.
    source_length: u32,
    encoding: PositionEncoding,
}

#[derive(Debug, Clone, Copy)]
struct WideChar {
    start: u32,
    end: u32,
    /// How many more bytes than units this character and every one before
    /// it take.
    extra_through: u32,
}

/// The run of bytes the wide-character scan skips at once when it's all
/// ASCII, which most of a Lua source is.
const ASCII_RUN: usize = 64;

impl LineIndex {
    pub(crate) fn new(source: &str, encoding: PositionEncoding) -> Self {
        let bytes = source.as_bytes();
        let mut line_starts = Vec::with_capacity(source.len().div_ceil(40));
        line_starts.push(0);
        for i in memchr::memchr2_iter(b'\n', b'\r', bytes) {
            // LSP's EOLs, the client's for every dialect, are LF, CRLF, and CR.
            let next = i.saturating_add(1);
            if bytes.get(i) == Some(&b'\n') || bytes.get(next) != Some(&b'\n') {
                line_starts.push(index_to_offset(next));
            }
        }
        let mut wide_chars = Vec::new();
        if encoding != PositionEncoding::Utf8 {
            let mut extra_bytes = 0u32;
            for (run_index, run) in bytes.chunks(ASCII_RUN).enumerate() {
                if run.is_ascii() {
                    continue;
                }
                let run_start = run_index.saturating_mul(ASCII_RUN);
                for (i, b) in run.iter().enumerate() {
                    // A lead byte gives its character's length.
                    let length = match *b {
                        0xC0..=0xDF => 2,
                        0xE0..=0xEF => 3,
                        0xF0..=0xFF => 4,
                        _ => continue,
                    };
                    let start = index_to_offset(run_start.saturating_add(i));
                    extra_bytes = extra_bytes.saturating_add(encoding.extra_bytes(length));
                    wide_chars.push(WideChar {
                        start,
                        end: start.saturating_add(length),
                        extra_through: extra_bytes,
                    });
                }
            }
        }
        Self { line_starts, wide_chars, source_length: index_to_offset(source.len()), encoding }
    }

    /// How many more bytes than units every character before `wide` takes.
    const fn extra_before(&self, wide: WideChar) -> u32 {
        let own = self.encoding.extra_bytes(wide.end.saturating_sub(wide.start));
        wide.extra_through.saturating_sub(own)
    }

    /// How many more bytes than units the text before `offset` takes.
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
        self.positions(source).position(offset)
    }

    /// A cursor that finds each offset's position from the last one's, for a
    /// run of offsets in order.
    pub(crate) const fn positions<'index>(&'index self, source: &'index str) -> Positions<'index> {
        Positions { index: self, source, last: None }
    }

    /// How many units the text `span` covers takes.
    pub(crate) fn length(&self, span: Span) -> u32 {
        let extra =
            self.extra_bytes_before(span.end).saturating_sub(self.extra_bytes_before(span.start));
        span.end.min(self.source_length).saturating_sub(span.start).saturating_sub(extra)
    }

    fn search(&self, offset: u32) -> Cursor {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(exact) => exact,
            // `Err(insert)` returns the index where `offset` *would* go; the
            // line containing `offset` is therefore `insert - 1`.
            Err(insert) => insert.saturating_sub(1),
        };
        let wide_before = self.wide_chars.partition_point(|wide| wide.end <= offset);
        Cursor { line, line_extra: self.extra_bytes_before(self.line_bounds(line).0), wide_before }
    }

    /// `cursor` moved on to `offset`, or `None` when `offset` is behind it
    /// or too far ahead to step to.
    fn step(&self, cursor: Cursor, offset: u32) -> Option<Cursor> {
        let Cursor { mut line, mut line_extra, mut wide_before } = cursor;
        let is_behind = offset < self.line_bounds(line).0
            || self.wide_before(wide_before).is_some_and(|wide| wide.end > offset);
        let is_far = self
            .line_starts
            .get(line.saturating_add(STEP_LIMIT))
            .is_some_and(|&start| start <= offset)
            || self
                .wide_chars
                .get(wide_before.saturating_add(STEP_LIMIT))
                .is_some_and(|wide| wide.end <= offset);
        if is_behind || is_far {
            return None;
        }
        let first_line = line;
        while self.line_starts.get(line.saturating_add(1)).is_some_and(|&next| next <= offset) {
            line = line.saturating_add(1);
        }
        while self.wide_chars.get(wide_before).is_some_and(|wide| wide.end <= offset) {
            wide_before = wide_before.saturating_add(1);
        }
        if line != first_line {
            line_extra = self.extra_bytes_before(self.line_bounds(line).0);
        }
        Some(Cursor { line, line_extra, wide_before })
    }

    /// The last of the first `count` wide characters.
    fn wide_before(&self, count: usize) -> Option<&WideChar> {
        self.wide_chars.get(count.checked_sub(1)?)
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
        let landed = match on_line.get(next) {
            // A column inside a character lands on its start.
            Some(&wide) if units_to(wide.start, self.extra_before(wide)) <= column => wide.start,
            Some(_) => narrow_offset,
            // A column past the line's end clamps to it.
            None => narrow_offset.min(text_end),
        };
        // Under UTF-8 no character is wide, so a byte column inside one
        // lands on its start here.
        index_to_offset(
            line_text.floor_char_boundary(offset_to_index(landed.saturating_sub(line_start))),
        )
        .saturating_add(line_start)
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

/// The most lines or wide characters a cursor steps over before it searches
/// instead.
const STEP_LIMIT: usize = 16;

/// Finds each offset's position by stepping on from the last one's.
///
/// A run of offsets in order, a few lines apart, costs one pass over the
/// lines and wide characters between them rather than a search each.
pub(crate) struct Positions<'index> {
    index: &'index LineIndex,
    source: &'index str,
    last: Option<Cursor>,
}

/// Where an offset fell.
#[derive(Debug, Clone, Copy)]
struct Cursor {
    line: usize,
    /// How many more bytes than units the text before `line` takes.
    line_extra: u32,
    /// How many wide characters end by the offset.
    wide_before: usize,
}

impl Positions<'_> {
    pub(crate) fn position(&mut self, offset: u32) -> Position {
        let index = self.index;
        let clamped = offset.min(index.source_length);
        let cursor = self
            .last
            .and_then(|last| index.step(last, clamped))
            .unwrap_or_else(|| index.search(clamped));
        self.last = Some(cursor);
        let (line_start, line_end) = index.line_bounds(cursor.line);
        let upper = clamped.min(line_end);
        // An offset inside a multi-byte character maps to column 0.
        let character = if self.source.is_char_boundary(offset_to_index(upper)) {
            let extra_before =
                index.wide_before(cursor.wide_before).map_or(0, |wide| wide.extra_through);
            let extra = extra_before.saturating_sub(cursor.line_extra);
            upper.saturating_sub(line_start).saturating_sub(extra)
        } else {
            0
        };
        Position { line: index_to_offset(cursor.line), character }
    }

    pub(crate) fn range(&mut self, span: Span) -> Range {
        Range { start: self.position(span.start), end: self.position(span.end) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::span_text;

    const ENCODINGS: [PositionEncoding; 3] =
        [PositionEncoding::Utf8, PositionEncoding::Utf16, PositionEncoding::Utf32];

    fn units(text: &str, encoding: PositionEncoding) -> u32 {
        text.chars().map(|c| char_units(c, encoding)).sum()
    }

    fn char_units(c: char, encoding: PositionEncoding) -> u32 {
        index_to_offset(match encoding {
            PositionEncoding::Utf8 => c.len_utf8(),
            PositionEncoding::Utf16 => c.len_utf16(),
            PositionEncoding::Utf32 => 1,
        })
    }

    #[test]
    fn utf8_wins_and_otherwise_the_client_s_order_does() {
        let negotiate = |offered: &[&'static str]| {
            let kinds: Vec<PositionEncodingKind> =
                offered.iter().copied().map(PositionEncodingKind::from).collect();
            PositionEncoding::negotiate(&kinds)
        };
        assert_eq!(negotiate(&[]), PositionEncoding::Utf16, "LSP assumes UTF-16");
        assert_eq!(negotiate(&["utf-16", "utf-8"]), PositionEncoding::Utf8);
        assert_eq!(negotiate(&["utf-32", "utf-16"]), PositionEncoding::Utf32);
        assert_eq!(negotiate(&["utf-16", "utf-32"]), PositionEncoding::Utf16);
        assert_eq!(negotiate(&["latin-1"]), PositionEncoding::Utf16);
    }

    /// Stepping and searching land on one position, whichever way and
    /// however far the offsets move.
    #[test]
    fn a_cursor_agrees_with_a_search_in_any_order() {
        let line = "a\u{e4}\u{6f22}\u{1F600}b\r\n\u{e4}x\ry\u{6f22}\n\n";
        let source = format!("{}zz", line.repeat(12));
        for encoding in ENCODINGS {
            let index = LineIndex::new(&source, encoding);
            let offsets: Vec<u32> = (0..=index_to_offset(source.len()).saturating_add(2)).collect();
            let every_seventh: Vec<u32> = offsets.iter().copied().step_by(7).collect();
            let reversed: Vec<u32> = offsets.iter().rev().copied().collect();
            for run in [&offsets, &every_seventh, &reversed] {
                let mut cursor = index.positions(&source);
                for &offset in run {
                    let position = index.position(&source, offset);
                    assert_eq!(cursor.position(offset), position, "{encoding:?} {offset}");
                }
            }
        }
    }

    #[test]
    fn position_offset_roundtrip_ascii() {
        let source = "hello\nworld\nfoo";
        let index = LineIndex::new(source, PositionEncoding::Utf16);
        let position = index.position(source, 6);
        assert_eq!(position, Position { line: 1, character: 0 });
        assert_eq!(index.offset(source, position), 6);
    }

    #[test]
    fn position_handles_multi_byte() {
        // U+00E4 is 2 bytes in UTF-8 and 1 UTF-16 unit.
        let source = "\u{e4}\nb";
        let index = LineIndex::new(source, PositionEncoding::Utf16);
        let position = index.position(source, 3);
        assert_eq!(position, Position { line: 1, character: 0 });
    }

    #[test]
    fn a_column_counts_what_the_encoding_counts() {
        // 'a', an emoji of 4 bytes, 2 UTF-16 units, and 1 character, then 'b'.
        let source = "a\u{1F600}b";
        for (encoding, column) in [
            (PositionEncoding::Utf8, 5),
            (PositionEncoding::Utf16, 3),
            (PositionEncoding::Utf32, 2),
        ] {
            let index = LineIndex::new(source, encoding);
            let position = index.position(source, 5);
            assert_eq!(position, Position { line: 0, character: column }, "{encoding:?}");
            assert_eq!(index.offset(source, position), 5, "{encoding:?}");
            assert_eq!(index.length(Span::new(0, 6)), column.saturating_add(1), "{encoding:?}");
        }
    }

    /// The column of every offset is the length of the line before it in the
    /// encoding's units, and every column of every line maps back to the
    /// last character boundary at or before it.
    #[test]
    fn positions_and_offsets_count_units_on_every_line() {
        let source = "\
a\u{e4}\u{6f22}\u{1F600}b
\u{1F600}\r
\u{e4}x\ry\u{6f22}

zz";
        for encoding in ENCODINGS {
            let index = LineIndex::new(source, encoding);
            for offset in 0..=source.len() {
                let position = index.position(source, index_to_offset(offset));
                let line_start = index.line_starts[offset_to_index(position.line)];
                let expected = source
                    .get(offset_to_index(line_start)..offset)
                    .map_or(0, |line| units(line, encoding));
                assert_eq!(position.character, expected, "{encoding:?} offset {offset}");
            }
            for (line_number, &line_start) in index.line_starts.iter().enumerate() {
                let line = index_to_offset(line_number);
                let (_, line_end) = index.line_bounds(line_number);
                let terminated = span_text(source, Span::new(line_start, line_end));
                let line_text = terminated.trim_end_matches(['\n', '\r']);
                for column in 0..units(line_text, encoding).saturating_add(3) {
                    let mut walked = line_start;
                    let mut remaining = column;
                    for c in line_text.chars() {
                        let Some(rest) = remaining.checked_sub(char_units(c, encoding)) else {
                            break;
                        };
                        remaining = rest;
                        walked = walked.saturating_add(index_to_offset(c.len_utf8()));
                    }
                    let position = Position { line, character: column };
                    assert_eq!(
                        index.offset(source, position),
                        walked,
                        "{encoding:?} line {line} column {column}"
                    );
                }
            }
        }
    }

    #[test]
    fn offset_clamps_an_out_of_range_position() {
        let source = "abc";
        let index = LineIndex::new(source, PositionEncoding::Utf16);
        let position = Position { line: 99, character: 99 };
        let offset = index.offset(source, position);
        assert_eq!(offset, 3);
    }

    #[test]
    fn a_bare_cr_ends_a_line() {
        // LSP defines `\r` as an EOL alongside `\n` and `\r\n`, and Lua's
        // lexer counts it as a line break too.
        let source = "local a = 1\rlocal b = 2";
        let index = LineIndex::new(source, PositionEncoding::Utf16);
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
